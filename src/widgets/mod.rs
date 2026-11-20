//! The widgets an app composes in a window, each keeping the
//! [`Widget`](crate::Widget) contract, with what they share: list keys,
//! glides, drags, popups, and the command registry.

pub(crate) mod band;
pub(crate) mod breadcrumb;
pub(crate) mod button;
pub(crate) mod checkbox;
pub(crate) mod code_view;
pub(crate) mod color_field;
pub(crate) mod command;
pub(crate) mod completion_list;
pub(crate) mod dialog;
pub(crate) mod document;
pub(crate) mod draft;
pub(crate) mod drag;
pub(crate) mod drop_list;
pub(crate) mod find;
pub(crate) mod floating_panel;
pub(crate) mod flow_list;
pub(crate) mod glide;
pub(crate) mod grid_scroll;
pub(crate) mod hex_view;
#[cfg(feature = "highlight")]
pub(crate) mod highlight;
pub(crate) mod hints;
pub(crate) mod icon_grid;
pub(crate) mod image_view;
pub(crate) mod info_card;
pub(crate) mod key_capture;
pub(crate) mod label;
pub(crate) mod list_keys;
pub(crate) mod list_view;
pub(crate) mod menu;
pub(crate) mod names_field;
pub(crate) mod number_range;
pub mod picker;
pub(crate) mod popup;
pub(crate) mod progress;
pub(crate) mod quick_pick;
pub(crate) mod radio;
pub(crate) mod rich_text;
pub(crate) mod row_content;
pub(crate) mod scroll_view;
pub(crate) mod select;
pub(crate) mod slider;
pub(crate) mod stepper;
pub(crate) mod table;
pub(crate) mod text_area;
pub(crate) mod text_field;
pub(crate) mod thumbnails;
pub(crate) mod tiles;
pub(crate) mod toast;
pub(crate) mod toolbar;
pub(crate) mod tooltip;
pub(crate) mod transient;
pub(crate) mod tree_view;
pub(crate) mod viewport;
pub(crate) mod which_key;

// ========================================================================
// Constants
// ========================================================================

/// The thickness of the accent ring around a focused control (a button,
/// a checkbox, a slider), in logical pixels, so every control shows the
/// focus alike.
pub(crate) const FOCUS_STROKE: f32 = 2.0;
