//! The tray icon on Linux and the BSDs: a StatusNotifierItem served over
//! D-Bus by `ksni` on a thread of its own, with no GTK main loop.

use std::fmt::{self, Debug, Formatter};

use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::{CheckmarkItem, StandardItem, SubMenu};
use ksni::{Icon, MenuItem, ToolTip};

use super::super::super::render::image::{BYTES_PER_PIXEL, Image};
use super::{Sender, TrayEntry, TrayError, TrayEvent, TrayOptions};

// ========================================================================
// Constants
// ========================================================================

/// What marks the next character of a StatusNotifierItem menu label as
/// its access key; doubled, it is shown as itself.
const MNEMONIC: &str = "_";

/// A literal underscore in a StatusNotifierItem menu label.
const ESCAPED_MNEMONIC: &str = "__";

/// Why a tray whose service stopped cannot change.
const SERVICE_STOPPED: &str = "the tray's D-Bus service stopped";

// ========================================================================
// Data Structures
// ========================================================================

/// A StatusNotifierItem being served.
pub(super) struct Sni {
    /// The service, to change what it shows.
    handle: Handle<Item>,
}

/// What the StatusNotifierItem shows, and where its events go.
struct Item {
    /// The icon, as ARGB pixels.
    icon: Icon,
    /// The app's name for its icon.
    id: String,
    /// The menu.
    menu: Vec<TrayEntry>,
    /// Hands events to the app.
    send: Sender,
    /// The text shown when the pointer rests on the icon.
    tooltip: String,
}

// ========================================================================
// Sni Implementations
// ========================================================================

impl Sni {
    /// Serves the icon `options` describes, each event going to `send`.
    pub(super) fn new(options: TrayOptions, send: Sender) -> Result<Self, TrayError> {
        let item = Item {
            icon: icon_of(&options.icon),
            id: options.id,
            menu: options.menu,
            send,
            tooltip: options.tooltip,
        };
        let handle = item
            .spawn()
            .map_err(|error| TrayError::Unavailable(error.to_string()))?;
        Ok(Self { handle })
    }

    /// Shows `icon` in place of the icon.
    pub(super) fn set_icon(&self, icon: &Image) -> Result<(), TrayError> {
        let icon = icon_of(icon);
        self.update(|item| item.icon = icon)
    }

    /// Shows `menu` in place of the menu.
    pub(super) fn set_menu(&self, menu: Vec<TrayEntry>) -> Result<(), TrayError> {
        self.update(|item| item.menu = menu)
    }

    /// Shows `tooltip` when the pointer rests on the icon.
    pub(super) fn set_tooltip(&self, tooltip: &str) -> Result<(), TrayError> {
        let tooltip = tooltip.to_owned();
        self.update(|item| item.tooltip = tooltip)
    }

    /// Changes what the service shows with `change`.
    fn update(&self, change: impl FnOnce(&mut Item)) -> Result<(), TrayError> {
        self.handle
            .update(change)
            .ok_or_else(|| TrayError::Unavailable(SERVICE_STOPPED.to_owned()))
    }
}

impl Drop for Sni {
    fn drop(&mut self) {
        // The service ends on its own thread; nothing waits for it.
        let _ = self.handle.shutdown();
    }
}

impl Debug for Sni {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sni").finish_non_exhaustive()
    }
}

// ========================================================================
// Item Implementations
// ========================================================================

impl ksni::Tray for Item {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn title(&self) -> String {
        self.tooltip.clone()
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        (self.send)(TrayEvent::Activated);
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        vec![self.icon.clone()]
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: self.tooltip.clone(),
            ..ToolTip::default()
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        menu_items(&self.menu)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// `entries` as `ksni` menu items, each choice handed to the item's
/// sender.
fn menu_items(entries: &[TrayEntry]) -> Vec<MenuItem<Item>> {
    entries
        .iter()
        .map(|entry| match entry {
            TrayEntry::Item(tray_item) => {
                let id = tray_item.id.clone();
                let activate = Box::new(move |item: &mut Item| {
                    (item.send)(TrayEvent::Chosen(id.clone()));
                });
                let label = escaped(&tray_item.label);
                match tray_item.checked {
                    Some(checked) => CheckmarkItem {
                        activate,
                        checked,
                        enabled: tray_item.enabled,
                        label,
                        ..CheckmarkItem::default()
                    }
                    .into(),
                    None => StandardItem {
                        activate,
                        enabled: tray_item.enabled,
                        label,
                        ..StandardItem::default()
                    }
                    .into(),
                }
            }
            TrayEntry::Separator => MenuItem::Separator,
            TrayEntry::Submenu(submenu) => SubMenu {
                label: escaped(&submenu.label),
                submenu: menu_items(&submenu.entries),
                ..SubMenu::default()
            }
            .into(),
        })
        .collect()
}

/// `label` with its underscores shown as themselves, not taken as access
/// keys.
fn escaped(label: &str) -> String {
    label.replace(MNEMONIC, ESCAPED_MNEMONIC)
}

/// `image` as a StatusNotifierItem icon: ARGB, each pixel's bytes in
/// network order.
fn icon_of(image: &Image) -> Icon {
    let (pixels, _) = image.pixels().as_chunks::<BYTES_PER_PIXEL>();
    let data = pixels
        .iter()
        .flat_map(|&[r, g, b, a]| [a, r, g, b])
        .collect();
    Icon {
        data,
        height: i32::try_from(image.height()).unwrap_or(i32::MAX),
        width: i32::try_from(image.width()).unwrap_or(i32::MAX),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::super::{TrayItem, TraySubmenu};
    use super::*;

    #[test]
    fn test_icons_go_out_as_argb_and_labels_keep_their_underscores() {
        let image = Image::from_rgba(1, 1, vec![10, 20, 30, 40]).expect("1 by 1");
        let icon = icon_of(&image);
        assert_eq!(icon.data, [40, 10, 20, 30]);
        assert_eq!((icon.width, icon.height), (1, 1));
        assert_eq!(escaped("snake_case_name"), "snake__case__name");
    }

    #[test]
    fn test_choosing_an_item_in_a_submenu_reports_its_id() {
        let got = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&got);
        let mut item = Item {
            icon: icon_of(&Image::from_rgba(1, 1, vec![0; 4]).expect("1 by 1")),
            id: "t".to_owned(),
            menu: Vec::new(),
            send: Arc::new(move |event| sink.lock().expect("unpoisoned").push(event)),
            tooltip: String::new(),
        };
        let nested = TraySubmenu::new(
            "More",
            vec![TrayEntry::Item(
                TrayItem::new("deep", "Deep").with_checked(false),
            )],
        );
        let menu = menu_items(&[TrayEntry::Separator, TrayEntry::Submenu(nested)]);
        let [MenuItem::Separator, MenuItem::SubMenu(submenu)] = menu.as_slice() else {
            panic!("a separator then a submenu");
        };
        let [MenuItem::Checkmark(deep)] = submenu.submenu.as_slice() else {
            panic!("one check item");
        };
        assert!(!deep.checked);
        (deep.activate)(&mut item);
        ksni::Tray::activate(&mut item, 0, 0);
        assert_eq!(
            *got.lock().expect("unpoisoned"),
            [TrayEvent::Chosen("deep".to_owned()), TrayEvent::Activated]
        );
    }
}
