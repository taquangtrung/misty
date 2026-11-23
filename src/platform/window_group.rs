//! One task bar group for every window of the app. On X11 each window is
//! put in a window group whose leader is a hidden window of the app's own,
//! named after the app's class, as GTK and Qt do. Without one, libwnck
//! (the XFCE panel's window buttons, among others) names the app's group
//! after its windows' titles, and a task bar sorting groups by name moves
//! the app's buttons whenever a title changes. Elsewhere nothing is done.

use winit::window::Window;

// ========================================================================
// Functions
// ========================================================================

/// Puts `window`, not yet shown, in the app's window group. On X11 a
/// failure is logged as a warning and the window is left as winit made it.
pub(crate) fn join(window: &Window) {
    #[cfg(all(
        unix,
        not(any(target_os = "macos", target_os = "ios", target_os = "android"))
    ))]
    x11::join(window);
    #[cfg(not(all(
        unix,
        not(any(target_os = "macos", target_os = "ios", target_os = "android"))
    )))]
    let _ = window;
}

// ========================================================================
// X11
// ========================================================================

#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
))]
mod x11 {
    use std::sync::OnceLock;

    use thiserror::Error;
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use winit::window::Window;
    use x11rb::connection::Connection;
    use x11rb::errors::{ConnectError, ConnectionError, ReplyError, ReplyOrIdError};
    use x11rb::properties::{WmClass, WmHints};
    use x11rb::protocol::xproto::{
        AtomEnum, ConnectionExt, CreateWindowAux, PropMode, WindowClass,
    };
    use x11rb::rust_connection::RustConnection;
    use x11rb::wrapper::ConnectionExt as _;

    // ====================================================================
    // Constants
    // ====================================================================

    /// The ICCCM property naming a window's client leader.
    const CLIENT_LEADER: &[u8] = b"WM_CLIENT_LEADER";
    /// The EWMH property holding a window's name in UTF-8, and its type.
    const NET_WM_NAME: &[u8] = b"_NET_WM_NAME";
    const UTF8_STRING: &[u8] = b"UTF8_STRING";
    /// The leader's name when the window it leads has no class.
    const FALLBACK_NAME: &[u8] = b"misty";

    /// The app's group leader, made for the first window to join, or
    /// `None` when it could not be made (logged once).
    static LEADER: OnceLock<Option<Leader>> = OnceLock::new();

    // ====================================================================
    // Data Structures
    // ====================================================================

    /// The hidden window leading the app's group, on the connection that
    /// made it: the window lives as long as the connection, so both are
    /// kept for the life of the process.
    struct Leader {
        connection: RustConnection,
        id: u32,
    }

    /// Why a window could not be put in the app's group.
    #[derive(Debug, Error)]
    enum GroupError {
        /// The X server could not be reached.
        #[error("cannot reach the X server: {0}")]
        Connect(#[from] ConnectError),
        /// The connection to it broke.
        #[error("the X connection broke: {0}")]
        Connection(#[from] ConnectionError),
        /// It refused a request.
        #[error("the X server refused the request: {0}")]
        Reply(#[from] ReplyError),
        /// It refused a request or ran out of ids.
        #[error("the X server refused the request: {0}")]
        ReplyOrId(#[from] ReplyOrIdError),
    }

    // ====================================================================
    // Functions
    // ====================================================================

    /// Puts the X11 window behind `window` in the app's group; nothing on
    /// Wayland.
    pub(super) fn join(window: &Window) {
        let Some(id) = x11_window(window) else {
            return;
        };
        let leader = LEADER.get_or_init(|| {
            make_leader(id)
                .inspect_err(|error| {
                    log::warn!("the task bar may reorder the app's windows: {error}");
                })
                .ok()
        });
        if let Some(leader) = leader
            && let Err(error) = lead(leader, id)
        {
            log::warn!("the task bar may reorder this window: {error}");
        }
    }

    /// The X11 id of `window`, or `None` on Wayland.
    fn x11_window(window: &Window) -> Option<u32> {
        match window.window_handle().ok()?.as_raw() {
            RawWindowHandle::Xcb(handle) => Some(handle.window.get()),
            RawWindowHandle::Xlib(handle) => u32::try_from(handle.window).ok(),
            _ => None,
        }
    }

    /// Makes the hidden leader, on a connection of its own to the display
    /// winit uses, named and classed as the window `first` is.
    fn make_leader(first: u32) -> Result<Leader, GroupError> {
        let (connection, screen) = x11rb::connect(None)?;
        let root = connection.setup().roots[screen].root;
        let class = WmClass::get(&connection, first)?.reply()?;
        let (instance, name) = match &class {
            Some(class) if !class.class().is_empty() => (class.instance(), class.class()),
            Some(_) | None => (FALLBACK_NAME, FALLBACK_NAME),
        };
        let id = connection.generate_id()?;
        connection
            .create_window(
                0,
                id,
                root,
                0,
                0,
                1,
                1,
                0,
                WindowClass::INPUT_ONLY,
                0,
                &CreateWindowAux::new(),
            )?
            .check()?;
        let net_wm_name = connection.intern_atom(false, NET_WM_NAME)?.reply()?.atom;
        let utf8 = connection.intern_atom(false, UTF8_STRING)?.reply()?.atom;
        let wm_class = [instance, b"\0", name, b"\0"].concat();
        connection
            .change_property8(
                PropMode::REPLACE,
                id,
                AtomEnum::WM_NAME,
                AtomEnum::STRING,
                name,
            )?
            .check()?;
        connection
            .change_property8(PropMode::REPLACE, id, net_wm_name, utf8, name)?
            .check()?;
        connection
            .change_property8(
                PropMode::REPLACE,
                id,
                AtomEnum::WM_CLASS,
                AtomEnum::STRING,
                &wm_class,
            )?
            .check()?;
        let leader = Leader { connection, id };
        // ICCCM has the leader name itself its own leader.
        lead(&leader, id)?;
        Ok(leader)
    }

    /// Names `leader` the group leader and client leader of the window
    /// `id`, keeping the window's other hints.
    fn lead(leader: &Leader, id: u32) -> Result<(), GroupError> {
        let connection = &leader.connection;
        if id != leader.id {
            let mut hints = WmHints::get(connection, id)?.reply()?.unwrap_or_default();
            hints.window_group = Some(leader.id);
            hints.set(connection, id)?.check()?;
        }
        let client_leader = connection.intern_atom(false, CLIENT_LEADER)?.reply()?.atom;
        connection
            .change_property32(
                PropMode::REPLACE,
                id,
                client_leader,
                AtomEnum::WINDOW,
                &[leader.id],
            )?
            .check()?;
        connection.flush()?;
        Ok(())
    }
}
