//! The tray icon on Windows and macOS, through `tray-icon`. The crate
//! reports clicks and menu choices to one handler each for the whole
//! process, set once; they go to the sender of the tray shown now.

use std::fmt::{self, Debug, Formatter};
use std::sync::{Mutex, Once, PoisonError};

use tray_icon::menu::{
    CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu,
};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use super::super::super::render::image::Image;
use super::{Sender, TrayEntry, TrayError, TrayEvent, TrayOptions};

// ========================================================================
// Constants
// ========================================================================

/// The sender of the tray shown now, or `None`.
static ROUTE: Mutex<Option<Sender>> = Mutex::new(None);

/// Sets the crate's handlers, which may be set only once a process.
static HANDLERS: Once = Once::new();

// ========================================================================
// Data Structures
// ========================================================================

/// A tray icon the system shows.
pub(super) struct Native {
    /// The icon.
    icon: TrayIcon,
}

// ========================================================================
// Native Implementations
// ========================================================================

impl Native {
    /// Shows the icon `options` describes, each event going to `send`.
    pub(super) fn new(options: TrayOptions, send: Sender) -> Result<Self, TrayError> {
        let icon = TrayIconBuilder::new()
            .with_id(options.id)
            .with_icon(icon_of(&options.icon)?)
            .with_tooltip(options.tooltip)
            .with_menu(Box::new(menu_of(&options.menu)?))
            .with_menu_on_left_click(false)
            .build()
            .map_err(unavailable)?;
        HANDLERS.call_once(|| {
            MenuEvent::set_event_handler(Some(|event: MenuEvent| {
                deliver(TrayEvent::Chosen(event.id.0));
            }));
            TrayIconEvent::set_event_handler(Some(|event: TrayIconEvent| {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    deliver(TrayEvent::Activated);
                }
            }));
        });
        *ROUTE.lock().unwrap_or_else(PoisonError::into_inner) = Some(send);
        Ok(Self { icon })
    }

    /// Shows `icon` in place of the icon.
    pub(super) fn set_icon(&self, icon: &Image) -> Result<(), TrayError> {
        self.icon
            .set_icon(Some(icon_of(icon)?))
            .map_err(unavailable)
    }

    /// Shows `menu` in place of the menu.
    pub(super) fn set_menu(&self, menu: &[TrayEntry]) -> Result<(), TrayError> {
        self.icon.set_menu(Some(Box::new(menu_of(menu)?)));
        Ok(())
    }

    /// Shows `tooltip` when the pointer rests on the icon.
    pub(super) fn set_tooltip(&self, tooltip: &str) -> Result<(), TrayError> {
        self.icon.set_tooltip(Some(tooltip)).map_err(unavailable)
    }
}

impl Drop for Native {
    fn drop(&mut self) {
        *ROUTE.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }
}

impl Debug for Native {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Native").finish_non_exhaustive()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Hands `event` to the tray shown now.
fn deliver(event: TrayEvent) {
    let route = ROUTE.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(send) = route.as_ref() {
        send(event);
    }
}

/// `image` as the crate's icon.
fn icon_of(image: &Image) -> Result<Icon, TrayError> {
    Icon::from_rgba(image.pixels().to_vec(), image.width(), image.height())
        .map_err(|error| TrayError::Icon(error.to_string()))
}

/// `entries` as the crate's menu.
fn menu_of(entries: &[TrayEntry]) -> Result<Menu, TrayError> {
    let menu = Menu::new();
    append_all(entries, &|item| menu.append(item))?;
    Ok(menu)
}

/// Appends `entries` with `append`, to a menu or a submenu.
fn append_all(
    entries: &[TrayEntry],
    append: &dyn Fn(&dyn IsMenuItem) -> tray_icon::menu::Result<()>,
) -> Result<(), TrayError> {
    for entry in entries {
        match entry {
            TrayEntry::Item(item) => match item.checked {
                Some(checked) => append(&CheckMenuItem::with_id(
                    item.id.as_str(),
                    &item.label,
                    item.enabled,
                    checked,
                    None,
                )),
                None => append(&MenuItem::with_id(
                    item.id.as_str(),
                    &item.label,
                    item.enabled,
                    None,
                )),
            },
            TrayEntry::Separator => append(&PredefinedMenuItem::separator()),
            TrayEntry::Submenu(submenu) => {
                let nested = Submenu::new(&submenu.label, true);
                append_all(&submenu.entries, &|item| nested.append(item))?;
                append(&nested)
            }
        }
        .map_err(unavailable)?;
    }
    Ok(())
}

/// `error` from the crate as the reason the tray is unavailable.
fn unavailable(error: impl ToString) -> TrayError {
    TrayError::Unavailable(error.to_string())
}
