//! The widgets an app that drives its own window composes in it, each
//! keeping the [`Widget`](crate::Widget) contract, with what they share:
//! list keys, glides, drags, popups, and the command registry; the
//! window's chrome is in [`chrome`](crate::chrome).
//! An app described as a tree of elements builds the same widgets with
//! the functions of [`view`](crate::view) instead.

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

// ========================================================================
// Public API
// ========================================================================

pub use breadcrumb::{Breadcrumb, BreadcrumbProps, BreadcrumbResponse, Crumb};
pub use button::{Button, ButtonProps, ButtonResponse, ButtonVariant};
pub use checkbox::{Checkbox, CheckboxProps, CheckboxResponse};
pub use code_view::{CodeView, CodeViewResponse, GutterMark, Highlight};
pub use color_field::{ColorField, ColorFieldProps, ColorFieldResponse};
pub use command::{
    Command, CommandError, CommandKeys, CommandRegistry, ConflictKind, InvalidKeys, KeyConflict,
    KeyScope, MenuSlot,
};
pub use completion_list::{CompletionList, CompletionListResponse, CompletionRow};
pub use dialog::{Dialog, DialogButton, DialogResponse};
#[cfg(feature = "html")]
pub use document::html::{ClassStyle, HtmlOptions, ImageSource};
pub use document::{
    Block, BlockImage, BlockStyle, Container, CustomBlock, CustomHeight, DataLink, Document,
    Heading, HeightForWidth, HighlightedCode, Inline, Link, ListBlock, TableBlock, TableRow,
};
pub use drag::{
    Drag, DragResponse, Dropped, FileDrop, FileDropEvent, FileDropped, FileHover, draw_drop_target,
};
pub use drop_list::{DropList, DropListResponse, DropRow};
pub use floating_panel::{Corner, FloatingPanel, FloatingPanelResponse};
pub use flow_list::{
    FlowList, FlowListResponse, FlowSource, RowPointer, RowPointerKind, ScrollAlign,
};
pub use grid_scroll::{GridLayout, GridLine, GridScroll};
pub use hex_view::{ByteSource, HexView, HexViewResponse};
#[cfg(feature = "highlight")]
pub use highlight::Highlighter;
pub use hints::{Hints, HintsError, HintsResponse};
pub use icon_grid::{GridData, GridSource, IconGrid, IconGridResponse};
pub use image_view::ImageView;
pub use info_card::InfoCard;
pub use key_capture::{KeyCapture, KeyCaptureProps, KeyCaptureResponse, KeyEdit};
pub use label::{
    draw_keycap, draw_label, draw_styled_label, fit_label, label_width, styled_label_width,
};
pub use list_keys::ListKeys;
pub use list_view::{List, ListResponse, ListSource, RowDrop};
pub use menu::{Menu, MenuEntry, MenuItem, MenuResponse};
pub use names_field::{NamesField, NamesFieldProps, NamesFieldResponse};
pub use number_range::NumberRange;
pub use popup::{Popup, PopupSide, draw_backdrop, draw_popup_header};
pub use progress::{ProgressBar, ProgressProps};
pub use quick_pick::{QueryStyle, QuickPick, QuickPickPlacement, QuickPickTick, command_palette};
pub use radio::{RadioGroup, RadioGroupProps, RadioGroupResponse};
pub use rich_text::{
    ContextPress, Mark, MarkStyle, PageSize, Paged, PlacedBlock, RichText, RichTextPos,
    RichTextResponse, ScrollAmount,
};
pub use row_content::{Badge, RowContent, RowIcon};
pub use scroll_view::{ScrollView, ScrollViewResponse};
pub use select::{Select, SelectProps, SelectResponse};
pub use slider::{NumberProps, Slider, SliderResponse};
pub use stepper::{Stepper, StepperResponse};
pub use table::{
    CellBorder, CellBorders, CellRange, HeaderMenu, SortDirection, SortKey, Table, TableCellStyle,
    TableCells, TableColumn, TableResponse, TableSource,
};
pub use text_area::{TextArea, TextAreaProps, TextAreaResponse};
pub use text_field::{TextField, TextFieldProps, TextFieldResponse};
pub use thumbnails::{Thumbnail, Thumbnails};
pub use tiles::{TileKey, TileSet};
pub use toast::{SeverityMark, Toasts, ToastsResponse};
pub use toolbar::{ToolButton, Toolbar, ToolbarItem, ToolbarResponse};
pub use tooltip::{Tooltip, TooltipPlacement};
pub use transient::{
    SwitchKind, Transient, TransientCommand, TransientResponse, TransientRow, TransientSwitch,
};
pub use tree_view::{TreeDrop, TreeItem, TreeItems, TreeView, TreeViewResponse};
pub use viewport::{Viewport, ViewportResponse};
pub use which_key::{WhichKey, WhichKeyEntry, WhichKeyPlacement, WhichKeyResponse};
