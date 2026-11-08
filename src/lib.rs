//! A GPU-rendered, keyboard-first GUI framework for desktop apps: the window
//! runtime, custom chrome, widgets, and themes.
//!
//! An app implements [`Application`] and hands it to [`run`] with the
//! [`WindowOptions`] of its first window; [`AppContext::open_window`]
//! opens more, all sharing one GPU and one set of fonts. A window opens
//! where its [`EventCx::geometry`] said it was when it closed, with
//! [`WindowOptions::with_geometry`], and an app follows the user's light or
//! dark, accent, and motion settings through [`AppContext::appearance`]
//! and [`Input::AppearanceChanged`]. A frameless window draws its own chrome
//! with a [`TitleBar`], a [`TabStrip`] shows the app's tabs, a
//! [`SplitView`] its panes, and a [`StatusBar`] the app's state and
//! passing messages. A [`List`] shows rows the app supplies, and a
//! [`Table`] shows them in sortable, resizable columns and a [`TreeView`]
//! as a tree whose children the app loads lazily; an [`IconGrid`] shows
//! items as thumbnails a [`Thumbnails`] pool loads in the background. Their
//! rows show plain text, or a [`RowContent`] with a [`RowIcon`], styled
//! text, and [`Badge`]s; Shift+click, Shift with the arrows, and a rubber
//! band select ranges, and `row_rect`, `cell_rect`, and `item_rect` tell
//! where a row was drawn, to place an inline rename field. A [`FlowList`]
//! shows rows of different heights ([`FlowSource`]), scrolled by pixels,
//! such as a reader's paragraphs. [`Hints`] puts Vimium-style letter
//! labels over any targets, to pick one from the keyboard. A
//! [`Menu`] pops up commands in a [`Popup`]; a [`TextField`] takes a line
//! of text (a rename box, an address bar) and a [`TextArea`] several,
//! wrapped to its width; a [`Select`] drops down a list
//! of options to choose one; a [`QuickPick`] draws a session of the
//! [`picker`] framework, its rows filtered as the user types; a [`Tooltip`] and [`Toasts`] show help and messages,
//! a [`WhichKey`] popup the keys that may follow a pending prefix, a
//! [`CompletionList`] the matches an editor offers as the user types, and
//! an [`InfoCard`] what a word is, over it. A [`Transient`] offers
//! commands with the switches that change them, as magit does, and a
//! [`DropList`] drops a list such as an outline over a view's cells.
//! A [`ScrollView`] scrolls content larger than its area behind thin
//! scrollbars, and a [`ProgressBar`] shows how far a long task has come.
//! Text is drawn in styled runs measured in cells with
//! [`Frame::draw_text`], prose in the proportional font as wrapped rich
//! [`Paragraph`]s with [`Frame::draw_paragraph`], and pictures with [`Frame::draw_image`] and
//! [`Frame::draw_icon`] from an [`IconSet`]. A paragraph's spans take sizes, families
//! (including fonts the app ships, loaded with [`load_font`]), exact colors, backgrounds,
//! and strikethroughs, and its [`ParagraphLayout`] maps points to characters and characters
//! to carets and selections. An app with a renderer of its own (Vello, a video decoder, a 3D
//! view) renders into a [`GpuTexture`] on misty's [`Gpu`], from [`AppContext::gpu`], and
//! draws it with [`Frame::draw_texture`]; misty re-exports the [`wgpu`] it is built on.
//! A [`RichText`] shows paragraphs as a read-only, scrolling,
//! selectable document with links and find.
//! Charts, diagrams, and graph edges are [`VectorPath`]s, built with a
//! [`VectorPathBuilder`] or a shape helper and drawn antialiased with
//! [`Frame::fill_path`] and [`Frame::stroke_path`] (a [`Stroke`] gives the
//! width, [`LineJoin`], [`LineCap`], and dashes) in a [`Paint`]: a theme
//! color or a [`LinearGradient`] of [`GradientStop`]s.
//! [`Frame::draw_arrow`] ends a line in an arrowhead, and
//! [`Frame::with_transform`] moves, scales, and rotates paths by a
//! [`Transform`].
//!
//! Every widget keeps one contract, [`Widget`]: [`Widget::handle`] takes
//! an [`Input`] and answers with a response typed for the widget, and
//! [`Widget::draw`] draws it into an area. Shared needs (the clipboard,
//! the pointer, redraw requests) reach widgets through an [`EventCx`], and
//! a widget showing the app's data (a list's rows, an editor) is [`Lent`]
//! it for one call: `list.lend(&rows).draw(frame, area, theme)`.
//!
//! A [`FocusTree`] says which of the widgets composed in a window has the
//! keyboard and which is under the pointer, and
//! [`FocusTree::dispatch`] hands each input to the widget it is for: keys
//! to the focused widget, bubbled to its containers; pointer input to the
//! topmost widget under the pointer (widgets register in paint order, and
//! open focus traps for dialogs and popups sit above the rest), falling
//! through to those below until one takes it; plus tab order and a focus
//! ring.
//! The same frame describes the window to screen readers through
//! AccessKit (see [`AccessAction`]): widgets add themselves while drawing,
//! and a reader's clicks and focus requests come back as [`AccessAction`]s.
//!
//! A [`Button`] runs an action or, as a toggle, stays pressed in; a
//! [`Toolbar`] sets buttons in a row (bound to commands, in toggle groups,
//! the ones that do not fit in a menu); and a [`Breadcrumb`] shows where
//! the user is as crumbs to click, or as an address to type. Menus hold
//! checked and radio items and nested submenus.
//! A [`Checkbox`] turns an option on or off, drawn as a box or a switch,
//! and a [`RadioGroup`] picks one of a few options.
//! A [`Dialog`] asks a question with buttons, laid out by the [`layout`]
//! engine. A [`Slider`] picks a number along a track and a [`Stepper`]
//! steps one with minus and plus buttons, both inside a [`NumberRange`].
//! A [`SettingsPage`] edits an app's settings and keybindings from the
//! keyboard. A [`SystemClipboard`] carries text to and from other apps.
//!
//! Every command lives in one [`CommandRegistry`], which feeds the keymap,
//! the menus, the [`command_palette`], and the keybinding editor. What the
//! user does reaches the app and its widgets as misty [`Input`]s, read once
//! from the window system by the runtime: pointer moves, presses, and wheel
//! turns with the pointer's position and the modifiers held, and keys as
//! misty [`Key`]s already normalized ([`parse_key`] and `Display` read and
//! write the key notation); [`key_label`] writes keys for people.
//!
//! Misty owns its keys and resolves none: which keymap library turns keys
//! into commands is the app's, and so is any editor it embeds. The `kdl`
//! feature reads theme files in KDL, and the `dialogs`
//! feature adds `FileDialog` for the system's own
//! dialogs to open files, pick a folder, or choose where to save. For apps
//! that live beside others, the `global-hotkey` feature adds
//! `GlobalHotkeys` (keys that reach the app from anywhere), `tray` adds a
//! `Tray` icon with a menu, `notify` adds desktop `Notification`s, and
//! `single-instance` adds `SingleInstance`, which hands a second launch's
//! arguments to the running copy; each answers through the app's
//! messages. [`WindowOptions`] opens popup windows (always on top, off the
//! task bar, transparent, placed where the window system allows), and
//! [`AnimatedImage`] steps through animated GIF and WebP frames. The `testing`
//! feature adds `misty::testing`, a headless harness that feeds widgets
//! inputs and reads back what they asked for, drew, and described to
//! screen readers.
//!
//! Every item is reachable at the crate root, `misty::Theme`, except the
//! few that live in a namespace of their own: [`picker`], [`layout`], and
//! `testing`. Misty re-exports two of its dependencies whole, [`wgpu`] for
//! apps that render with the GPU themselves and [`accesskit`] for the nodes
//! apps describe to screen readers; a new major version of either is a
//! breaking change of misty, released as one.

mod app;
mod base;
#[cfg(test)]
mod bench;
mod chrome;
mod input;
mod platform;
mod render;
mod runtime;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
mod widgets;

pub use accesskit;
pub use app::access::AccessAction;
pub use app::application::{AppContext, Application};
pub use app::context::EventCx;
#[cfg(feature = "dialogs")]
pub use app::file_dialog::FileDialog;
pub use app::focus::{FocusId, FocusTree, Route};
pub use app::frame::Frame;
#[cfg(feature = "global-hotkey")]
pub use app::hotkey::{GlobalHotkeys, Hotkey, HotkeyError, HotkeyEvent};
pub use app::ime::{ImeFocus, Preedit, PreeditChange};
#[cfg(feature = "notify")]
pub use app::notify::{Notification, NotificationEvent, NotifyError, Urgency};
#[cfg(feature = "single-instance")]
pub use app::single_instance::{Claim, SingleInstance, SingleInstanceError};
#[cfg(feature = "tray")]
pub use app::tray::{Tray, TrayEntry, TrayError, TrayEvent, TrayItem, TrayOptions, TraySubmenu};
pub use app::widget::{Lent, Outcome, Widget};
pub use app::window::{
    Decorations, GeometryError, OptionAsAlt, Position, ShowPolicy, Size, WindowGeometry,
    WindowLevel, WindowOptions,
};
pub use base::appearance::{Appearance, ColorScheme};
pub use base::box_style::{Border, BoxStyle, Shadow};
pub use base::cell::{
    Attributes, Cell, CellBuffer, CellColor, CellHighlight, CellSource, CellStyle, CellWidth,
    Cursor, CursorShape, UnderlineStyle,
};
pub use base::color::{HexColorError, Rgb, Rgba};
pub use base::cursor::CursorIcon;
pub use base::fuzzy::{
    FuzzyMatch, highlight as fuzzy_highlight, ranges as fuzzy_ranges, score as fuzzy_score,
};
pub use base::geometry::{Point, Rect};
pub use base::key::{Key, KeyCode, KeyParseError, Mods, key_label, parse_key, parse_key_sequence};
pub use base::list::ListState;
pub use base::motion::{Animator, Blink, Easing, HoverFade, Lerp, Motion, Repeat, Tween};
pub use base::notice::{
    LoggedNotice, Notice, NoticeAction, NoticeLog, Notifier, Progress, ProgressId, Routing,
    Severity, Task, Toast, ToastId, ToastStack,
};
pub use base::paint::{GradientStop, LineCap, LineJoin, LinearGradient, Paint, Stroke};
pub use base::palette::{ANSI_COLORS, PALETTE_SIZE, Palette};
pub use base::path::{FillRule, VectorPath, VectorPathBuilder};
pub use base::scroll::SmoothScroll;
pub use base::split::{
    Axis, Direction, Divider, PaneId, PaneRect, SplitId, SplitLayout, SplitTree,
};
pub use base::strings::Strings;
pub use base::styled::{FontFamily, Span, StyledText, TextColor, TextSize};
pub use base::syntax::SyntaxColors;
pub use base::text::{
    ELLIPSIS, char_width, fit as fit_text, truncate as truncate_text, width as text_width,
};
pub use base::theme::{ChromeText, Shape, ShapeShadow, Theme, ThemeToken};
#[cfg(feature = "kdl")]
pub use base::theme_file::{InvalidColor, ThemeFileError};
pub use base::transform::Transform;
pub use chrome::split_view::{DropZone, SplitView, SplitViewResponse};
pub use chrome::status_bar::{ModeBadge, StatusBar, StatusBarResponse, StatusLine, StatusSegment};
pub use chrome::tab_strip::{TabDrag, TabId, TabStrip, TabStripHit, TabStripResponse};
pub use chrome::title_bar::{TitleBar, TitleBarHit, TitleBarResponse, WindowButton, WindowButtons};
pub use input::event::{
    ButtonInput, Composition, FileInput, ImeInput, Input, PointerButton, WheelDelta, WheelInput,
};
pub use platform::clipboard::{ClipboardError, ClipboardKind, FileOperation, SystemClipboard};
pub use platform::error::Error;
pub use platform::resize::ResizeEdge;
pub use platform::waker::{SendError, Waker};
pub use platform::window_id::WindowId;
pub use render::app_fonts::{FontError, load_font};
pub use render::font::{CellMetrics, Font};
pub use render::gpu::Gpu;
#[cfg(feature = "icon-theme")]
pub use render::icon_theme::{IconTheme, ThemeIcon};
pub use render::image::{
    AnimatedImage, AnimationFrame, IconSet, Image, ImageDraw, ImageError, Svg,
};
pub use render::layout;
pub use render::paragraph::{Paragraph, ParagraphLayout, TextAlign};
pub use render::texture::GpuTexture;
pub use runtime::event_loop::run;
pub use wgpu;
pub use widgets::breadcrumb::{Breadcrumb, BreadcrumbResponse, Crumb};
pub use widgets::button::{Button, ButtonResponse, ButtonVariant};
pub use widgets::checkbox::{Checkbox, CheckboxResponse};
pub use widgets::code_view::{CodeView, CodeViewResponse, GutterMark, Highlight};
pub use widgets::command::{
    Command, CommandError, CommandKeys, CommandRegistry, ConflictKind, InvalidKeys, KeyConflict,
    KeyScope, MenuSlot,
};
pub use widgets::completion_list::{CompletionList, CompletionListResponse, CompletionRow};
pub use widgets::dialog::{Dialog, DialogButton, DialogResponse};
#[cfg(feature = "html")]
pub use widgets::document::html::{ClassStyle, HtmlOptions, ImageSource};
pub use widgets::document::{
    Block, BlockImage, Container, DataLink, Document, Heading, Inline, Link, ListBlock, TableBlock,
    TableRow,
};
pub use widgets::drag::{
    Drag, DragResponse, Dropped, FileDrop, FileDropEvent, FileDropped, FileHover, draw_drop_target,
};
pub use widgets::drop_list::{DropList, DropListResponse, DropRow};
pub use widgets::floating_panel::{Corner, FloatingPanel, FloatingPanelResponse};
pub use widgets::flow_list::{FlowList, FlowListResponse, FlowSource, ScrollAlign};
pub use widgets::grid_scroll::{GridLayout, GridLine, GridScroll};
pub use widgets::hex_view::{ByteSource, HexView, HexViewResponse};
#[cfg(feature = "highlight")]
pub use widgets::highlight::Highlighter;
pub use widgets::hints::{Hints, HintsError, HintsResponse};
pub use widgets::icon_grid::{GridData, GridSource, IconGrid, IconGridResponse};
pub use widgets::image_view::ImageView;
pub use widgets::info_card::InfoCard;
pub use widgets::list_keys::ListKeys;
pub use widgets::list_view::{List, ListResponse, ListSource, RowDrop};
pub use widgets::menu::{Menu, MenuEntry, MenuItem, MenuResponse};
pub use widgets::picker;
pub use widgets::popup::{Popup, PopupSide};
pub use widgets::progress::ProgressBar;
pub use widgets::quick_pick::{QuickPick, QuickPickPlacement, QuickPickTick, command_palette};
pub use widgets::radio::{RadioGroup, RadioGroupResponse};
pub use widgets::rich_text::{RichText, RichTextResponse};
pub use widgets::row_content::{Badge, RowContent, RowIcon};
pub use widgets::scroll_view::{ScrollView, ScrollViewResponse};
pub use widgets::select::{Select, SelectResponse};
pub use widgets::settings::{
    NumberRange, Setting, SettingValue, SettingsPage, SettingsPageResponse, SettingsSection,
};
pub use widgets::slider::{Slider, SliderResponse};
pub use widgets::stepper::{Stepper, StepperResponse};
pub use widgets::table::{
    HeaderMenu, SortDirection, SortKey, Table, TableColumn, TableResponse, TableSource,
};
pub use widgets::text_area::{TextArea, TextAreaResponse};
pub use widgets::text_field::{TextField, TextFieldResponse};
pub use widgets::thumbnails::{Thumbnail, Thumbnails};
pub use widgets::tiles::{TileKey, TileSet};
pub use widgets::toast::{Toasts, ToastsResponse};
pub use widgets::toolbar::{ToolButton, Toolbar, ToolbarItem, ToolbarResponse};
pub use widgets::tooltip::Tooltip;
pub use widgets::transient::{
    SwitchKind, Transient, TransientCommand, TransientResponse, TransientRow, TransientSwitch,
};
pub use widgets::tree_view::{TreeItem, TreeView, TreeViewResponse};
pub use widgets::viewport::{Viewport, ViewportResponse};
pub use widgets::which_key::{WhichKey, WhichKeyEntry, WhichKeyResponse};
