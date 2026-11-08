//! [`Tray`]: an icon in the system tray with a tooltip and a menu, behind
//! the `tray` feature. A click on the icon and each menu choice come back
//! to [`Application::message`](crate::Application::message) as messages
//! the app makes from a [`TrayEvent`]. An app that lives in the tray also
//! returns `false` from
//! [`Application::exit_when_last_window_closes`](crate::Application::exit_when_last_window_closes).
//!
//! On Linux and the BSDs the icon is a StatusNotifierItem over D-Bus (the
//! `sni` module, through `ksni`), which needs no GTK main loop and runs on
//! the same zbus misty reads the desktop's appearance with. KDE Plasma,
//! Xfce, LXQt, most panels, and GNOME with the AppIndicator extension
//! show it; a desktop with none refuses it. On Windows and macOS the
//! system's own tray holds it (the `native` module, through `tray-icon`).

#[cfg(any(target_os = "windows", target_os = "macos"))]
mod native;
#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
))]
mod sni;

use std::sync::Arc;

use thiserror::Error;

use super::super::render::image::Image;
use super::application::AppContext;

// ========================================================================
// Data Structures
// ========================================================================

/// An icon in the system tray, shown while this value lives. Make it in a
/// callback (such as [`Application::init`](crate::Application::init)),
/// on the main thread as Windows and macOS require, and keep it in the app.
///
/// ```no_run
/// use misty::{AppContext, Image, Tray, TrayEntry, TrayEvent, TrayItem, TrayOptions};
///
/// enum Message {
///     Tray(TrayEvent),
/// }
///
/// fn tray(cx: &AppContext<'_, Message>, icon: &Image) -> Option<Tray> {
///     let menu = vec![
///         TrayEntry::Item(TrayItem::new("show", "Show window")),
///         TrayEntry::Item(TrayItem::new("watch", "Watch the clipboard").with_checked(true)),
///         TrayEntry::Separator,
///         TrayEntry::Item(TrayItem::new("quit", "Quit")),
///     ];
///     let options = TrayOptions::new("org.example.dict", icon)
///         .with_tooltip("Dictionary")
///         .with_menu(menu);
///     Tray::new(cx, options, Message::Tray)
///         .map_err(|error| eprintln!("{error}"))
///         .ok()
/// }
/// ```
#[derive(Debug)]
pub struct Tray {
    /// The system's tray icon.
    backend: Backend,
}

/// What a [`Tray`] shows: start from [`TrayOptions::new`] and add what you
/// need with the `with_` methods.
#[derive(Clone, Debug, PartialEq)]
pub struct TrayOptions {
    /// The icon.
    icon: Image,
    /// The app's name for its icon, steady between runs.
    id: String,
    /// The menu.
    menu: Vec<TrayEntry>,
    /// The text shown when the pointer rests on the icon.
    tooltip: String,
}

/// One entry of a tray menu.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum TrayEntry {
    /// An item to choose.
    Item(TrayItem),
    /// A line between groups of entries.
    Separator,
    /// An item opening a menu of its own.
    Submenu(TraySubmenu),
}

/// A tray menu item to choose.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TrayItem {
    /// Whether the item shows a check mark that is on (`Some(true)`) or
    /// off, or `None` for a plain item.
    pub checked: Option<bool>,
    /// Whether the user may choose the item.
    pub enabled: bool,
    /// The app's name for the item, handed back when it is chosen.
    pub id: String,
    /// The words shown.
    pub label: String,
}

/// A tray menu item opening a menu of its own.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TraySubmenu {
    /// The entries of its menu.
    pub entries: Vec<TrayEntry>,
    /// The words shown.
    pub label: String,
}

/// What the user did with a [`Tray`].
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum TrayEvent {
    /// The user clicked the icon itself (its primary button).
    Activated,
    /// The user chose the menu item with this id. Checking an item does
    /// not change its mark; the app sets the menu again with
    /// [`Tray::set_menu`].
    Chosen(String),
}

/// Why a tray icon could not be shown or changed.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum TrayError {
    /// The system took the icon's pixels wrongly, for this reason.
    #[error("the tray icon could not be made from the image ({0}): use a small square image")]
    Icon(String),
    /// The system has no tray to show the icon in, for this reason.
    #[error(
        "the tray is unavailable ({0}): on Linux it needs a StatusNotifierItem host (KDE \
         Plasma, Xfce, most panels, or GNOME with the AppIndicator extension); make it from \
         an app callback on the main thread"
    )]
    Unavailable(String),
}

/// Hands a backend's events to the app.
type Sender = Arc<dyn Fn(TrayEvent) + Send + Sync>;

/// The system's tray icon.
#[derive(Debug)]
enum Backend {
    /// The system tray of Windows or macOS.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    Native(native::Native),
    /// A StatusNotifierItem over D-Bus.
    #[cfg(all(
        unix,
        not(any(target_os = "macos", target_os = "ios", target_os = "android"))
    ))]
    Sni(sni::Sni),
}

// ========================================================================
// Tray Implementations
// ========================================================================

impl Tray {
    /// Shows the icon `options` describes, each click and menu choice
    /// reaching [`Application::message`](crate::Application::message) as
    /// `message(event)`. Refused when the system has no tray.
    pub fn new<M: Send + 'static>(
        cx: &AppContext<'_, M>,
        options: TrayOptions,
        message: impl Fn(TrayEvent) -> M + Send + Sync + 'static,
    ) -> Result<Self, TrayError> {
        let waker = cx.waker();
        let send: Sender = Arc::new(move |event| {
            // An app that has quit wants no more clicks.
            let _ = waker.send(message(event));
        });
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        let backend = Backend::Native(native::Native::new(options, send)?);
        #[cfg(all(
            unix,
            not(any(target_os = "macos", target_os = "ios", target_os = "android"))
        ))]
        let backend = Backend::Sni(sni::Sni::new(options, send)?);
        Ok(Self { backend })
    }

    /// Shows `icon` in place of the icon.
    pub fn set_icon(&self, icon: &Image) -> Result<(), TrayError> {
        match &self.backend {
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            Backend::Native(native) => native.set_icon(icon),
            #[cfg(all(
                unix,
                not(any(target_os = "macos", target_os = "ios", target_os = "android"))
            ))]
            Backend::Sni(sni) => sni.set_icon(icon),
        }
    }

    /// Shows `menu` in place of the menu, such as with a check mark
    /// changed.
    pub fn set_menu(&self, menu: Vec<TrayEntry>) -> Result<(), TrayError> {
        match &self.backend {
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            Backend::Native(native) => native.set_menu(&menu),
            #[cfg(all(
                unix,
                not(any(target_os = "macos", target_os = "ios", target_os = "android"))
            ))]
            Backend::Sni(sni) => sni.set_menu(menu),
        }
    }

    /// Shows `tooltip` when the pointer rests on the icon.
    pub fn set_tooltip(&self, tooltip: &str) -> Result<(), TrayError> {
        match &self.backend {
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            Backend::Native(native) => native.set_tooltip(tooltip),
            #[cfg(all(
                unix,
                not(any(target_os = "macos", target_os = "ios", target_os = "android"))
            ))]
            Backend::Sni(sni) => sni.set_tooltip(tooltip),
        }
    }
}

// ========================================================================
// TrayOptions Implementations
// ========================================================================

impl TrayOptions {
    /// A tray icon showing `icon`, with no tooltip or menu. `id` names the
    /// app's icon steadily between runs (a reverse domain name such as
    /// `org.example.dict`), for the desktop to remember where it goes.
    pub fn new(id: impl Into<String>, icon: &Image) -> Self {
        Self {
            icon: icon.clone(),
            id: id.into(),
            menu: Vec::new(),
            tooltip: String::new(),
        }
    }

    /// These options with `menu` opening from the icon.
    pub fn with_menu(self, menu: Vec<TrayEntry>) -> Self {
        Self { menu, ..self }
    }

    /// These options with `tooltip` shown when the pointer rests on the
    /// icon.
    pub fn with_tooltip(self, tooltip: impl Into<String>) -> Self {
        Self {
            tooltip: tooltip.into(),
            ..self
        }
    }
}

// ========================================================================
// TrayItem Implementations
// ========================================================================

impl TrayItem {
    /// An enabled item showing `label`, without a check mark, called `id`
    /// by the app.
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            checked: None,
            enabled: true,
            id: id.into(),
            label: label.into(),
        }
    }

    /// This item with a check mark, on when `checked` is.
    pub fn with_checked(self, checked: bool) -> Self {
        Self {
            checked: Some(checked),
            ..self
        }
    }

    /// This item choosable (`true`) or shown greyed out.
    pub fn with_enabled(self, enabled: bool) -> Self {
        Self { enabled, ..self }
    }
}

// ========================================================================
// TraySubmenu Implementations
// ========================================================================

impl TraySubmenu {
    /// An item showing `label` that opens a menu of `entries`.
    pub fn new(label: impl Into<String>, entries: Vec<TrayEntry>) -> Self {
        Self {
            entries,
            label: label.into(),
        }
    }
}
