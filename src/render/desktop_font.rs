//! The desktop's interface font: the family the system sets its own menus
//! and dialogs in, which misty's proportional text (labels,
//! paragraphs) takes as its sans-serif, so an app reads as the rest of the
//! desktop does rather than in whatever family the font library guesses.
//!
//! On Linux the family is the one GNOME's interface setting names, else the
//! one fontconfig matches for `sans-serif`; on macOS and Windows, the
//! system's own. Only an installed family is taken. The interface text's
//! size, which windows draw in by default, comes from the same setting.

#[cfg(all(unix, not(target_os = "macos")))]
use std::process::Command;
#[cfg(all(unix, not(target_os = "macos")))]
use std::sync::OnceLock;

use glyphon::fontdb::Database;

// ========================================================================
// Constants
// ========================================================================

/// The program and arguments that print GNOME's interface font setting, as
/// `'Noto Sans 10'`.
#[cfg(all(unix, not(target_os = "macos")))]
const GNOME_FONT_SETTING: (&str, &[&str]) = (
    "gsettings",
    &["get", "org.gnome.desktop.interface", "font-name"],
);

/// The program and arguments that print the family fontconfig matches for
/// `sans-serif`.
#[cfg(all(unix, not(target_os = "macos")))]
const FONTCONFIG_SANS: (&str, &[&str]) = ("fc-match", &["-f", "%{family[0]}", "sans-serif"]);

/// The system's interface families on macOS, most wanted first.
#[cfg(target_os = "macos")]
const SYSTEM_FAMILIES: &[&str] = &["SF Pro Text", "SF Pro", "Helvetica Neue"];

/// The system's interface family on Windows.
#[cfg(windows)]
const SYSTEM_FAMILIES: &[&str] = &["Segoe UI"];

/// Logical pixels in a point, at the 96 pixels an inch GNOME sizes fonts
/// at.
#[cfg(all(unix, not(target_os = "macos")))]
const PIXELS_PER_POINT: f32 = 96.0 / 72.0;

/// The size of the system's interface text on macOS, in logical pixels:
/// the 13 points AppKit sets its controls in.
#[cfg(target_os = "macos")]
const SYSTEM_SIZE: f32 = 13.0;

/// The size of the system's interface text on Windows, in logical pixels:
/// the 14 pixels Fluent sets its body text in.
#[cfg(windows)]
const SYSTEM_SIZE: f32 = 14.0;

// ========================================================================
// Functions
// ========================================================================

/// The desktop's interface family as `db` names it, or `None` when the
/// desktop names none that is installed.
pub(crate) fn interface_family(db: &Database) -> Option<String> {
    candidates().into_iter().find_map(|wanted| {
        db.faces()
            .flat_map(|face| face.families.iter())
            .find(|(family, _)| family.eq_ignore_ascii_case(&wanted))
            .map(|(family, _)| family.clone())
    })
}

/// The size of the desktop's interface text in logical pixels, or `None`
/// when the desktop names none.
#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) fn interface_size() -> Option<f32> {
    gnome_font_setting().as_deref().and_then(gnome_size)
}

/// The size of the system's interface text in logical pixels.
#[cfg(any(target_os = "macos", windows))]
pub(crate) fn interface_size() -> Option<f32> {
    Some(SYSTEM_SIZE)
}

/// The size of the desktop's interface text: none known here.
#[cfg(not(any(unix, windows)))]
pub(crate) fn interface_size() -> Option<f32> {
    None
}

/// The families the desktop names for its interface, most wanted first.
#[cfg(all(unix, not(target_os = "macos")))]
fn candidates() -> Vec<String> {
    let gnome = gnome_font_setting().as_deref().and_then(gnome_family);
    let fontconfig = command_output(FONTCONFIG_SANS)
        .map(|out| out.trim().to_owned())
        .filter(|family| !family.is_empty());
    gnome.into_iter().chain(fontconfig).collect()
}

/// The families the desktop names for its interface, most wanted first.
#[cfg(any(target_os = "macos", windows))]
fn candidates() -> Vec<String> {
    SYSTEM_FAMILIES
        .iter()
        .map(|family| (*family).to_owned())
        .collect()
}

/// The families the desktop names for its interface: none known here.
#[cfg(not(any(unix, windows)))]
fn candidates() -> Vec<String> {
    Vec::new()
}

/// GNOME's interface font setting as `gsettings` prints it, read once.
#[cfg(all(unix, not(target_os = "macos")))]
fn gnome_font_setting() -> &'static Option<String> {
    static SETTING: OnceLock<Option<String>> = OnceLock::new();
    SETTING.get_or_init(|| command_output(GNOME_FONT_SETTING))
}

/// What `(program, args)` prints, or `None` when it cannot run or fails.
#[cfg(all(unix, not(target_os = "macos")))]
fn command_output((program, args): (&str, &[&str])) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The family in GNOME's font setting as `gsettings` prints it (`'Noto Sans
/// 10'`): the quotes and the size taken off; `None` for an empty one.
#[cfg(all(unix, not(target_os = "macos")))]
fn gnome_family(setting: &str) -> Option<String> {
    let (family, _) = split_gnome_setting(setting);
    (!family.is_empty()).then(|| family.to_owned())
}

/// The size in GNOME's font setting (`'Noto Sans 10'`), in points, as
/// logical pixels; `None` for a setting with no size that can be drawn.
#[cfg(all(unix, not(target_os = "macos")))]
fn gnome_size(setting: &str) -> Option<f32> {
    let (_, points) = split_gnome_setting(setting);
    points
        .filter(|points| points.is_finite() && *points > 0.0)
        .map(|points| points * PIXELS_PER_POINT)
}

/// GNOME's font setting as `gsettings` prints it, its quotes taken off,
/// split into its family and its size in points when it ends in one.
#[cfg(all(unix, not(target_os = "macos")))]
fn split_gnome_setting(setting: &str) -> (&str, Option<f32>) {
    let name = setting.trim().trim_matches('\'').trim();
    match name.rsplit_once(' ') {
        Some((family, size)) => match size.parse::<f32>() {
            Ok(points) => (family.trim(), Some(points)),
            Err(_) => (name, None),
        },
        None => (name, None),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(all(test, unix, not(target_os = "macos")))]
mod tests {
    use super::*;

    #[test]
    fn test_gnomes_setting_gives_its_family_without_the_quotes_or_the_size() {
        assert_eq!(
            gnome_family("'Noto Sans 10'\n").as_deref(),
            Some("Noto Sans")
        );
        assert_eq!(
            gnome_family("'Inter Display Medium 10.5'").as_deref(),
            Some("Inter Display Medium")
        );
        assert_eq!(
            gnome_family("'Ubuntu'").as_deref(),
            Some("Ubuntu"),
            "no size"
        );
        assert_eq!(gnome_family("''"), None);
    }

    #[test]
    fn test_gnomes_setting_gives_its_size_in_points_as_logical_pixels() {
        assert_eq!(gnome_size("'Noto Sans 12'\n"), Some(16.0));
        assert_eq!(gnome_size("'Inter Display Medium 10.5'"), Some(14.0));
        assert_eq!(gnome_size("'Ubuntu'"), None, "no size");
        assert_eq!(gnome_size("'Noto Sans 0'"), None, "nothing to draw");
    }
}
