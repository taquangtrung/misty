//! [`Appearance`]: how the user asked their operating system to show apps
//! (light or dark, an accent color, high contrast, less motion), for an
//! app to follow with [`Appearance::theme`] and its widgets'
//! reduced-motion settings.

use super::color::Rgb;
use super::theme::Theme;

// ========================================================================
// Data Structures
// ========================================================================

/// What the user asked of every app's look in their system settings. Each
/// part the system does not say (or misty cannot read on this platform)
/// is left out: `None`, or `false` for the flags.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Appearance {
    /// The accent color the user picked, for focus and selection.
    pub accent: Option<Rgb>,
    /// Whether the user asked for light or dark apps.
    pub color_scheme: Option<ColorScheme>,
    /// Whether the user asked for high contrast.
    pub high_contrast: bool,
    /// Whether the user asked for less motion: scrolls that jump instead
    /// of gliding, no sweeping animations.
    pub reduced_motion: bool,
}

/// Light or dark, as the user asked of apps.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ColorScheme {
    /// Light text on a dark background.
    Dark,
    /// Dark text on a light background.
    Light,
}

// ========================================================================
// Appearance Implementations
// ========================================================================

impl Appearance {
    /// The built-in theme that follows this appearance: [`Theme::light`]
    /// when the user asked for light apps, [`Theme::dark`] otherwise, in
    /// the user's accent color when they picked one. High contrast and
    /// reduced motion are left to the app.
    pub fn theme(&self) -> Theme {
        let theme = match self.color_scheme {
            Some(ColorScheme::Light) => Theme::light(),
            Some(ColorScheme::Dark) | None => Theme::dark(),
        };
        match self.accent {
            Some(accent) => Theme { accent, ..theme },
            None => theme,
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_the_theme_follows_the_scheme_and_takes_the_accent() {
        let unknown = Appearance::default();
        assert_eq!(unknown.theme(), Theme::dark(), "dark when nothing is said");
        let light = Appearance {
            color_scheme: Some(ColorScheme::Light),
            ..Appearance::default()
        };
        assert_eq!(light.theme(), Theme::light());
        let accent = Rgb::new(0x12, 0x34, 0x56);
        let accented = Appearance {
            accent: Some(accent),
            ..light
        };
        assert_eq!(
            accented.theme(),
            Theme {
                accent,
                ..Theme::light()
            }
        );
    }
}
