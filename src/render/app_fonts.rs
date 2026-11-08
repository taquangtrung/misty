//! Fonts an app ships with itself: [`load_font`] takes a font file's
//! bytes, and every window's text can then name its families with
//! [`FontFamily::Named`](crate::FontFamily::Named).
//!
//! The fonts are kept for the whole process, so an app may load them
//! before [`run`](crate::run) or at any time after; text already on screen
//! is shaped again with them on the next frame.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use glyphon::FontSystem;
use glyphon::fontdb::{Database, Source};
use thiserror::Error;

// ========================================================================
// Constants
// ========================================================================

/// The fonts every [`FontSystem`] the process makes loads.
static REGISTRY: Registry = Registry {
    files: Mutex::new(Vec::new()),
    len: AtomicUsize::new(0),
};

// ========================================================================
// Data Structures
// ========================================================================

/// Why [`load_font`] refused the bytes it was given.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum FontError {
    /// The bytes hold no font face misty can read.
    #[error(
        "the data is not a font misty can read: pass the whole contents of a TrueType (.ttf), \
         OpenType (.otf), or font collection (.ttc) file"
    )]
    NotAFont,
}

/// The font files loaded so far, in the order loaded.
struct Registry {
    /// Each file's bytes.
    files: Mutex<Vec<Arc<Vec<u8>>>>,
    /// How many files there are, read without the lock.
    len: AtomicUsize,
}

// ========================================================================
// Functions
// ========================================================================

/// Loads the font file `data` (the bytes of a `.ttf`, `.otf`, or `.ttc`)
/// for every window to draw with, and returns the names of the families it
/// holds, to name in [`FontFamily::Named`](crate::FontFamily::Named).
/// Text naming a family keeps its look until the font is loaded, so load
/// fonts before opening the first window when that matters.
pub fn load_font(data: impl Into<Vec<u8>>) -> Result<Vec<String>, FontError> {
    let data = Arc::new(data.into());
    let mut probe = Database::new();
    probe.load_font_source(Source::Binary(Arc::clone(&data) as _));
    let mut families: Vec<String> = Vec::new();
    for face in probe.faces() {
        if let Some((name, _)) = face.families.first()
            && !families.contains(name)
        {
            families.push(name.clone());
        }
    }
    if families.is_empty() {
        return Err(FontError::NotAFont);
    }
    let mut files = REGISTRY
        .files
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    files.push(data);
    REGISTRY.len.store(files.len(), Ordering::Release);
    Ok(families)
}

/// Loads into `font_system` the files loaded after the first `loaded`, and
/// returns how many there are now.
pub(crate) fn sync(font_system: &mut FontSystem, loaded: usize) -> usize {
    if REGISTRY.len.load(Ordering::Acquire) == loaded {
        return loaded;
    }
    let files = REGISTRY
        .files
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let db = font_system.db_mut();
    for file in files.iter().skip(loaded) {
        db.load_font_source(Source::Binary(Arc::clone(file) as _));
    }
    files.len()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    /// The path of some installed TrueType font, when fontconfig lists one.
    fn installed_font() -> Option<String> {
        let output = Command::new("fc-list")
            .args([":fontformat=TrueType", "file"])
            .output()
            .ok()?;
        String::from_utf8(output.stdout)
            .ok()?
            .lines()
            .find_map(|line| {
                let path = line.trim().trim_end_matches(':');
                path.ends_with(".ttf").then(|| path.to_owned())
            })
    }

    #[test]
    fn test_bytes_that_are_no_font_are_refused() {
        assert_eq!(load_font(b"not a font".to_vec()), Err(FontError::NotAFont));
        assert_eq!(load_font(Vec::new()), Err(FontError::NotAFont));
    }

    #[test]
    fn test_a_loaded_font_names_its_family_and_reaches_a_font_system_once() {
        let Some(path) = installed_font() else {
            eprintln!("skipped: fontconfig lists no TrueType font");
            return;
        };
        let data = std::fs::read(&path).expect("a listed font file reads");
        let mut font_system = FontSystem::new_with_fonts([]);
        let loaded = sync(&mut font_system, 0);
        let before = font_system.db().len();
        let families = load_font(data).expect("an installed font loads");
        assert!(!families.is_empty(), "{path} names a family");
        let now = sync(&mut font_system, loaded);
        assert!(now > loaded);
        assert!(font_system.db().len() > before, "the faces were added");
        let after = font_system.db().len();
        assert_eq!(sync(&mut font_system, now), now);
        assert_eq!(font_system.db().len(), after, "nothing loaded twice");
    }
}
