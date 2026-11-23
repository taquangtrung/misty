//! The desktop portal's appearance settings over D-Bus (light or dark, the
//! accent color, high contrast, and GNOME's animations switch): read once
//! at start, waiting no longer than a moment, and watched on a thread.

use std::thread;
use std::time::Duration;

use zbus::blocking::connection::Builder;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{OwnedValue, Value};

use super::super::super::base::appearance::{Appearance, ColorScheme};
use super::super::super::base::color::Rgb;

// ========================================================================
// Constants
// ========================================================================

/// The portal's code for "the user asked for dark apps".
const PORTAL_DARK: u32 = 1;

/// The portal's code for "the user asked for light apps".
const PORTAL_LIGHT: u32 = 2;

/// The portal's code for "the user asked for high contrast".
const PORTAL_HIGH_CONTRAST: u32 = 1;

/// The largest value of a color channel in the portal's accent color.
const PORTAL_CHANNEL_MAX: f64 = 1.0;

/// The largest value of a color channel in an [`Rgb`].
const RGB_CHANNEL_MAX: f64 = 255.0;

/// The settings namespace of the portal's own appearance keys.
const APPEARANCE_NAMESPACE: &str = "org.freedesktop.appearance";

/// The key of light or dark.
const COLOR_SCHEME_KEY: &str = "color-scheme";

/// The key of the accent color.
const ACCENT_KEY: &str = "accent-color";

/// The key of high contrast.
const CONTRAST_KEY: &str = "contrast";

/// The settings namespace of GNOME's interface keys, which the portal
/// passes on.
const INTERFACE_NAMESPACE: &str = "org.gnome.desktop.interface";

/// GNOME's key for whether apps animate.
const ANIMATIONS_KEY: &str = "enable-animations";

/// GNOME's interface key naming the icon theme.
#[cfg(feature = "icon-theme")]
const ICON_THEME_KEY: &str = "icon-theme";

/// Every setting misty follows, by namespace and key.
const KEYS: [(&str, &str); 4] = [
    (APPEARANCE_NAMESPACE, COLOR_SCHEME_KEY),
    (APPEARANCE_NAMESPACE, ACCENT_KEY),
    (APPEARANCE_NAMESPACE, CONTRAST_KEY),
    (INTERFACE_NAMESPACE, ANIMATIONS_KEY),
];

/// The bus name of the desktop portal.
const PORTAL_DESTINATION: &str = "org.freedesktop.portal.Desktop";

/// The object path of the desktop portal.
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";

/// The portal's settings interface.
const SETTINGS_INTERFACE: &str = "org.freedesktop.portal.Settings";

/// The method reading one setting.
const READ_ONE: &str = "ReadOne";

/// The signal saying a setting changed.
const SETTING_CHANGED: &str = "SettingChanged";

/// How long the read at start may wait for the portal, so a missing or
/// slow portal never holds up the first window.
const STARTUP_TIMEOUT: Duration = Duration::from_millis(250);

/// The name of the thread watching the settings.
const WATCHER_NAME: &str = "misty appearance";

// ========================================================================
// Data Structures
// ========================================================================

/// One setting the portal reports, decoded.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Setting {
    /// The accent color, or `None` when the user picked none.
    Accent(Option<Rgb>),
    /// Whether apps animate.
    Animations(bool),
    /// Light, dark, or no preference.
    ColorScheme(Option<ColorScheme>),
    /// Whether the user asked for high contrast.
    HighContrast(bool),
}

/// A setting's value, as far as decoding needs to see it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum RawValue {
    /// A flag.
    Bool(bool),
    /// A color as three channels from 0 to 1.
    Color(f64, f64, f64),
    /// A code.
    Code(u32),
    /// Any other shape, which no known key has.
    Other,
}

// ========================================================================
// Setting Implementations
// ========================================================================

impl Setting {
    /// The setting `key` of `namespace` holding `value`, or `None` for a
    /// key misty does not follow or a value of the wrong shape.
    fn decode(namespace: &str, key: &str, value: RawValue) -> Option<Self> {
        match (namespace, key, value) {
            (APPEARANCE_NAMESPACE, COLOR_SCHEME_KEY, RawValue::Code(code)) => {
                Some(Self::ColorScheme(color_scheme_of(code)))
            }
            (APPEARANCE_NAMESPACE, ACCENT_KEY, RawValue::Color(r, g, b)) => {
                Some(Self::Accent(accent_of(r, g, b)))
            }
            (APPEARANCE_NAMESPACE, CONTRAST_KEY, RawValue::Code(code)) => {
                Some(Self::HighContrast(code == PORTAL_HIGH_CONTRAST))
            }
            (INTERFACE_NAMESPACE, ANIMATIONS_KEY, RawValue::Bool(on)) => Some(Self::Animations(on)),
            _ => None,
        }
    }

    /// Writes this setting into `appearance`.
    fn apply(self, appearance: &mut Appearance) {
        match self {
            Self::Accent(accent) => appearance.accent = accent,
            Self::Animations(on) => appearance.reduced_motion = !on,
            Self::ColorScheme(scheme) => appearance.color_scheme = scheme,
            Self::HighContrast(high) => appearance.high_contrast = high,
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The user's appearance as the portal reports it now, waiting no longer
/// than a moment; what it does not say is left out.
pub(crate) fn read() -> Appearance {
    Builder::session()
        .map(|builder| builder.method_timeout(STARTUP_TIMEOUT))
        .and_then(Builder::build)
        .ok()
        .and_then(|connection| settings(&connection).ok().map(|proxy| read_all(&proxy)))
        .unwrap_or_default()
}

/// The icon theme the desktop names through the portal (GNOME's
/// interface setting), waiting no longer than a moment; `None` without a
/// portal or the setting.
#[cfg(feature = "icon-theme")]
pub(crate) fn icon_theme_name() -> Option<String> {
    let connection = Builder::session()
        .ok()?
        .method_timeout(STARTUP_TIMEOUT)
        .build()
        .ok()?;
    let proxy = settings(&connection).ok()?;
    let value: OwnedValue = proxy
        .call(READ_ONE, &(INTERFACE_NAMESPACE, ICON_THEME_KEY))
        .ok()?;
    string_of(&value)
}

/// The text `value` holds, looking through the variants a portal may wrap
/// it in.
#[cfg(feature = "icon-theme")]
fn string_of(value: &Value<'_>) -> Option<String> {
    match value {
        Value::Str(text) => Some(text.to_string()),
        Value::Value(inner) => string_of(inner),
        _ => None,
    }
}

/// Watches the portal's settings on a thread of its own, calling
/// `changed` with the whole appearance each time a part of it changes.
/// Without a session bus or a portal the thread ends at once.
pub(crate) fn watch(start: Appearance, changed: impl Fn(Appearance) + Send + 'static) {
    // A system that cannot start a thread only loses the updates.
    let _ = thread::Builder::new()
        .name(WATCHER_NAME.to_owned())
        .spawn(move || {
            let Ok(connection) = Connection::session() else {
                return;
            };
            let Ok(proxy) = settings(&connection) else {
                return;
            };
            let Ok(signals) = proxy.receive_signal(SETTING_CHANGED) else {
                return;
            };
            let mut appearance = start;
            for message in signals {
                let Ok((namespace, key, value)) =
                    message.body().deserialize::<(String, String, OwnedValue)>()
                else {
                    continue;
                };
                let Some(setting) = Setting::decode(&namespace, &key, raw_value(&value)) else {
                    continue;
                };
                let before = appearance;
                setting.apply(&mut appearance);
                if appearance != before {
                    changed(appearance);
                }
            }
        });
}

/// The portal's settings interface on `connection`.
fn settings(connection: &Connection) -> zbus::Result<Proxy<'_>> {
    Proxy::new(
        connection,
        PORTAL_DESTINATION,
        PORTAL_PATH,
        SETTINGS_INTERFACE,
    )
}

/// Every setting misty follows, read through `proxy`.
fn read_all(proxy: &Proxy<'_>) -> Appearance {
    let mut appearance = Appearance::default();
    for (namespace, key) in KEYS {
        let value: Option<OwnedValue> = proxy.call(READ_ONE, &(namespace, key)).ok();
        if let Some(setting) =
            value.and_then(|value| Setting::decode(namespace, key, raw_value(&value)))
        {
            setting.apply(&mut appearance);
        }
    }
    appearance
}

/// What `value` holds, looking through the variants a portal may wrap a
/// setting in.
fn raw_value(value: &Value<'_>) -> RawValue {
    match value {
        Value::Bool(on) => RawValue::Bool(*on),
        Value::Structure(structure) => match structure.fields() {
            [Value::F64(r), Value::F64(g), Value::F64(b)] => RawValue::Color(*r, *g, *b),
            _ => RawValue::Other,
        },
        Value::U32(code) => RawValue::Code(*code),
        Value::Value(inner) => raw_value(inner),
        _ => RawValue::Other,
    }
}

/// The color scheme the portal's `code` names; `None` for no preference.
fn color_scheme_of(code: u32) -> Option<ColorScheme> {
    match code {
        PORTAL_DARK => Some(ColorScheme::Dark),
        PORTAL_LIGHT => Some(ColorScheme::Light),
        _ => None,
    }
}

/// The accent color of the portal's channels, each from 0 to 1; `None`
/// when one is outside, as the portal reports "no accent".
fn accent_of(r: f64, g: f64, b: f64) -> Option<Rgb> {
    let channel = |value: f64| {
        (0.0..=PORTAL_CHANNEL_MAX)
            .contains(&value)
            .then(|| (value * RGB_CHANNEL_MAX).round() as u8)
    };
    Some(Rgb::new(channel(r)?, channel(g)?, channel(b)?))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn decoded(namespace: &str, key: &str, value: RawValue) -> Appearance {
        let mut appearance = Appearance::default();
        if let Some(setting) = Setting::decode(namespace, key, value) {
            setting.apply(&mut appearance);
        }
        appearance
    }

    #[test]
    fn test_the_portal_codes_map_to_dark_light_and_no_preference() {
        let scheme = |code| decoded(APPEARANCE_NAMESPACE, COLOR_SCHEME_KEY, RawValue::Code(code));
        assert_eq!(scheme(1).color_scheme, Some(ColorScheme::Dark));
        assert_eq!(scheme(2).color_scheme, Some(ColorScheme::Light));
        assert_eq!(scheme(0).color_scheme, None, "no preference");
        let contrast = |code| decoded(APPEARANCE_NAMESPACE, CONTRAST_KEY, RawValue::Code(code));
        assert!(contrast(1).high_contrast);
        assert!(!contrast(0).high_contrast);
    }

    #[test]
    fn test_an_accent_in_range_becomes_a_color_and_one_outside_means_none() {
        let accent =
            |r, g, b| decoded(APPEARANCE_NAMESPACE, ACCENT_KEY, RawValue::Color(r, g, b)).accent;
        assert_eq!(accent(1.0, 0.5, 0.0), Some(Rgb::new(255, 128, 0)));
        assert_eq!(accent(-1.0, 0.5, 0.0), None, "the portal's unset accent");
        assert_eq!(accent(0.2, 0.2, 1.5), None);
    }

    #[test]
    fn test_animations_off_means_reduced_motion_and_unknown_keys_change_nothing() {
        let animations = |on| decoded(INTERFACE_NAMESPACE, ANIMATIONS_KEY, RawValue::Bool(on));
        assert!(animations(false).reduced_motion);
        assert!(!animations(true).reduced_motion);
        assert_eq!(
            decoded(APPEARANCE_NAMESPACE, "font-name", RawValue::Code(1)),
            Appearance::default()
        );
        assert_eq!(
            decoded(APPEARANCE_NAMESPACE, COLOR_SCHEME_KEY, RawValue::Bool(true)),
            Appearance::default(),
            "a value of the wrong shape"
        );
    }
}
