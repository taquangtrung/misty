//! Opening a window without the keyboard: what the window system needs
//! told beyond winit's own request, which covers Windows and macOS. On
//! X11 the window is marked, before it is first shown, with the EWMH hint
//! that a window wants no focus when mapped (`_NET_WM_USER_TIME` 0);
//! Wayland lets no client ask, so nothing is done there.

use winit::window::Window;

#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
))]
use thiserror::Error;
#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
))]
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
))]
use x11rb::connection::Connection;
#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
))]
use x11rb::errors::{ConnectError, ConnectionError, ReplyError};
#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
))]
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, PropMode};

// ========================================================================
// Constants
// ========================================================================

/// The EWMH property a window manager reads, when a window is first
/// mapped, to decide whether to focus it: 0 asks it not to.
#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
))]
const USER_TIME: &[u8] = b"_NET_WM_USER_TIME";

/// The bits of each value of a `CARDINAL` property.
#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
))]
const CARDINAL_FORMAT: u8 = 32;

// ========================================================================
// Data Structures
// ========================================================================

/// Why an X11 window could not be marked to open without the keyboard.
#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
))]
#[derive(Debug, Error)]
enum MarkError {
    /// The X server could not be reached.
    #[error("cannot reach the X server: {0}")]
    Connect(#[from] ConnectError),
    /// The connection to it broke.
    #[error("the X connection broke: {0}")]
    Connection(#[from] ConnectionError),
    /// It refused a request.
    #[error("the X server refused the request: {0}")]
    Reply(#[from] ReplyError),
}

// ========================================================================
// Functions
// ========================================================================

/// Asks the window system not to give `window`, not yet shown, the
/// keyboard when it first shows. On X11 a failure is logged as a warning
/// and the window opens as the window manager decides.
pub(crate) fn decline_first_focus(window: &Window) {
    #[cfg(all(
        unix,
        not(any(target_os = "macos", target_os = "ios", target_os = "android"))
    ))]
    if let Some(id) = x11_window(window)
        && let Err(error) = mark_without_focus(id)
    {
        log::warn!("the window may take the keyboard as it opens: {error}");
    }
    #[cfg(not(all(
        unix,
        not(any(target_os = "macos", target_os = "ios", target_os = "android"))
    )))]
    let _ = window;
}

/// The X11 id of `window`, or `None` on Wayland.
#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
))]
pub(crate) fn x11_window(window: &Window) -> Option<u32> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Xcb(handle) => Some(handle.window.get()),
        RawWindowHandle::Xlib(handle) => u32::try_from(handle.window).ok(),
        _ => None,
    }
}

/// Sets `_NET_WM_USER_TIME` to 0 on the X11 window `id`, over a
/// connection of its own to the display winit uses.
#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
))]
fn mark_without_focus(id: u32) -> Result<(), MarkError> {
    let (connection, _) = x11rb::connect(None)?;
    let atom = connection.intern_atom(false, USER_TIME)?.reply()?.atom;
    // One 32-bit value, in the byte order of this machine, as the
    // protocol carries 32-bit property data.
    let zero = 0u32.to_ne_bytes();
    connection
        .change_property(
            PropMode::REPLACE,
            id,
            atom,
            AtomEnum::CARDINAL,
            CARDINAL_FORMAT,
            1,
            &zero,
        )?
        .check()?;
    connection.flush()?;
    Ok(())
}
