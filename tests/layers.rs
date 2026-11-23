//! The layering rules of misty's modules, checked on the source: a module
//! may import only from its own layer and the layers below it, only the
//! lower layers and the runtime talk to `winit`, and the base layer imports no
//! GPU, window, or other GUI crate. Tests inside a module may use anything.
//!
//! The layers, lowest first, are the top-level folders of `src`: `base`,
//! `platform`, `render`, `input`, `app`, `widgets`, `chrome`, `view`,
//! `runtime`; the modules at the top (`lib.rs`, `testing`, `bench.rs`)
//! sit above them all.

use std::fs;
use std::path::{Path, PathBuf};

// ========================================================================
// Constants
// ========================================================================

/// The layers of `src`, lowest first.
const LAYERS: [&str; 9] = [
    "base", "platform", "render", "input", "app", "widgets", "chrome", "view", "runtime",
];

/// The layers that may import `winit`.
const WINIT_LAYERS: [&str; 5] = ["platform", "render", "input", "app", "runtime"];

/// The crates the base layer must not import, as their paths are written
/// in code: everything misty depends on for drawing, windows, and input.
const GUI_CRATES: [&str; 10] = [
    "accesskit::",
    "accesskit_winit::",
    "arboard::",
    "glyphon::",
    "image::",
    "pollster::",
    "resvg::",
    "taffy::",
    "wgpu::",
    "winit::",
];

/// What opens the tests of a module, after which anything goes.
const TESTS_START: &str = "mod tests {";

// ========================================================================
// Data Structures
// ========================================================================

/// One source file of a crate.
struct Source {
    /// Its module path from the crate root (`widgets`, `list_view`).
    module: Vec<String>,
    /// Its path under `src`, with `/` separators.
    relative: String,
    /// Its code before its tests, comments left out.
    code: String,
}

// ========================================================================
// Source Implementations
// ========================================================================

impl Source {
    /// The layer the file sits in, or `None` for a file at the top.
    fn layer(&self) -> Option<usize> {
        let top = self.module.first()?;
        LAYERS.iter().position(|layer| layer == top)
    }

    /// The top-level modules the file's code imports from, by `crate::`
    /// and `super::` paths.
    fn imported_tops(&self) -> Vec<String> {
        let mut tops = Vec::new();
        for (start, _) in self
            .code
            .match_indices("super::")
            .chain(self.code.match_indices("crate::"))
        {
            let before = self.code[..start].chars().next_back();
            if before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == ':') {
                continue;
            }
            let path: Vec<&str> = self.code[start..]
                .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':'))
                .next()
                .unwrap_or_default()
                .split("::")
                .filter(|segment| !segment.is_empty())
                .collect();
            if let Some(top) = absolute_top(&self.module, &path) {
                tops.push(top);
            }
        }
        tops
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The top-level module a path written in `module` lands in, if the path
/// leaves `module`'s own folder for another top-level module.
fn absolute_top(module: &[String], path: &[&str]) -> Option<String> {
    let mut base: Vec<String> = module.to_vec();
    let mut rest = path;
    match rest.first() {
        Some(&"crate") => {
            base.clear();
            rest = &rest[1..];
        }
        Some(&"super") => {
            while rest.first() == Some(&"super") {
                base.pop()?;
                rest = &rest[1..];
            }
        }
        Some(_) | None => return None,
    }
    match base.first() {
        Some(top) => Some(top.clone()),
        None => rest.first().map(|segment| (*segment).to_owned()),
    }
}

/// Every `.rs` file under `dir`, recursively.
fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).expect("a readable source folder") {
        let path = entry.expect("a readable entry").path();
        if path.is_dir() {
            files.extend(rust_files(&path));
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
    files
}

/// The sources of misty's `src` folder.
fn sources() -> Vec<Source> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    rust_files(&src)
        .into_iter()
        .map(|path| {
            let relative = path
                .strip_prefix(&src)
                .expect("under src")
                .to_string_lossy()
                .replace('\\', "/");
            let mut module: Vec<String> = relative
                .trim_end_matches(".rs")
                .split('/')
                .map(str::to_owned)
                .collect();
            if module
                .last()
                .is_some_and(|last| last == "mod" || last == "lib")
            {
                module.pop();
            }
            let text = fs::read_to_string(&path).expect("a readable source");
            let before_tests = text.split(TESTS_START).next().unwrap_or_default();
            let code = before_tests
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            Source {
                module,
                relative,
                code,
            }
        })
        .collect()
}

/// Whether `source` names one of `crates` in its code.
fn names_any(source: &Source, crates: &[&str]) -> bool {
    crates.iter().any(|name| {
        source.code.match_indices(name).any(|(start, _)| {
            let before = source.code[..start].chars().next_back();
            !before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == ':')
        })
    })
}

// ========================================================================
// Tests
// ========================================================================

#[test]
fn test_no_module_imports_from_a_layer_above_its_own() {
    let mut violations = Vec::new();
    for source in sources() {
        let Some(layer) = source.layer() else {
            continue;
        };
        for top in source.imported_tops() {
            let above = LAYERS
                .iter()
                .position(|candidate| *candidate == top)
                .is_none_or(|imported| imported > layer);
            let is_top_file = ["testing", "bench"].contains(&top.as_str());
            if above && (is_top_file || LAYERS.contains(&top.as_str())) {
                violations.push(format!("{} imports from {top}", source.relative));
            }
        }
    }
    assert!(violations.is_empty(), "{violations:#?}");
}

#[test]
fn test_only_the_lower_layers_and_the_runtime_import_winit() {
    let violations: Vec<String> = sources()
        .into_iter()
        .filter(|source| source.code.contains("winit::"))
        .filter_map(|source| {
            let layer = LAYERS[source.layer()?];
            (!WINIT_LAYERS.contains(&layer)).then_some(source.relative)
        })
        .collect();
    assert!(violations.is_empty(), "{violations:#?}");
}

#[test]
fn test_the_base_layer_imports_no_gui_crate() {
    let violations: Vec<String> = sources()
        .into_iter()
        .filter(|source| source.layer() == Some(0))
        .filter(|source| names_any(source, &GUI_CRATES))
        .map(|source| source.relative)
        .collect();
    assert!(violations.is_empty(), "{violations:#?}");
}
