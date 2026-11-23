//! Whether the desktop composites windows, so what a window leaves
//! see-through shows what is behind it rather than black. Wayland,
//! Windows, and macOS always do. On X11 a compositing manager is optional:
//! one runs while it owns the EWMH `_NET_WM_CM_S<screen>` selection.

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
use x11rb::errors::{ConnectError, ConnectionError, ReplyError};
#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
))]
use x11rb::protocol::xproto::ConnectionExt;

#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
))]
use super::focus::x11_window;

// ========================================================================
// Constants
// ========================================================================

/// The start of the name of the selection a compositing manager owns on
/// each screen, the screen's number following it.
#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
))]
const COMPOSITOR_SELECTION: &str = "_NET_WM_CM_S";

// ========================================================================
// Data Structures
// ========================================================================

/// Why the X server could not be asked whether a compositor runs.
#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
))]
#[derive(Debug, Error)]
enum AskError {
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

/// Whether the desktop `window` is on composites it with what is behind
/// it: `false` only on X11 with no compositing manager running as the
/// window opens. When the X server cannot be asked, the failure is logged
/// and the desktop is taken to composite.
pub(crate) fn composites(window: &Window) -> bool {
    #[cfg(all(
        unix,
        not(any(target_os = "macos", target_os = "ios", target_os = "android"))
    ))]
    if x11_window(window).is_some() {
        return x11_compositor_runs().unwrap_or_else(|error| {
            log::warn!("cannot tell whether a compositor runs: {error}");
            true
        });
    }
    #[cfg(not(all(
        unix,
        not(any(target_os = "macos", target_os = "ios", target_os = "android"))
    )))]
    let _ = window;
    true
}

/// Whether a compositing manager owns the default screen's compositor
/// selection, over a connection of its own to the display winit uses.
#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
))]
fn x11_compositor_runs() -> Result<bool, AskError> {
    let (connection, screen) = x11rb::connect(None)?;
    let name = format!("{COMPOSITOR_SELECTION}{screen}");
    let atom = connection
        .intern_atom(false, name.as_bytes())?
        .reply()?
        .atom;
    let owner = connection.get_selection_owner(atom)?.reply()?.owner;
    Ok(owner != x11rb::NONE)
}
