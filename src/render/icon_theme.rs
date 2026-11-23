//! [`IconTheme`], behind the `icon-theme` feature: icons from the
//! freedesktop icon theme, as a file manager shows beside each file. It
//! finds an icon by name (`folder`, `text-x-generic`) or for a file by its
//! extension's MIME type, in the theme, then the themes it inherits, then
//! `hicolor`, at the size nearest the one asked for, as SVG or PNG, and
//! keeps what it loaded.
//!
//! This is the Linux and BSD desktops' way. On Windows an app asks the
//! shell (`SHGetFileInfo`) and on macOS `NSWorkspace` for a file's icon;
//! both need FFI misty leaves to the app.

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use super::image::{Image, Svg};

// ========================================================================
// Constants
// ========================================================================

/// The theme every theme falls back to.
const FALLBACK_THEME: &str = "hicolor";

/// The file each theme describes itself in.
const INDEX_FILE: &str = "index.theme";

/// The section of the index naming the theme's directories and parents.
const THEME_SECTION: &str = "Icon Theme";

/// The icon formats read, in the order preferred.
const EXTENSIONS: [&str; 2] = ["svg", "png"];

/// How far from a threshold directory's size an icon still fits it, when
/// its index does not say.
const DEFAULT_THRESHOLD: u32 = 2;

/// The icon of a folder, and the MIME name it goes by.
const FOLDER_ICONS: [&str; 2] = ["folder", "inode-directory"];

/// The icon of a file of no known type.
const GENERIC_FILE: &str = "text-x-generic";

/// The setting naming the icon theme in a GTK `settings.ini`.
const GTK_ICON_KEY: &str = "gtk-icon-theme-name";

/// The GTK settings files, under the user's configuration directory,
/// newest GTK first.
const GTK_SETTINGS: [&str; 2] = ["gtk-4.0/settings.ini", "gtk-3.0/settings.ini"];

/// KDE's settings file, under the user's configuration directory.
const KDE_SETTINGS: &str = "kdeglobals";

/// The section of KDE's settings naming its icon theme.
const KDE_SECTION: &str = "Icons";

/// The key of KDE's settings naming its icon theme.
const KDE_ICON_KEY: &str = "Theme";

// ========================================================================
// Data Structures
// ========================================================================

/// A freedesktop icon theme and the icons loaded from it.
#[derive(Debug)]
pub struct IconTheme {
    /// What each name, size, and scale asked for found.
    cache: HashMap<(String, u32, u32), Option<ThemeIcon>>,
    /// The theme and those it inherits, `hicolor` last.
    chain: Vec<ThemeIndex>,
    /// The directories of loose icons searched last.
    pixmaps: Vec<PathBuf>,
}

/// An icon an [`IconTheme`] loaded.
#[derive(Clone, Debug)]
pub enum ThemeIcon {
    /// A PNG, drawn with [`Frame::draw_image`](crate::Frame::draw_image).
    Image(Image),
    /// An SVG, drawn with [`Frame::draw_svg`](crate::Frame::draw_svg).
    Svg(Svg),
}

/// One theme's index: where it is and its directories of icons.
#[derive(Clone, Debug, Default, PartialEq)]
struct ThemeIndex {
    /// The directories of icons.
    dirs: Vec<IconDir>,
    /// The themes it inherits, in order.
    parents: Vec<String>,
    /// Where the theme is, one root in each base directory holding it.
    roots: Vec<PathBuf>,
}

/// A directory of icons of one size.
#[derive(Clone, Debug, PartialEq)]
struct IconDir {
    /// How it holds sizes.
    kind: DirKind,
    /// The largest size it fits, for a scalable directory.
    max: u32,
    /// The smallest size it fits, for a scalable directory.
    min: u32,
    /// Its path under the theme's root.
    path: String,
    /// The scale its icons are drawn at.
    scale: u32,
    /// The size of its icons.
    size: u32,
    /// How far from its size it fits, for a threshold directory.
    threshold: u32,
}

/// How an icon directory holds sizes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DirKind {
    /// Icons of exactly its size.
    Fixed,
    /// Icons that scale from its least to its largest size.
    Scalable,
    /// Icons of about its size, within its threshold.
    Threshold,
}

// ========================================================================
// IconTheme Implementations
// ========================================================================

impl IconTheme {
    /// The theme the desktop uses (the portal's setting, else GTK's, else
    /// KDE's, else `hicolor`), found in the XDG data directories.
    pub fn system() -> Self {
        Self::named(&current_theme_name())
    }

    /// The theme named `name`, found in the XDG data directories.
    pub fn named(name: &str) -> Self {
        Self::with_dirs(name, &base_dirs(), &pixmap_dirs())
    }

    /// The theme named `name`, found in the directories `bases` (each
    /// holding themes, as `/usr/share/icons` does), with loose icons in
    /// `pixmaps` searched last.
    pub fn with_dirs(name: &str, bases: &[PathBuf], pixmaps: &[PathBuf]) -> Self {
        let mut chain = Vec::new();
        let mut seen = HashSet::new();
        let mut pending = vec![name.to_owned()];
        while let Some(theme) = pending.pop() {
            if !seen.insert(theme.clone()) {
                continue;
            }
            let Some(index) = ThemeIndex::find(&theme, bases) else {
                continue;
            };
            pending.extend(index.parents.iter().rev().cloned());
            chain.push(index);
        }
        if !seen.contains(FALLBACK_THEME) {
            chain.extend(ThemeIndex::find(FALLBACK_THEME, bases));
        }
        Self {
            cache: HashMap::new(),
            chain,
            pixmaps: pixmaps.to_vec(),
        }
    }

    /// The icon named `name` for `size` logical pixels at `scale`, loaded
    /// once and kept; `None` when no theme has it.
    pub fn icon(&mut self, name: &str, size: u32, scale: u32) -> Option<ThemeIcon> {
        let key = (name.to_owned(), size, scale);
        if let Some(found) = self.cache.get(&key) {
            return found.clone();
        }
        let loaded = self.find(name, size, scale).and_then(|path| load(&path));
        self.cache.insert(key, loaded.clone());
        loaded
    }

    /// The icon of the file at `path` (a folder when `is_dir`) for `size`
    /// at `scale`: its MIME type's icon by its extension, else its kind's
    /// generic one (`image-x-generic`), else a generic file's.
    pub fn file_icon(
        &mut self,
        path: &Path,
        is_dir: bool,
        size: u32,
        scale: u32,
    ) -> Option<ThemeIcon> {
        let names: Vec<String> = match is_dir {
            true => FOLDER_ICONS.iter().map(|name| (*name).to_owned()).collect(),
            false => {
                let mut names = Vec::new();
                if let Some(mime) = mime_guess::from_path(path).first() {
                    names.push(mime.essence_str().replace('/', "-"));
                    names.push(format!("{}-x-generic", mime.type_()));
                }
                names.push(GENERIC_FILE.to_owned());
                names
            }
        };
        names.iter().find_map(|name| self.icon(name, size, scale))
    }

    /// The file of the icon `name` for `size` at `scale`: in the first
    /// theme of the chain having it, an exact size if there is one, else
    /// the nearest; else among the loose icons.
    pub fn find(&self, name: &str, size: u32, scale: u32) -> Option<PathBuf> {
        self.chain
            .iter()
            .find_map(|theme| theme.lookup(name, size, scale))
            .or_else(|| {
                self.pixmaps
                    .iter()
                    .flat_map(|dir| {
                        EXTENSIONS.map(|extension| dir.join(format!("{name}.{extension}")))
                    })
                    .find(|path| path.is_file())
            })
    }
}

// ========================================================================
// ThemeIndex Implementations
// ========================================================================

impl ThemeIndex {
    /// The theme `name` as the first of `bases` holding its index
    /// describes it, with every base holding it as a root.
    fn find(name: &str, bases: &[PathBuf]) -> Option<Self> {
        let roots: Vec<PathBuf> = bases
            .iter()
            .map(|base| base.join(name))
            .filter(|root| root.is_dir())
            .collect();
        let text = roots
            .iter()
            .find_map(|root| fs::read_to_string(root.join(INDEX_FILE)).ok())?;
        let mut index = Self::parse(&text);
        index.roots = roots;
        Some(index)
    }

    /// The index `text` describes: its parents and its directories.
    fn parse(text: &str) -> Self {
        let sections = ini_sections(text);
        let theme = sections.get(THEME_SECTION);
        let list = |key: &str| -> Vec<String> {
            theme
                .and_then(|values| values.get(key))
                .map(|value| {
                    value
                        .split(',')
                        .map(str::trim)
                        .filter(|item| !item.is_empty())
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default()
        };
        let mut names = list("Directories");
        names.extend(list("ScaledDirectories"));
        let dirs = names
            .into_iter()
            .filter_map(|path| {
                let values = sections.get(path.as_str())?;
                let number = |key: &str| {
                    values
                        .get(key)
                        .and_then(|value| value.trim().parse::<u32>().ok())
                };
                let size = number("Size")?;
                let kind = match values.get("Type").map(|kind| kind.trim()) {
                    Some("Fixed") => DirKind::Fixed,
                    Some("Scalable") => DirKind::Scalable,
                    Some(_) | None => DirKind::Threshold,
                };
                Some(IconDir {
                    kind,
                    max: number("MaxSize").unwrap_or(size),
                    min: number("MinSize").unwrap_or(size),
                    path,
                    scale: number("Scale").unwrap_or(1),
                    size,
                    threshold: number("Threshold").unwrap_or(DEFAULT_THRESHOLD),
                })
            })
            .collect();
        Self {
            dirs,
            parents: list("Inherits"),
            roots: Vec::new(),
        }
    }

    /// The file of icon `name` in this theme for `size` at `scale`: from a
    /// directory fitting the size, else the nearest in size.
    fn lookup(&self, name: &str, size: u32, scale: u32) -> Option<PathBuf> {
        let file_in = |dir: &IconDir| {
            self.roots.iter().find_map(|root| {
                EXTENSIONS
                    .iter()
                    .map(|extension| root.join(&dir.path).join(format!("{name}.{extension}")))
                    .find(|path| path.is_file())
            })
        };
        if let Some(found) = self
            .dirs
            .iter()
            .filter(|dir| dir.fits(size, scale))
            .find_map(file_in)
        {
            return Some(found);
        }
        let mut by_distance: Vec<&IconDir> = self.dirs.iter().collect();
        by_distance.sort_by_key(|dir| dir.distance(size, scale));
        by_distance.into_iter().find_map(file_in)
    }
}

// ========================================================================
// IconDir Implementations
// ========================================================================

impl IconDir {
    /// Whether its icons fit `size` at `scale`.
    fn fits(&self, size: u32, scale: u32) -> bool {
        if self.scale != scale {
            return false;
        }
        match self.kind {
            DirKind::Fixed => self.size == size,
            DirKind::Scalable => (self.min..=self.max).contains(&size),
            DirKind::Threshold => size.abs_diff(self.size) <= self.threshold,
        }
    }

    /// How far its icons are from `size` at `scale`, in physical pixels.
    fn distance(&self, size: u32, scale: u32) -> u32 {
        let wanted = size * scale;
        let (low, high) = match self.kind {
            DirKind::Fixed => (self.size, self.size),
            DirKind::Scalable => (self.min, self.max),
            DirKind::Threshold => (
                self.size.saturating_sub(self.threshold),
                self.size + self.threshold,
            ),
        };
        let (low, high) = (low * self.scale, high * self.scale);
        match wanted {
            wanted if wanted < low => low - wanted,
            wanted if wanted > high => wanted - high,
            _ => 0,
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The icon in the file at `path`: SVG or PNG by its extension.
fn load(path: &Path) -> Option<ThemeIcon> {
    let bytes = fs::read(path).ok()?;
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("svg") => Svg::parse(&bytes).ok().map(ThemeIcon::Svg),
        Some(_) | None => Image::decode(&bytes).ok().map(ThemeIcon::Image),
    }
}

/// The sections of an INI `text`, each its keys' values.
fn ini_sections(text: &str) -> HashMap<String, HashMap<String, String>> {
    let mut sections: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut current = String::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            current = name.trim().to_owned();
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            sections
                .entry(current.clone())
                .or_default()
                .insert(key.trim().to_owned(), value.trim().to_owned());
        }
    }
    sections
}

/// The directories holding icon themes, most personal first:
/// `~/.icons`, then `icons` under each XDG data directory.
fn base_dirs() -> Vec<PathBuf> {
    let home = env::var_os("HOME").map(PathBuf::from);
    let data_home = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|home| home.join(".local/share")));
    let data_dirs =
        env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".to_owned());
    home.map(|home| home.join(".icons"))
        .into_iter()
        .chain(data_home.map(|dir| dir.join("icons")))
        .chain(
            data_dirs
                .split(':')
                .filter(|dir| !dir.is_empty())
                .map(|dir| Path::new(dir).join("icons")),
        )
        .collect()
}

/// The directories of loose icons searched after every theme.
fn pixmap_dirs() -> Vec<PathBuf> {
    vec![PathBuf::from("/usr/share/pixmaps")]
}

/// The icon theme the desktop names: the portal's (GNOME) setting, else
/// GTK's settings file, else KDE's, else `hicolor`.
fn current_theme_name() -> String {
    #[cfg(all(
        unix,
        not(any(target_os = "macos", target_os = "ios", target_os = "android"))
    ))]
    if let Some(name) = super::super::platform::appearance::icon_theme_name() {
        return name;
    }
    let config = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    let Some(config) = config else {
        return FALLBACK_THEME.to_owned();
    };
    let setting = |file: &str, section: &str, key: &str| {
        let text = fs::read_to_string(config.join(file)).ok()?;
        ini_sections(&text).get(section)?.get(key).cloned()
    };
    GTK_SETTINGS
        .iter()
        .find_map(|file| setting(file, "Settings", GTK_ICON_KEY))
        .or_else(|| setting(KDE_SETTINGS, KDE_SECTION, KDE_ICON_KEY))
        .unwrap_or_else(|| FALLBACK_THEME.to_owned())
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const SQUARE: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"16\" height=\"16\"><rect width=\"16\" height=\"16\"/></svg>";

    /// A theme directory under the scratch directory, unique to `test`.
    fn scratch(test: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("misty-icons-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    /// Writes theme `name` under `base` inheriting `parents`, with icon
    /// `icons` (name, directory) in its `16x16/apps` (fixed 16) and
    /// `scalable/apps` (8 to 512) directories.
    fn theme(base: &Path, name: &str, parents: &str, icons: &[(&str, &str)]) {
        let root = base.join(name);
        let index = format!(
            "[Icon Theme]\nName={name}\nInherits={parents}\nDirectories=16x16/apps,scalable/apps\n\n\
             [16x16/apps]\nSize=16\nType=Fixed\n\n[scalable/apps]\nSize=64\nMinSize=8\nMaxSize=512\nType=Scalable\n"
        );
        fs::create_dir_all(&root).expect("a theme root");
        fs::write(root.join(INDEX_FILE), index).expect("an index");
        for (icon, dir) in icons {
            let dir = root.join(dir);
            fs::create_dir_all(&dir).expect("an icon directory");
            fs::write(dir.join(format!("{icon}.svg")), SQUARE).expect("an icon");
        }
    }

    #[test]
    fn test_icons_come_from_the_theme_then_its_parents_then_hicolor() {
        let base = scratch("chain");
        theme(&base, "Child", "Parent", &[("folder", "16x16/apps")]);
        theme(&base, "Parent", "", &[("text-x-generic", "scalable/apps")]);
        theme(
            &base,
            FALLBACK_THEME,
            "",
            &[("only-in-hicolor", "16x16/apps")],
        );
        let mut icons = IconTheme::with_dirs("Child", std::slice::from_ref(&base), &[]);
        let path = |icons: &IconTheme, name| {
            icons
                .find(name, 16, 1)
                .map(|path| path.strip_prefix(&base).map(Path::to_path_buf))
        };
        assert_eq!(
            path(&icons, "folder"),
            Some(Ok(PathBuf::from("Child/16x16/apps/folder.svg")))
        );
        assert_eq!(
            path(&icons, "text-x-generic"),
            Some(Ok(PathBuf::from("Parent/scalable/apps/text-x-generic.svg")))
        );
        assert_eq!(
            path(&icons, "only-in-hicolor"),
            Some(Ok(PathBuf::from("hicolor/16x16/apps/only-in-hicolor.svg")))
        );
        assert!(icons.find("missing", 16, 1).is_none());
        assert!(matches!(
            icons.icon("folder", 16, 1),
            Some(ThemeIcon::Svg(_))
        ));
        assert!(
            matches!(
                icons.file_icon(Path::new("notes.weird"), false, 16, 1),
                Some(ThemeIcon::Svg(_))
            ),
            "a generic file"
        );
        assert!(matches!(
            icons.file_icon(Path::new("docs"), true, 16, 1),
            Some(ThemeIcon::Svg(_))
        ));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_an_exact_size_wins_and_else_the_nearest_directory_serves() {
        let base = scratch("sizes");
        theme(
            &base,
            "Sized",
            "",
            &[("app", "16x16/apps"), ("app", "scalable/apps")],
        );
        let icons = IconTheme::with_dirs("Sized", std::slice::from_ref(&base), &[]);
        let found = |size| icons.find("app", size, 1).expect("found");
        assert!(found(16).ends_with("16x16/apps/app.svg"));
        assert!(
            found(48).ends_with("scalable/apps/app.svg"),
            "in the scalable range"
        );
        assert!(
            found(1024).ends_with("scalable/apps/app.svg"),
            "the nearest past its range"
        );
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_a_mime_type_names_its_icon_and_its_generic_one() {
        let base = scratch("mime");
        theme(
            &base,
            "Mime",
            "",
            &[
                ("image-x-generic", "16x16/apps"),
                ("text-x-generic", "16x16/apps"),
            ],
        );
        let mut icons = IconTheme::with_dirs("Mime", std::slice::from_ref(&base), &[]);
        let picked = icons.file_icon(Path::new("cat.png"), false, 16, 1);
        assert!(
            matches!(picked, Some(ThemeIcon::Svg(_))),
            "image/png falls back to image-x-generic"
        );
        assert!(
            icons
                .cache
                .contains_key(&("image-x-generic".to_owned(), 16, 1))
        );
        assert!(
            icons
                .cache
                .get(&("image-png".to_owned(), 16, 1))
                .is_some_and(Option::is_none),
            "image-png tried first"
        );
        let _ = fs::remove_dir_all(&base);
    }
}
