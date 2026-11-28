//! Fonts an app ships with itself: [`load_font`] takes a font file's
//! bytes, and every window's text can then name its families with
//! [`FontFamily::Named`](crate::FontFamily::Named).
//!
//! The fonts are kept for the whole process, so an app may load them
//! before [`run`](crate::run) or at any time after; text already on screen
//! is shaped again with them on the next frame. [`font_families`] lists
//! the families of the installed fonts and the loaded ones, for a font
//! picker.
//!
//! The installed fonts are scanned once for the process; every font
//! system misty makes (the windows' and each
//! [`ParagraphMeasurer`](crate::ParagraphMeasurer)'s) starts from that
//! scan, with misty's inline box font and the app's fonts added in the
//! same order, so they all choose the same fonts for the same text.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use glyphon::FontSystem;
use glyphon::fontdb::{Database, Source};
use thiserror::Error;

use super::desktop_font;
use super::inline_font;

// ========================================================================
// Constants
// ========================================================================

/// The fonts every [`FontSystem`] the process makes loads.
static REGISTRY: Registry = Registry {
    files: Mutex::new(Vec::new()),
    len: AtomicUsize::new(0),
};

/// The installed fonts, scanned the first time they are needed.
static SYSTEM_FONTS: OnceLock<SystemFonts> = OnceLock::new();

/// The family proportional text is set in, where the app named one
/// ([`UiFont::with_family`](crate::UiFont::with_family)); `None` for the
/// desktop's.
static UI_FAMILY: Mutex<Option<String>> = Mutex::new(None);

/// How many times the app changed [`UI_FAMILY`], so each font system sets
/// its family again, and drops what it shaped in the old one, once.
static UI_FAMILY_VERSION: AtomicU64 = AtomicU64::new(0);

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

/// The installed fonts as scanned once, with misty's inline box font.
/// What a font system holds of what the app gave: how many of its fonts,
/// and which of its UI families, by [`UI_FAMILY_VERSION`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Synced {
    /// The UI family's version it was set to.
    family: u64,
    /// How many of the app's fonts it loaded.
    files: usize,
}

struct SystemFonts {
    /// The faces, and the default families cosmic-text sets.
    db: Database,
    /// The desktop's interface family, which proportional text is set in
    /// unless the app names its own.
    desktop_family: Option<String>,
    /// The system's locale, which picks fallback fonts.
    locale: String,
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

/// The family names of the installed fonts and those the app loaded with
/// [`load_font`], each once, sorted ignoring case: what a font picker
/// offers, each name ready for
/// [`FontFamily::Named`](crate::FontFamily::Named) or
/// [`Font::with_family`](crate::Font::with_family). Needs no window; the
/// first call in a process scans the installed fonts (up to a second in a
/// debug build), so call it off the UI thread or before opening windows
/// when that matters.
pub fn font_families() -> Vec<String> {
    let mut db = system_fonts().db.clone();
    sync_db(&mut db, 0);
    let mut families: Vec<String> = db
        .faces()
        .filter_map(|face| face.families.first())
        .map(|(name, _)| name.clone())
        .filter(|name| name != inline_font::FAMILY)
        .collect();
    families.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()).then(a.cmp(b)));
    families.dedup();
    families
}

/// A font system of the installed fonts, misty's inline box font, and the
/// app's fonts, and what of the app's it holds.
pub(crate) fn font_system() -> (FontSystem, Synced) {
    let fonts = system_fonts();
    let mut font_system =
        FontSystem::new_with_locale_and_db(fonts.locale.clone(), fonts.db.clone());
    let synced = sync(&mut font_system, Synced::default());
    (font_system, synced)
}

/// The installed fonts, scanned now unless they were already.
fn system_fonts() -> &'static SystemFonts {
    SYSTEM_FONTS.get_or_init(|| {
        let (locale, mut db) = FontSystem::new().into_locale_and_db();
        db.load_font_source(Source::Binary(Arc::new(inline_font::font_data())));
        // Proportional text in the desktop's own interface font, where the
        // desktop names one that is installed.
        let desktop_family = desktop_font::interface_family(&db);
        if let Some(family) = &desktop_family {
            db.set_sans_serif_family(family.clone());
        }
        SystemFonts {
            db,
            desktop_family,
            locale,
        }
    })
}

/// Brings `font_system` up to date from `synced`, what it had: the app's
/// fonts loaded since, and the UI family the app named since; returns what
/// it has now, which a cache shaped with the old compares unequal to.
pub(crate) fn sync(font_system: &mut FontSystem, synced: Synced) -> Synced {
    let mut now = synced;
    if REGISTRY.len.load(Ordering::Acquire) != synced.files {
        now.files = sync_db(font_system.db_mut(), synced.files);
    }
    let version = UI_FAMILY_VERSION.load(Ordering::Acquire);
    if version != synced.family {
        set_family(font_system.db_mut());
        now.family = version;
    }
    now
}

/// Sets proportional text in `family` from now on, in every window, or in
/// the desktop's interface family for `None`.
pub(crate) fn set_ui_family(family: Option<&str>) {
    let mut named = UI_FAMILY.lock().unwrap_or_else(PoisonError::into_inner);
    if named.as_deref() != family {
        *named = family.map(str::to_owned);
        UI_FAMILY_VERSION.fetch_add(1, Ordering::AcqRel);
    }
}

/// Makes `db`'s sans-serif family, which proportional text is shaped in,
/// the one the app named, or the desktop's.
fn set_family(db: &mut Database) {
    let named = UI_FAMILY.lock().unwrap_or_else(PoisonError::into_inner);
    let family = named
        .clone()
        .or_else(|| system_fonts().desktop_family.clone());
    if let Some(family) = family {
        db.set_sans_serif_family(family);
    }
}

/// Loads into `db` the files loaded after the first `loaded`, and returns
/// how many there are now.
fn sync_db(db: &mut Database, loaded: usize) -> usize {
    let files = REGISTRY
        .files
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
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
    fn test_font_families_are_sorted_once_each_and_include_a_loaded_font_but_not_misty_s() {
        let Some(path) = installed_font() else {
            eprintln!("skipped: fontconfig lists no TrueType font");
            return;
        };
        let data = std::fs::read(&path).expect("a listed font file reads");
        let loaded = load_font(data).expect("an installed font loads");
        let families = font_families();
        assert!(loaded.iter().all(|name| families.contains(name)));
        assert!(!families.iter().any(|name| name == inline_font::FAMILY));
        let lower: Vec<String> = families.iter().map(|name| name.to_lowercase()).collect();
        assert!(lower.windows(2).all(|pair| pair[0] <= pair[1]), "sorted");
        assert!(
            families.windows(2).all(|pair| pair[0] != pair[1]),
            "once each"
        );
    }

    #[test]
    fn test_naming_a_ui_family_reaches_a_font_system_and_drops_its_shapes_once() {
        // The desktop's own family, named: a change every font system must
        // take, yet the same faces for the tests shaping text meanwhile.
        let Some(desktop) = system_fonts().desktop_family.clone() else {
            eprintln!("skipped: the desktop names no interface family");
            return;
        };
        let (mut font_system, synced) = font_system();
        set_ui_family(Some(&desktop));
        let named = sync(&mut font_system, synced);
        assert_ne!(named, synced, "what was shaped before no longer matches");
        let sans = font_system
            .db()
            .family_name(&glyphon::fontdb::Family::SansSerif);
        assert_eq!(sans, desktop);
        assert_eq!(sync(&mut font_system, named), named, "taken once");
        set_ui_family(None);
        assert_ne!(
            sync(&mut font_system, named),
            named,
            "and back to the desktop's"
        );
    }

    #[test]
    fn test_a_loaded_font_names_its_family_and_reaches_a_font_system_once() {
        let Some(path) = installed_font() else {
            eprintln!("skipped: fontconfig lists no TrueType font");
            return;
        };
        let data = std::fs::read(&path).expect("a listed font file reads");
        let mut font_system = FontSystem::new_with_fonts([]);
        let loaded = sync(&mut font_system, Synced::default());
        let before = font_system.db().len();
        let families = load_font(data).expect("an installed font loads");
        assert!(!families.is_empty(), "{path} names a family");
        let now = sync(&mut font_system, loaded);
        assert!(now.files > loaded.files);
        assert!(font_system.db().len() > before, "the faces were added");
        let after = font_system.db().len();
        assert_eq!(sync(&mut font_system, now), now);
        assert_eq!(font_system.db().len(), after, "nothing loaded twice");
    }
}
