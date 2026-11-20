//! The user's [`Appearance`] settings where winit cannot read them: on
//! Linux and the BSDs, the desktop portal's (the `portal` module), read
//! once at start and watched for changes. Elsewhere the runtime takes
//! light or dark from winit and reads nothing here.

#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
))]
mod portal;

#[cfg(not(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
)))]
use super::super::base::appearance::Appearance;

#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
))]
pub(crate) use portal::{read, watch};

#[cfg(all(
    feature = "icon-theme",
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
))]
pub(crate) use portal::icon_theme_name;

// ========================================================================
// Functions
// ========================================================================

/// The user's appearance as the system reports it beyond winit: nothing on
/// this platform.
#[cfg(not(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
)))]
pub(crate) fn read() -> Appearance {
    Appearance::default()
}

/// Watches the system's appearance: nothing to watch on this platform,
/// where winit reports light or dark changing.
#[cfg(not(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
)))]
pub(crate) fn watch(_start: Appearance, _changed: impl Fn(Appearance) + Send + 'static) {}
