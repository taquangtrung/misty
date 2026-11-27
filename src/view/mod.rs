//! Windows described as trees of elements: an [`App`](crate::App) returns
//! its window's tree from [`App::view`](crate::App::view), built from the
//! free functions here, and misty lays it out, draws it, describes it to
//! screen readers, routes the user's input through it, and runs the
//! callbacks its controls were given.
//!
//! ```
//! use misty::view::{checkbox, column, field, form, slider, text};
//! use misty::{App, Element, NumberRange, Theme, WindowId};
//!
//! const VOLUME: NumberRange = NumberRange::new(0.0, 100.0, 5.0);
//!
//! struct Mixer {
//!     muted: bool,
//!     theme: Theme,
//!     volume: f64,
//! }
//!
//! impl App for Mixer {
//!     fn view(&self, _window: WindowId) -> Element<'_, Self> {
//!         column([
//!             form([
//!                 field("Volume", slider(VOLUME, self.volume)
//!                     .on_change(|app: &mut Self, volume| app.volume = volume)),
//!                 field("", checkbox("Mute", self.muted)
//!                     .on_change(|app: &mut Self, muted| app.muted = muted)),
//!             ])
//!             .scroll(),
//!             text("Press Tab to move between the controls.").muted().into(),
//!         ])
//!     }
//!
//!     fn theme(&self) -> &Theme {
//!         &self.theme
//!     }
//! }
//! ```
//!
//! - **Containers:** [`column()`] and [`row`] lay elements out top to bottom
//!   and left to right; [`form`] lays [`field`]s out beside a label column,
//!   each label also naming its control for screen readers;
//!   [`Element::scroll`](crate::Element::scroll) makes a container scroll.
//! - **Controls:** [`button`] (with an icon, or [`icon_button`] for an
//!   icon alone), [`checkbox`], [`radio`], [`select`], [`slider`],
//!   [`stepper`], [`progress`], [`text_field`], [`text_area`],
//!   [`breadcrumb`] (where a view is, each crumb going there), and
//!   [`menu`], a button opening a menu (`on_choose`). Each is
//!   controlled: its value comes from the app's state, and it changes only
//!   through the closure its `on_change` (or `on_click`) was given, which
//!   misty runs on the app's state once the tree is dropped. Callbacks are
//!   `'static`, since they run after the tree that borrows the app is
//!   gone; one that needs a value from `view` moves an owned copy in.
//! - **The app's data:** [`list`], [`table`], [`tree_view`], and
//!   [`icon_grid`] show rows the app lends for one rebuild, never copies;
//!   each takes the height its column leaves. Their cursor and selection
//!   are the app's, changed through `on_cursor_move` and `on_select`,
//!   while misty keeps the scroll, the keys typed toward a motion, drags,
//!   and a tree's expanded nodes. [`cells`] draws the app's grid of cells
//!   with a blinking cursor.
//! - **Bars:** [`title_bar`] draws a frameless window's bar (its
//!   buttons, drag region, and resize border, and a menu button and
//!   menubar asking the app for its menus), [`tab_strip`] the app's
//!   [`tab`]s, [`split`] two elements beside a divider the user drags,
//!   and [`status_bar`] the app's status line. The tabs, their order, the
//!   active tab, and a split's ratio are the app's, changed through
//!   callbacks (`on_activate`, `on_move`, `on_resize`).
//! - **Text:** [`text`] draws a line in one or more colors.
//! - **Layers:** [`Element::overlay`](crate::Element::overlay) draws a
//!   layer over the window while the element shows: a [`dialog`], the
//!   [`toasts`] of the app's notifier, a [`quick_pick`] filtering rows as
//!   the user types, a [`context_menu`] open where the user asked for one
//!   ([`Element::on_context_menu`](crate::Element::on_context_menu), a
//!   right click or Shift+F10, reports where), the [`which_key`] hints of
//!   a prefix the app's keymap waits on, or elements made with
//!   [`overlay()`].
//!   A modal layer, like a select's open list or an open menu, keeps the
//!   keyboard until it goes, then gives the focus back.
//!   [`Element::tooltip`](crate::Element::tooltip) shows help under an
//!   element the pointer rests on.
//! - **Focus:** every control takes the keyboard in reading order (Tab and
//!   Shift+Tab move along it), a click focuses one, the focus ring follows
//!   the keyboard, and a scrolling container scrolls the control it
//!   focuses into view. Keys a control leaves go to the scrolling
//!   container around it, then to [`App::key`](crate::App::key).
//! - **State:** a control's own state (a caret, a scroll offset) is kept
//!   by misty under the element's id, its place in the tree unless
//!   [`Element::id`](crate::Element::id) names it, and dropped when the id
//!   is no longer built.

pub(crate) mod app;
pub(crate) mod bars;
pub(crate) mod breadcrumb;
pub(crate) mod chart;
pub(crate) mod controls;
pub(crate) mod custom;
pub(crate) mod data;
pub(crate) mod element;
pub(crate) mod embed;
pub(crate) mod fields;
pub(crate) mod host;
pub(crate) mod lazy;
pub(crate) mod node;
pub(crate) mod page;
pub(crate) mod pick;
pub(crate) mod popups;
pub(crate) mod props;
pub(crate) mod region;
pub(crate) mod settings;
pub(crate) mod store;

pub use bars::{
    DraggedTab, Split, StatusBar, Tab, TabStrip, TitleBar, split, status_bar, tab, tab_strip,
    title_bar,
};
pub use breadcrumb::{Breadcrumb, breadcrumb};
pub use chart::{meter, sparkline};
pub use controls::{
    Button, Checkbox, Number, Progress, Radio, Select, TextArea, TextField, button, checkbox,
    icon_button, progress, radio, select, select_owned, slider, stepper, text_area, text_field,
};
pub use custom::{Canvas, Custom, canvas, custom};
pub use data::{Cells, Grid, List, Table, Tree, cells, icon_grid, list, table, tree_view};
pub use element::{Field, MenuRequest, Overlay, Text, column, field, form, overlay, row, text};
pub use embed::{HostedView, ScrollStep, ViewHost};
pub use fields::{ColorField, KeyCapture, NamesField, color_field, key_capture, names_field};
pub use lazy::{LazyColumn, LazySource, lazy_column};
pub use page::{
    FilledPage, Item, Page, ScrollingPage, Section, SectionHeader, badge, divider, grid, item,
    keycap, page, proportional_keycap, section, section_header, sidebar, untitled_page,
    vertical_divider,
};
pub use pick::{QuickPick, quick_pick};
pub use popups::{
    ContextMenu, Dialog, Menu, Toasts, WhichKey, context_menu, dialog, menu, toasts, which_key,
};
pub use props::{Corners, ElementSize, Extent, PointerDrag, Press, Space};
pub use settings::{
    KeyChange, KeybindingsCard, SettingChange, SettingEdit, SettingRow, SettingScope, SettingsCard,
    SettingsPage, keybindings_section, setting_row, settings_page, settings_section,
};
