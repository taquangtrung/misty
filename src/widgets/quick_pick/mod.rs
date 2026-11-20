//! [`QuickPick`]: the view of a [`Picker`] session, the one popup every
//! type-to-filter list is drawn as (a command palette, quick open, a
//! project search). It shows a heading with the count, the query line, the
//! rows on screen (a label with its description after it, a detail on a
//! second line, a hint at the right, an icon before it, the letters the
//! query matched lit), a line naming the keys the picker offers now, the
//! selected row's action menu while it is open, and a side panel of the
//! text the app previews. [`command_palette`] is the picker over a
//! [`CommandRegistry`].
//!
//! The app holds at most one picker, so two never show at once. It feeds
//! the view every input with its [`Widget::handle`](crate::Widget::handle):
//! a key one of the [`PickerAct`]s does by default is that act (an app
//! with a keymap of its own turns the defaults off with
//! [`QuickPick::without_default_keys`] and calls [`QuickPick::act`]), and
//! any other key goes to the query, which filters as it is typed. A click
//! on a row runs its first action, a click on a row's button runs that
//! button's, and a click outside the picker closes it. The app calls its
//! [`Widget::about_to_wait`](crate::Widget::about_to_wait) from
//! [`Application::about_to_wait`](crate::Application::about_to_wait) to
//! take in the rows the source and the matcher sent, then carries out
//! what [`QuickPick::take_tick`] hands back.
//!
//! The picker is always the same size, centered near the top of the
//! window over a dimmed backdrop, so filtering never reflows it. One made
//! [`QuickPick::with_anchor`] is a menu instead, filtered as it is typed:
//! hung under a rect (a row, the cursor's cell) or above it where there is
//! no room below, as tall as its rows need, with no backdrop.

use std::collections::HashMap;
use std::time::Instant;

use super::super::app::context::EventCx;
use super::super::app::frame::Frame;
use super::super::app::widget::{Outcome, Widget};
use super::super::base::box_style::{Border, BoxStyle};
use super::super::base::color::{Rgb, Rgba};
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::{Key, parse_key};
use super::super::base::styled::{Span, StyledText, TextColor};
use super::super::base::text;
use super::super::base::theme::{ChromeText, Theme, ThemeToken};
use super::super::input::event::{Input, PointerButton, WheelDelta};
use super::super::platform::waker::Waker;
use super::super::render::icon::logical;
use super::super::render::image::IconSet;
use super::command::CommandRegistry;
use super::label::{draw_styled_label, label_width, styled_label_cells, styled_label_width};
use super::picker::{
    ActionId, FixedSource, MenuView, Picker, PickerAct, PickerMemory, PreviewKind, PreviewUpdate,
    Reply, Row, RowKey, SavedLines, SessionView, Tick, ViewRow,
};
use super::popup::{HEADER_HEIGHT_RATIO, Popup, PopupSide, draw_backdrop, draw_popup_header};
use super::text_field::TextFieldResponse;
use super::tooltip::Tooltip;

// ========================================================================
// Constants
// ========================================================================

/// How many lines of rows the picker shows, always; at most this many for
/// one hung from an anchor.
const VISIBLE_LINES: usize = 12;

/// The widest the list is, in cells.
const MAX_COLS: usize = 72;

/// How wide the side panel is beside the list, in cells, and how many lines
/// it takes under the list where the window has no room beside it.
const PANEL_COLS: usize = 60;
const PANEL_LINES: usize = 10;

/// The widest the action menu is, in cells.
const MENU_MAX_COLS: usize = 40;

/// How far down the window the picker's top sits, as a share of the
/// window's height.
const TOP_SHARE: f32 = 0.12;

/// The side of the grip at a placed picker's bottom right corner that
/// resizes it, in cells.
const GRIP_CELLS: f32 = 1.0;

/// The fewest lines of rows a picker resized by the user keeps.
const MIN_LINES: usize = 3;

/// The narrowest a picker resized by the user gets, in cells.
const MIN_COLS: usize = 20;

/// The rows a wheel notch moves the selection.
const WHEEL_ROWS: usize = 3;

/// The cells between a label and its hint, and between it and its
/// description.
const HINT_GAP_CELLS: usize = 2;
const DESCRIPTION_GAP_CELLS: usize = 1;

/// The cells an icon takes, the blank after it included.
const ICON_CELLS: usize = 2;

/// The room between a row's icon and its label, in cells.
const ICON_GAP_CELLS: f32 = 0.6;

/// The share of a line's height an icon is tall.
const ICON_SHARE: f32 = 0.85;

/// What precedes the query.
const PROMPT: &str = "> ";

/// What stands between two of the hints line's keys.
const HINT_SEPARATOR: &str = " · ";

/// A boxed query's height ([`QueryStyle::Field`]) as a multiple of the
/// line height, room for the box around the text.
const FIELD_QUERY_RATIO: f32 = 1.5;

/// The room between a boxed query and the line under it, as a multiple of
/// the line height, so the box does not sit on the rows.
const FIELD_GAP_RATIO: f32 = 0.25;

/// The room above a boxed query past what it shares evenly with the line
/// under it, in logical pixels, so the box stands clear of the heading.
const FIELD_TOP: f32 = 2.0;

/// How much of the background a boxed query's well mixes into the raised
/// color, setting it a little in from the picker around it.
const FIELD_WELL_SHARE: f32 = 0.6;

/// How far a boxed query's text sits in from the box's side, in cells.
const FIELD_INSET_CELLS: f32 = 0.7;

/// The thickness of a boxed query's border, in logical pixels.
const FIELD_BORDER: f32 = 1.0;

/// The thickness of the lines between the picker's parts, in logical
/// pixels.
const SEPARATOR_STROKE: f32 = 1.0;

/// The color matched characters are drawn in: not the selection color, so
/// they stay visible on the selected row.
const MATCH_COLOR: TextColor = TextColor::Token(ThemeToken::MatchText);

/// The color of the prompt before the query.
const PROMPT_COLOR: TextColor = TextColor::Token(ThemeToken::Accent);

/// The color labels and details are drawn in.
const LABEL_COLOR: TextColor = TextColor::Token(ThemeToken::Foreground);

/// The color the heading, the count, descriptions, and hints are drawn in.
const QUIET_COLOR: TextColor = TextColor::Token(ThemeToken::Muted);

/// The color a source's failure is said in.
const ERROR_COLOR: TextColor = TextColor::Token(ThemeToken::Danger);

// ========================================================================
// Data Structures
// ========================================================================

/// The view of a picker session over rows doing `O`s and previewing as
/// `P`s. It keeps the session, the panel's text, and where the last frame
/// drew each part, which its pointer handling uses.
pub struct QuickPick<O, P = ()> {
    /// The rect the picker hangs from as a menu, or `None` for one placed
    /// as [`QuickPick::set_placement`] says.
    anchor: Option<Rect>,
    /// Whether keys a picker act does by default are those acts.
    default_keys: bool,
    /// The colors each row's description is drawn in, as runs of
    /// `(characters, color)`, by row.
    description_colors: HashMap<RowKey, Vec<(usize, Rgb)>>,
    /// The drag moving or resizing the picker, while the button is held.
    grab: Option<Grab>,
    /// The key the hints line names for an act, by act, where the app's
    /// keymap binds it to another than its default.
    hint_keys: HashMap<PickerAct, String>,
    /// The index on screen of the row under the pointer.
    hovered: Option<usize>,
    /// Whether a row has shown an icon, so their column stays for the rest
    /// of the session whatever the query leaves.
    icons_seen: bool,
    /// The icons rows and buttons name.
    icons: IconSet,
    /// Where the parts were last drawn, or `None` before the first frame.
    layout: Option<PickLayout>,
    /// How far the user moved the picker from its place.
    offset: Point,
    /// Where the pointer is over the picker, or `None` off it.
    pointer: Option<Point>,
    /// The text the side panel shows: the selected row's preview, as the
    /// app read it.
    panel: Vec<StyledText>,
    /// What the ticks since the app last took it handed back, for the app
    /// to carry out; `None` when none handed back anything.
    pending: Option<QuickPickTick<O, P>>,
    /// The lines of rows the list showed when it was last drawn, which
    /// the selection moves and pages by.
    lines: usize,
    /// The session.
    picker: Picker<O, P>,
    /// Where the picker goes in the window.
    placement: QuickPickPlacement,
    /// How the query line is drawn.
    query_style: QueryStyle,
    /// The shares of the window's width and height a picker placed in the
    /// window takes, or `None` for the size its rows need.
    window_share: Option<(f32, f32)>,
    /// The size the user gave the picker, `(width, height)`, or `None` for
    /// the size it takes.
    size: Option<(f32, f32)>,
    /// What the button under the pointer does, once it has rested there.
    tooltip: Tooltip,
}

/// How a [`QuickPick`] draws its query line.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum QueryStyle {
    /// After a `> ` prompt on a plain line, as a terminal's picker reads.
    /// The default.
    #[default]
    Prompt,
    /// In a box a line and a half tall, lined with the accent and set a
    /// little in from the picker, as a desktop app's search field is.
    Field,
}

/// What a [`QuickPick`]'s ticks took in for the app to carry out, handed
/// over by [`QuickPick::take_tick`]: at least one of the three is set.
#[derive(Debug)]
#[non_exhaustive]
pub struct QuickPickTick<O, P = ()> {
    /// What saving the rows came to, where the source saves them its own
    /// way: carry it out and close the picker.
    pub outcome: Option<O>,
    /// What to preview, when the selection came to rest on another row:
    /// read it and show it with [`QuickPick::set_panel`].
    pub preview: Option<PreviewUpdate<P>>,
    /// The rows saved as lines of text, where the source saves them no way
    /// of its own: keep them as the app keeps text, and close the picker.
    pub saved: Option<SavedLines>,
}

/// Where a [`QuickPick`] goes in the window.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum QuickPickPlacement {
    /// Centered in this rect: a list an app's pane asked for, over the
    /// pane.
    CenteredIn(Rect),
    /// Along the bottom of this rect, across its width: a live picker over
    /// the pane whose view it moves, which stays in sight above it.
    DockedBottom(Rect),
    /// Filling this rect: a list taking the place of the box it opened
    /// from, such as a transient's.
    Filling(Rect),
    /// Centered near the top of the window over a dimmed backdrop (the
    /// default).
    #[default]
    Window,
}

/// A drag of the picker under way.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Grab {
    /// Moving it by its heading: where the drag began, and the offset then.
    Move {
        /// The offset when the drag began.
        offset: Point,
        /// Where the pointer was pressed.
        origin: Point,
    },
    /// Resizing it by its grip: where the drag began, and the size then.
    Resize {
        /// Where the pointer was pressed.
        origin: Point,
        /// The size when the drag began.
        size: (f32, f32),
    },
}

/// A row's button as drawn: where, the row on screen it is on, its
/// action, and what it is called.
type DrawnButton = (Rect, usize, ActionId, String);

/// Where a picker's parts were drawn.
#[derive(Clone, Debug, Default, PartialEq)]
struct PickLayout {
    /// Each button drawn.
    buttons: Vec<DrawnButton>,
    /// The action menu's box, while it is open.
    menu: Option<Rect>,
    /// The action menu's entries, top to bottom.
    menu_rows: Vec<Rect>,
    /// The resize grip, for a picker the user may resize.
    grip: Option<Rect>,
    /// The heading's strip, which moves a placed picker.
    heading: Rect,
    /// The height of a line of text, as last drawn.
    line: f32,
    /// The whole box.
    popup: Rect,
    /// The rows on screen, top to bottom, each its lines.
    rows: Vec<Rect>,
    /// The display's scale factor, as last drawn.
    scale_factor: f64,
}

/// The keys the hints line names: the defaults when the picker answers
/// them, and those the app named.
#[derive(Clone, Copy, Debug)]
struct HintKeys<'a> {
    /// Whether the acts' default keys are the picker's.
    defaults: bool,
    /// The keys the app named, by act.
    named: &'a HashMap<PickerAct, String>,
}

/// Where the parts of a picker go, before they are drawn.
#[derive(Clone, Copy, Debug)]
struct Geometry {
    /// The lines of rows the list shows.
    lines: usize,
    /// The list's column of text, heading to hints line.
    list: Rect,
    /// The side panel's text, when it is shown.
    panel: Option<Rect>,
    /// Whether the panel is beside the list rather than under it.
    panel_beside: bool,
    /// The whole box.
    popup: Rect,
}

// ========================================================================
// QuickPick Implementations
// ========================================================================

impl<O, P> QuickPick<O, P> {
    /// The view of `picker`, centered near the top of the window, its keys
    /// the acts' defaults.
    pub fn new(mut picker: Picker<O, P>) -> Self {
        picker.set_lines(VISIBLE_LINES);
        Self {
            anchor: None,
            default_keys: true,
            description_colors: HashMap::new(),
            grab: None,
            hint_keys: HashMap::new(),
            hovered: None,
            icons: IconSet::default(),
            icons_seen: false,
            layout: None,
            lines: VISIBLE_LINES,
            offset: Point::default(),
            panel: Vec::new(),
            pointer: None,
            pending: None,
            picker,
            placement: QuickPickPlacement::default(),
            query_style: QueryStyle::default(),
            size: None,
            tooltip: Tooltip::new(),
            window_share: None,
        }
    }

    /// This picker hung from `anchor` as a menu filtered as it is typed:
    /// a row's actions under the row, the choices at the cursor under its
    /// cell. It opens under `anchor`, or above it where only there it fits,
    /// as tall as its rows need (at most twelve lines) and as wide as its
    /// widest, with no backdrop.
    pub fn with_anchor(self, anchor: Rect) -> Self {
        Self {
            anchor: Some(anchor),
            ..self
        }
    }

    /// This picker drawing the icons its rows and buttons name from `icons`.
    pub fn with_icons(self, icons: IconSet) -> Self {
        Self { icons, ..self }
    }

    /// This picker, placed in the window, taking `width` and `height` of the
    /// window's width and height (each held to a tenth or more, the whole at
    /// most) in the middle of it, as a desktop app's command palette does,
    /// rather than the size its rows need near the top.
    pub fn with_window_share(self, width: f32, height: f32) -> Self {
        Self {
            window_share: Some((width.clamp(0.1, 1.0), height.clamp(0.1, 1.0))),
            ..self
        }
    }

    /// This picker drawing its query line as `query_style` says.
    pub fn with_query_style(self, query_style: QueryStyle) -> Self {
        Self {
            query_style,
            ..self
        }
    }

    /// This picker taking keys only for its query: an app with a keymap of
    /// its own resolves them and calls [`QuickPick::act`].
    pub fn without_default_keys(self) -> Self {
        Self {
            default_keys: false,
            ..self
        }
    }

    /// The session.
    pub fn picker(&self) -> &Picker<O, P> {
        &self.picker
    }

    /// The session, to seed its query or run a source's action.
    pub fn picker_mut(&mut self) -> &mut Picker<O, P> {
        &mut self.picker
    }

    /// The session, for the app to park it to resume once the picker
    /// closes.
    pub fn into_picker(self) -> Picker<O, P> {
        self.picker
    }

    /// Does `act`, as a key the app's keymap bound to it.
    pub fn act(&mut self, act: PickerAct) -> Reply<O> {
        // The lines drawn, not the most a list shows: told another count,
        // the session would fetch its rows again for it, then again when
        // the next frame says the count it drew.
        self.picker.act(act, self.lines, Instant::now())
    }

    /// Hangs the picker from `anchor` as [`QuickPick::with_anchor`] does,
    /// from the next frame, or places it as its placement says with `None`:
    /// for an anchor that moves, such as the cursor of the text it acts on.
    pub fn set_anchor(&mut self, anchor: Option<Rect>) {
        self.anchor = anchor;
    }

    /// The icons rows and buttons name, for an app that adds them as rows
    /// naming them come in rather than all up front.
    pub fn icons_mut(&mut self) -> &mut IconSet {
        &mut self.icons
    }

    /// Names `key` for `act` in the hints line: the key an app's own keymap
    /// binds it to, written as the app writes keys.
    pub fn set_hint_key(&mut self, act: PickerAct, key: impl Into<String>) {
        self.hint_keys.insert(act, key.into());
    }

    /// Puts the picker where `placement` says, from the next frame.
    pub fn set_placement(&mut self, placement: QuickPickPlacement) {
        self.placement = placement;
    }

    /// Draws each row's description in its colors, as runs of
    /// `(characters, color)` by row (a line of code in its syntax colors),
    /// the characters past the runs and rows with none in the muted color.
    /// Matched characters stay lit.
    pub fn set_description_colors(&mut self, colors: HashMap<RowKey, Vec<(usize, Rgb)>>) {
        self.description_colors = colors;
    }

    /// Shows `lines` in the side panel: the selected row's preview, as the
    /// app read it when [`QuickPick::take_tick`] handed it over.
    pub fn set_panel(&mut self, lines: Vec<StyledText>) {
        self.panel = lines;
    }

    /// What the picker's ticks (its [`Widget::about_to_wait`]) took in
    /// since the last call, for the app to carry out: an outcome or rows
    /// saved, and a preview to read. Ticks the app did not take in between
    /// are merged: the first outcome and rows saved are kept, and the
    /// latest preview. `None` when no tick since took in any of them.
    pub fn take_tick(&mut self) -> Option<QuickPickTick<O, P>> {
        self.pending.take()
    }

    /// The query caret's cell in the last frame, in physical pixels, for
    /// [`ImeFocus::follow`](crate::ImeFocus::follow) to anchor the input
    /// method's candidate window to; `None` before the first frame.
    pub fn cursor_area(&self) -> Option<Rect> {
        self.picker.field().cursor_area()
    }

    /// Reacts to pointer `input`: the query takes presses and drags on it,
    /// a left press runs the action menu's entry, a row's button, or a row's
    /// first action under it, and closes the picker outside it; the wheel
    /// moves the selection. Every pointer input is the picker's.
    fn handle_pointer(&mut self, input: &Input, cx: &mut EventCx<'_>) -> Reply<O> {
        if self.drag(input) {
            return Reply::Consumed;
        }
        if self.picker.field_mut().handle_pointer(input, cx) != TextFieldResponse::Ignored {
            return Reply::Consumed;
        }
        match input {
            Input::PointerButton(button) => {
                match button.pressed && button.button == PointerButton::Left {
                    true => self.press(button.position),
                    false => Reply::Consumed,
                }
            }
            Input::PointerLeft => {
                self.hovered = None;
                self.pointer = None;
                Reply::Consumed
            }
            Input::PointerMoved(position) => {
                self.pointer = Some(*position);
                self.hovered = self
                    .layout
                    .as_ref()
                    .and_then(|layout| layout.rows.iter().position(|row| row.contains(*position)));
                Reply::Consumed
            }
            Input::Wheel(wheel) => {
                let down = match wheel.delta {
                    WheelDelta::Lines(lines) => lines.y < 0.0,
                    WheelDelta::Pixels(pixels) => pixels.y < 0.0,
                };
                let act = match down {
                    true => PickerAct::Next,
                    false => PickerAct::Previous,
                };
                for _ in 0..WHEEL_ROWS {
                    self.act(act);
                }
                Reply::Consumed
            }
            Input::KeyReleased(_)
            | Input::AppearanceChanged(_)
            | Input::CloseRequested
            | Input::FileDropped(_)
            | Input::FileHovered(_)
            | Input::FileHoverCancelled
            | Input::FocusGained
            | Input::FocusLost
            | Input::Ime(_)
            | Input::Key(_) => Reply::Ignored,
        }
    }

    /// Moves or resizes the picker by `input` while it is grabbed, starts
    /// a grab on a left press on the heading (to move it) or on the grip
    /// (to resize it), and ends it on the release. Whether the input was
    /// the grab's.
    fn drag(&mut self, input: &Input) -> bool {
        if let Some(grab) = self.grab {
            return self.follow_grab(input, grab);
        }
        let Input::PointerButton(button) = input else {
            return false;
        };
        if !button.pressed || button.button != PointerButton::Left {
            return false;
        }
        let Some(layout) = &self.layout else {
            return false;
        };
        let point = button.position;
        let popup = layout.popup;
        if layout.grip.is_some_and(|grip| grip.contains(point)) {
            self.grab = Some(Grab::Resize {
                origin: point,
                size: (popup.width, popup.height),
            });
            return true;
        }
        let on_heading = layout.heading.contains(point) && layout.menu.is_none();
        if on_heading && self.anchor.is_none() {
            self.grab = Some(Grab::Move {
                offset: self.offset,
                origin: point,
            });
            return true;
        }
        false
    }

    /// Moves or resizes the picker by `input` under `grab`, ending the
    /// grab on a release. Every pointer input is the grab's.
    fn follow_grab(&mut self, input: &Input, grab: Grab) -> bool {
        match input {
            Input::PointerMoved(point) => {
                match grab {
                    Grab::Move { offset, origin } => {
                        self.offset = Point::new(
                            offset.x + point.x - origin.x,
                            offset.y + point.y - origin.y,
                        );
                    }
                    Grab::Resize { origin, size } => {
                        let least = self.least_size();
                        self.size = Some((
                            (size.0 + point.x - origin.x).max(least.0),
                            (size.1 + point.y - origin.y).max(least.1),
                        ));
                    }
                }
                true
            }
            Input::PointerButton(button) => {
                if !button.pressed {
                    self.grab = None;
                }
                true
            }
            Input::PointerLeft | Input::Wheel(_) => true,
            Input::KeyReleased(_)
            | Input::AppearanceChanged(_)
            | Input::CloseRequested
            | Input::FileDropped(_)
            | Input::FileHovered(_)
            | Input::FileHoverCancelled
            | Input::FocusGained
            | Input::FocusLost
            | Input::Ime(_)
            | Input::Key(_) => false,
        }
    }

    /// The smallest size the user may resize the picker to: a few rows of
    /// a narrow list.
    fn least_size(&self) -> (f32, f32) {
        let Some(layout) = &self.layout else {
            return (0.0, 0.0);
        };
        let line = layout.line;
        let cell = line / 2.0;
        let width = MIN_COLS as f32 * cell;
        let height = self.header_height(line, layout.scale_factor) + (MIN_LINES + 1) as f32 * line;
        (
            width.min(layout.popup.width),
            height.min(layout.popup.height),
        )
    }

    /// A left press at `point`: an action menu entry, a button, or a row
    /// runs; outside the picker, it closes.
    fn press(&mut self, point: Point) -> Reply<O> {
        let Some(layout) = &self.layout else {
            return Reply::Consumed;
        };
        if let Some(menu) = layout.menu {
            let entry = layout.menu_rows.iter().position(|row| row.contains(point));
            return match (entry, menu.contains(point)) {
                (Some(at), _) => {
                    self.picker.select_shown(at);
                    self.act(PickerAct::Accept)
                }
                (None, true) => Reply::Consumed,
                (None, false) => self.act(PickerAct::Close),
            };
        }
        let button = layout
            .buttons
            .iter()
            .find(|(rect, ..)| rect.contains(point));
        if let Some(&(_, at, action, _)) = button {
            return self.picker.click_button(at, action);
        }
        if let Some(at) = layout.rows.iter().position(|row| row.contains(point)) {
            self.picker.select_shown(at);
            return self.act(PickerAct::Accept);
        }
        match layout.popup.contains(point) {
            true => Reply::Consumed,
            false => Reply::Close,
        }
    }

    /// Where the parts go in `area`, for `view`: the box its placement (or
    /// its anchor) gives it at the size it takes, then the size and the
    /// offset the user gave it, and the lines and columns fitted inside.
    fn geometry(&self, frame: &Frame, view: &SessionView, area: Rect, theme: &Theme) -> Geometry {
        let metrics = frame.cell_metrics();
        let scale_factor = frame.scale_factor();
        let padding = metrics.width.round();
        let inset = Popup::inset(&theme.shape, scale_factor);
        let stroke = logical(SEPARATOR_STROKE, scale_factor);
        let margins = 2.0 * (padding + inset);
        let room = metrics.cols_in(area.width - margins);
        let natural_cols = match self.anchor {
            Some(_) => anchored_cols(frame, theme, view, self.icon_column(view)).min(MAX_COLS),
            None => MAX_COLS,
        };
        let shown = view.panel && view.preview_kind == PreviewKind::Panel;
        let natural_beside = shown && room >= natural_cols + 1 + PANEL_COLS;
        let natural_list = natural_cols.min(room);
        let natural_width_cols = match natural_beside {
            true => natural_list + 1 + PANEL_COLS,
            false => natural_list,
        };
        let natural_lines = match self.anchor {
            Some(_) => anchored_lines(view),
            None => VISIBLE_LINES,
        };
        let hint_lines = usize::from(!view.hints.is_empty());
        // The height around the rows: the heading and query, the hints, the
        // panel under the list, the separators, and the box's inset.
        let chrome = |beside: bool| {
            let under = match shown && !beside {
                true => PANEL_LINES,
                false => 0,
            };
            let strokes = 1 + usize::from(under > 0);
            self.header_height(metrics.height, scale_factor)
                + (hint_lines + under) as f32 * metrics.height
                + hint_lines as f32 * 2.0 * row_pad(theme, scale_factor)
                + strokes as f32 * stroke
                + 2.0 * inset
        };
        let slot = row_slot(metrics.height, theme, scale_factor);
        let width = (natural_width_cols as f32 * metrics.width + margins).ceil();
        let natural_height = natural_lines as f32 * slot + chrome(natural_beside);
        let (width, height) = match (self.anchor, self.placement, self.window_share) {
            (None, QuickPickPlacement::Window, Some((width_share, height_share))) => (
                (area.width * width_share).round(),
                (area.height * height_share).round(),
            ),
            _ => (width, natural_height),
        };
        let placed = match (self.anchor, self.placement) {
            (Some(anchor), _) => Popup::new(anchor, width, height, PopupSide::Below, area).rect(),
            (None, QuickPickPlacement::Window) => {
                let x = (area.x + (area.width - width) / 2.0).round().max(area.x);
                // A picker sized by its share sits in the middle; one sized by
                // its rows near the top, where its rows grow down from.
                let y = match self.window_share {
                    Some(_) => (area.y + (area.height - height) / 2.0).round(),
                    None => (area.y + area.height * TOP_SHARE).round(),
                };
                Rect::new(x, y, width, height)
            }
            (None, QuickPickPlacement::CenteredIn(pane)) => {
                let (width, height) = (width.min(pane.width), height.min(pane.height));
                let x = (pane.x + (pane.width - width) / 2.0).round();
                let y = (pane.y + (pane.height - height) / 2.0).round();
                Rect::new(x, y, width, height)
            }
            (None, QuickPickPlacement::DockedBottom(pane)) => {
                let height = height.min(pane.height);
                Rect::new(pane.x, pane.y + pane.height - height, pane.width, height)
            }
            (None, QuickPickPlacement::Filling(rect)) => rect,
        };
        let sized = match self.size {
            Some((width, height)) => Rect::new(placed.x, placed.y, width, height),
            None => placed,
        };
        let popup = Rect::new(
            sized.x + self.offset.x,
            sized.y + self.offset.y,
            sized.width,
            sized.height,
        );
        // What fits the box as it ended up.
        let cols = metrics.cols_in(popup.width - margins);
        let beside = match sized == placed && self.anchor.is_none() && placed.width == width {
            true => natural_beside,
            false => shown && cols >= MIN_COLS + 1 + PANEL_COLS,
        };
        let list_cols = match beside {
            true => cols.saturating_sub(1 + PANEL_COLS),
            false => cols,
        };
        let lines = match popup.height == natural_height {
            true => natural_lines,
            false => {
                (((popup.height - chrome(beside)) / slot).floor().max(0.0) as usize).max(MIN_LINES)
            }
        };
        // A floating box given a height (a share of the window, or the
        // user's) ends under a whole row, rather than leaving what is left
        // over of a row between the rows and the hints: grown to show the
        // row that only partly fit, where the room around it allows, else
        // trimmed to the rows that fit whole. One sized by its share stays
        // in the middle. A docked or filling box keeps the height it fills.
        let bound = match self.placement {
            QuickPickPlacement::CenteredIn(pane) => Some(pane),
            QuickPickPlacement::Window => Some(area),
            QuickPickPlacement::DockedBottom(_) | QuickPickPlacement::Filling(_) => None,
        };
        let whole = |lines: usize| lines as f32 * slot + chrome(beside);
        let centered = self.window_share.is_some() && self.size.is_none();
        let refit = |height: f32| {
            let rise = match centered {
                true => ((popup.height - height) / 2.0).round(),
                false => 0.0,
            };
            Rect::new(popup.x, popup.y + rise, popup.width, height)
        };
        let (lines, popup) = match bound.filter(|_| self.anchor.is_none()) {
            Some(bound) if whole(lines) < popup.height => {
                let grown = refit(whole(lines + 1));
                let fits = grown.y >= bound.y && grown.y + grown.height <= bound.y + bound.height;
                match fits {
                    true => (lines + 1, grown),
                    false => (lines, refit(whole(lines))),
                }
            }
            _ => (lines, popup),
        };
        let content = Popup::with_rect(popup).content(&theme.shape, scale_factor);
        // Whole columns beside a panel, which starts a column after them;
        // alone, the list takes the box's width, its padding even on both
        // sides.
        let list_width = match beside {
            true => list_cols as f32 * metrics.width,
            false => (content.width - 2.0 * padding).max(0.0),
        };
        let list = Rect::new(
            content.x + padding,
            content.y,
            list_width,
            self.header_height(metrics.height, scale_factor)
                + lines as f32 * slot
                + hint_lines as f32 * (metrics.height + 2.0 * row_pad(theme, scale_factor))
                + stroke,
        );
        let panel = shown.then(|| match beside {
            true => Rect::new(
                list.x + list.width + metrics.width,
                content.y,
                PANEL_COLS as f32 * metrics.width,
                list.height,
            ),
            false => Rect::new(
                list.x,
                list.y + list.height + stroke,
                list.width,
                PANEL_LINES as f32 * metrics.height,
            ),
        });
        Geometry {
            lines,
            list,
            panel,
            panel_beside: beside,
            popup,
        }
    }

    /// How tall the query line is in lines `line` pixels high, at
    /// `scale_factor`.
    fn query_height(&self, line: f32, scale_factor: f64) -> f32 {
        match self.query_style {
            QueryStyle::Prompt => line,
            QueryStyle::Field => {
                (line * FIELD_QUERY_RATIO).round()
                    + field_gap(line)
                    + logical(FIELD_TOP, scale_factor)
            }
        }
    }

    /// How tall the heading's strip is in lines `line` pixels high: a popup
    /// header's height, as [`Popup::header_height`] gives it.
    fn heading_height(line: f32) -> f32 {
        (line * HEADER_HEIGHT_RATIO).round()
    }

    /// How tall the heading and the query line are together, in lines
    /// `line` pixels high, at `scale_factor`.
    fn header_height(&self, line: f32, scale_factor: f64) -> f32 {
        Self::heading_height(line) + self.query_height(line, scale_factor)
    }

    /// Draws the heading on `strip`, across the top of the box, with its
    /// count at the right, and the query line under it, from the top of
    /// `list`.
    fn draw_header(
        &mut self,
        frame: &mut Frame,
        view: &SessionView,
        (strip, list): (Rect, Rect),
        theme: &Theme,
    ) {
        let metrics = frame.cell_metrics();
        let count = view.count(&theme.strings);
        let count_x = list.x + list.width - label_width(frame, theme, &count);
        let mut heading = StyledText::plain(view.title.as_str(), LABEL_COLOR).with_ellipsis();
        if let Some(error) = &view.error {
            heading.push(Span::new(format!(": {error}"), ERROR_COLOR));
        }
        let heading_height = Self::heading_height(metrics.height);
        draw_popup_header(frame, theme, strip, &heading);
        let count = StyledText::plain(count, QUIET_COLOR);
        let count_y = (strip.y + (strip.height - metrics.height) / 2.0).round();
        let line = Rect::new(list.x, strip.y, list.width, strip.height);
        draw_styled_label(frame, theme, &count, Point::new(count_x, count_y), line);
        let top = list.y + heading_height;
        let field = match self.query_style {
            QueryStyle::Prompt => {
                let query_line = Rect::new(list.x, top, list.width, metrics.height);
                let prompt = StyledText::plain(PROMPT, PROMPT_COLOR);
                frame.draw_text(&prompt, Point::new(list.x, top), query_line, theme);
                let prompt_width = text::width(PROMPT) as f32 * metrics.width;
                Rect::new(
                    list.x + prompt_width,
                    top,
                    (list.width - prompt_width).max(0.0),
                    metrics.height,
                )
            }
            QueryStyle::Field => {
                // Centered between the heading's strip and the line under
                // the query, the room above and below it even, then moved
                // down by the extra room above it: a pixel the two cannot
                // share goes to the box.
                let scale_factor = frame.scale_factor();
                let extra = logical(FIELD_TOP, scale_factor);
                let above = (top - (strip.y + strip.height)).max(0.0);
                let room = above + field_gap(metrics.height);
                let side = (room / 2.0).floor();
                let height = self.query_height(metrics.height, scale_factor)
                    - field_gap(metrics.height)
                    - extra
                    + (room - 2.0 * side);
                let well_top = top - above + side + extra;
                // As far in from the picker's border at the sides as from
                // the heading above it: the rows' highlight band, widened
                // (or narrowed) evenly to that, so beside a panel it still
                // ends where the rows do.
                let padding = metrics.width.round();
                let rows = Rect::new(
                    list.x - padding,
                    well_top,
                    list.width + 2.0 * padding,
                    height,
                );
                let band = theme.shape.highlight_band(rows, scale_factor);
                let gap = well_top - (strip.y + strip.height);
                let widen = band.x - (strip.x + gap);
                let well = Rect::new(
                    band.x - widen,
                    well_top,
                    (band.width + 2.0 * widen).max(0.0),
                    height,
                );
                let style = BoxStyle {
                    border: Some(Border {
                        color: Rgba::from(theme.accent),
                        width: logical(FIELD_BORDER, scale_factor),
                    }),
                    fill: Rgba::from(theme.raised.mix(theme.background, FIELD_WELL_SHARE)),
                    ..theme.shape.highlight(theme.raised, scale_factor)
                };
                frame.draw_box(well, &style);
                let inset = (FIELD_INSET_CELLS * metrics.width).round();
                Rect::new(
                    well.x + inset,
                    (well.y + (height - metrics.height) / 2.0).round(),
                    (well.width - 2.0 * inset).max(0.0),
                    metrics.height,
                )
            }
        };
        let top = field.y;
        // A boxed query is a desktop search field: in the proportional font
        // where the theme's chrome is, its placeholder with it.
        let proportional =
            self.query_style == QueryStyle::Field && theme.chrome_text == ChromeText::Proportional;
        if view.query.is_empty() && !view.placeholder.is_empty() {
            let placeholder = StyledText::plain(view.placeholder.as_str(), QUIET_COLOR);
            let at = Point::new(field.x, top);
            match proportional {
                true => {
                    draw_styled_label(frame, theme, &placeholder, at, field);
                }
                false => {
                    frame.draw_text(&placeholder, at, field, theme);
                }
            }
        }
        let query_field = self.picker.field_mut();
        query_field.set_proportional_font(proportional);
        query_field.draw(frame, field, theme);
    }

    /// Whether rows keep a column for icons: the source says its rows lead
    /// with them, or a row of this session has shown one, so labels never
    /// jump sideways as the query filters the rows with icons out.
    fn icon_column(&self, view: &SessionView) -> bool {
        view.icons || self.icons_seen || view.rows.iter().any(|shown| shown.row.icon.is_some())
    }

    /// Draws the rows on screen in `rows`; where each row and each button
    /// was drawn.
    fn draw_rows(
        &self,
        frame: &mut Frame,
        view: &SessionView,
        rows: Rect,
        theme: &Theme,
    ) -> (Vec<Rect>, Vec<DrawnButton>) {
        let metrics = frame.cell_metrics();
        let has_icons = self.icon_column(view);
        let mut bands = Vec::new();
        let mut buttons = Vec::new();
        let mut top = rows.y;
        for (at, shown) in view.rows.iter().enumerate() {
            let height =
                shown.row.lines() as f32 * row_slot(metrics.height, theme, frame.scale_factor());
            if top + height > rows.y + rows.height {
                break;
            }
            let band = Rect::new(rows.x, top, rows.width, height);
            let lit = view.selected == Some(at);
            if lit {
                let scale_factor = frame.scale_factor();
                let style = theme.shape.highlight(theme.selected_row, scale_factor);
                frame.draw_box(theme.shape.highlight_band(band, scale_factor), &style);
            }
            let look = RowLook {
                has_icons,
                lit,
                with_buttons: lit || self.hovered == Some(at),
            };
            let drawn = self.draw_row(frame, shown, band, look, theme);
            let drawn = drawn.into_iter();
            buttons.extend(drawn.map(|(rect, action, title)| (rect, at, action, title)));
            bands.push(band);
            top += height;
        }
        (bands, buttons)
    }

    /// Draws `shown` in `band` as `look` says: its icon, label and
    /// description, buttons, hint, and detail; where each button was drawn.
    fn draw_row(
        &self,
        frame: &mut Frame,
        shown: &ViewRow,
        band: Rect,
        look: RowLook,
        theme: &Theme,
    ) -> Vec<(Rect, ActionId, String)> {
        let metrics = frame.cell_metrics();
        let scale_factor = frame.scale_factor();
        // The text sits the shape's row padding into the band, each further
        // line of the row a slot below.
        let top = band.y + row_pad(theme, scale_factor);
        let slot = row_slot(metrics.height, theme, scale_factor);
        // In from the band's sides past its highlight, so the icon and the
        // hint sit inside the lit band.
        let side = (2.0 * f32::from(theme.shape.highlight_inset) * scale_factor as f32).round();
        let row = &shown.row;
        let icon_width = ICON_CELLS as f32 * metrics.width;
        let foreground = theme.foreground;
        let mut x = band.x + side;
        if look.has_icons {
            let rect = icon_rect(x, top, metrics.height);
            if let Some(name) = row.icon.as_deref() {
                frame.draw_named_icon(&self.icons, name, rect, foreground);
            }
            // The icon, then a gap before the label, whether this row has an
            // icon or not, so the labels line up.
            x += rect.width + (ICON_GAP_CELLS * metrics.width).round();
        }
        let mut right = band.x + band.width - side;
        // Muted text would fade into the selection color.
        let quiet = match look.lit {
            true => LABEL_COLOR,
            false => QUIET_COLOR,
        };
        if !row.hint.is_empty() {
            let hint = StyledText::plain(row.hint.as_str(), quiet);
            let hint_x = right - styled_label_width(frame, theme, &hint);
            let line = Rect::new(band.x, top, band.width, metrics.height);
            draw_styled_label(frame, theme, &hint, Point::new(hint_x, top), line);
            right = hint_x - HINT_GAP_CELLS as f32 * metrics.width;
        }
        let mut drawn = Vec::new();
        if look.with_buttons {
            for action in shown.buttons.iter().rev() {
                let Some(name) = action
                    .button
                    .as_deref()
                    .filter(|name| self.icons.get(name).is_some())
                else {
                    continue;
                };
                right -= icon_width;
                let rect = icon_rect(right, top, metrics.height);
                frame.draw_named_icon(&self.icons, name, rect, foreground);
                let rect = Rect::new(right, top, icon_width, metrics.height);
                drawn.push((rect, action.id, action.title.clone()));
            }
        }
        let mut line = highlighted(&row.label, &shown.lit.label, LABEL_COLOR);
        if !row.description.is_empty() {
            line.push(Span::new(" ".repeat(DESCRIPTION_GAP_CELLS), quiet));
            let colors = self.description_colors.get(&row.key).map(Vec::as_slice);
            let description = match colors {
                Some(runs) => colored(&row.description, &shown.lit.description, runs, quiet),
                None => highlighted(&row.description, &shown.lit.description, quiet),
            };
            for span in description.spans() {
                line.push(span.clone());
            }
        }
        let clip = Rect::new(x, top, (right - x).max(0.0), metrics.height);
        draw_styled_label(
            frame,
            theme,
            &line.with_ellipsis(),
            Point::new(x, top),
            clip,
        );
        if !row.detail.is_empty() {
            let top = top + slot;
            let detail = highlighted(&row.detail, &shown.lit.detail, LABEL_COLOR).with_ellipsis();
            let clip = Rect::new(
                x,
                top,
                (band.x + band.width - side - x).max(0.0),
                metrics.height,
            );
            draw_styled_label(frame, theme, &detail, Point::new(x, top), clip);
        }
        drawn
    }

    /// Draws the panel's text in `panel`, a line set between it and the
    /// list: left of it `beside`, else above it.
    fn draw_panel(&self, frame: &mut Frame, panel: Rect, beside: bool, theme: &Theme) {
        let metrics = frame.cell_metrics();
        let stroke = logical(SEPARATOR_STROKE, frame.scale_factor());
        let edge = match beside {
            true => Rect::new(panel.x - metrics.width / 2.0, panel.y, stroke, panel.height),
            false => Rect::new(panel.x, panel.y - stroke, panel.width, stroke),
        };
        frame.fill_rect_over_text(edge, theme.border);
        let fits = (panel.height / metrics.height).floor() as usize;
        for (at, line) in self.panel.iter().take(fits).enumerate() {
            let top = panel.y + at as f32 * metrics.height;
            let clip = Rect::new(panel.x, top, panel.width, metrics.height);
            frame.draw_text(line, Point::new(panel.x, top), clip, theme);
        }
    }
}

impl<O, P> Widget for QuickPick<O, P> {
    type Output = ();
    type Response = Reply<O>;

    /// Reacts to `input`: keys and input method text as the module
    /// documentation lists, and the pointer: the query takes presses and
    /// drags on it, a left press runs the action menu's entry, a row's
    /// button, or a row's first action under it and closes the picker
    /// outside it, and the wheel moves the selection. The picker is modal:
    /// every input is its own. Asks for a redraw whenever it used the input.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> Reply<O> {
        let now = Instant::now();
        // A key, a press, or a scroll puts a button's tooltip away.
        let pressed = matches!(input, Input::PointerButton(button) if button.pressed);
        if pressed || matches!(input, Input::Key(_) | Input::Wheel(_)) {
            self.tooltip.dismiss();
        }
        let response = match input {
            Input::Ime(ime) => self.picker.ime(ime, now),
            Input::Key(key) => match PickerAct::for_default_key(*key).filter(|_| self.default_keys)
            {
                Some(act) => self.act(act),
                None => self.picker.key(*key, cx, now),
            },
            Input::KeyReleased(_)
            | Input::AppearanceChanged(_)
            | Input::CloseRequested
            | Input::FileDropped(_)
            | Input::FileHovered(_)
            | Input::FileHoverCancelled
            | Input::FocusGained
            | Input::FocusLost
            | Input::PointerButton(_)
            | Input::PointerLeft
            | Input::PointerMoved(_)
            | Input::Wheel(_) => self.handle_pointer(input, cx),
        };
        if !response.is_ignored() {
            cx.request_redraw();
        }
        response
    }

    /// Draws the picker in the frame's overlay, in `theme`'s colors: near
    /// the top of `area` over a dimmed backdrop across it, or hung from its
    /// anchor inside `area`.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let view = self.picker.view();
        self.icons_seen |= view.rows.iter().any(|shown| shown.row.icon.is_some());
        let geometry = self.geometry(frame, &view, area, theme);
        self.lines = geometry.lines;
        self.picker.set_lines(geometry.lines);
        let metrics = frame.cell_metrics();
        let scale_factor = frame.scale_factor();
        let stroke = logical(SEPARATOR_STROKE, scale_factor);
        if self.anchor.is_none() && self.placement == QuickPickPlacement::Window {
            draw_backdrop(frame, area, theme);
        }
        let popup = Popup::with_rect(geometry.popup);
        popup.draw(frame, theme);
        let content = popup.content(&theme.shape, scale_factor);
        let list = geometry.list;
        let separator_y = list.y + self.header_height(metrics.height, scale_factor);
        // The rows' bands reach as far past the list's text on the right as
        // they do on the left, but stop at the text beside a panel.
        let list_right = match geometry.panel.is_some() && geometry.panel_beside {
            true => list.x + list.width,
            false => list.x + list.width + (list.x - content.x),
        };
        let rows = Rect::new(
            content.x,
            separator_y + stroke,
            list_right - content.x,
            geometry.lines as f32 * row_slot(metrics.height, theme, scale_factor),
        );
        let grip_side = (GRIP_CELLS * metrics.height).round();
        let popup_rect = geometry.popup;
        let mut layout = PickLayout {
            grip: self.anchor.is_none().then(|| {
                Rect::new(
                    popup_rect.x + popup_rect.width - grip_side,
                    popup_rect.y + popup_rect.height - grip_side,
                    grip_side,
                    grip_side,
                )
            }),
            heading: Rect::new(
                popup_rect.x,
                list.y,
                popup_rect.width,
                Self::heading_height(metrics.height),
            ),
            line: metrics.height,
            popup: popup_rect,
            scale_factor,
            ..PickLayout::default()
        };
        frame.overlay(|frame| {
            let strip = popup.header_strip(
                &theme.shape,
                scale_factor,
                Self::heading_height(metrics.height),
            );
            self.draw_header(frame, &view, (strip, list), theme);
            // A boxed query's border already sets it off from the rows.
            if self.query_style == QueryStyle::Prompt {
                let separator = Rect::new(content.x, separator_y, list_right - content.x, stroke);
                frame.fill_rect_over_text(separator, theme.border);
            }
            let mut drawn = None;
            frame.clipped(rows, |frame| {
                drawn = Some(self.draw_rows(frame, &view, rows, theme))
            });
            (layout.rows, layout.buttons) = drawn.unwrap_or_default();
            if !view.hints.is_empty() {
                let pad = row_pad(theme, scale_factor);
                // At the foot of the box, the room whole rows leave over
                // between it and them, unless a panel is under the list.
                let under_rows = rows.y + rows.height + pad;
                let foot = content.y + content.height - pad - metrics.height;
                let top = match geometry.panel.is_some() && !geometry.panel_beside {
                    true => under_rows,
                    false => under_rows.max(foot),
                };
                let line = Rect::new(list.x, top, list.width, metrics.height);
                let keys = HintKeys {
                    defaults: self.default_keys,
                    named: &self.hint_keys,
                };
                draw_hints(frame, &view, line, keys, theme);
            }
            if let Some(panel) = geometry.panel {
                self.draw_panel(frame, panel, geometry.panel_beside, theme);
            }
            if let Some(menu) = &view.menu {
                let row = view
                    .selected
                    .and_then(|at| layout.rows.get(at).copied())
                    .unwrap_or(rows);
                let (menu_box, entries) = draw_menu(frame, menu, row, area, theme);
                layout.menu = Some(menu_box);
                layout.menu_rows = entries;
            }
        });
        // The button under the pointer is named once the pointer rests.
        let button = self.pointer.and_then(|point| {
            let buttons = layout.buttons.iter();
            buttons.rev().find(|(rect, ..)| rect.contains(point))
        });
        match button {
            Some((rect, _, _, title)) => self.tooltip.hover(*rect, title.as_str()),
            None => {
                self.tooltip.leave();
            }
        }
        self.tooltip.draw(frame, area, theme);
        self.layout = Some(layout);
    }

    /// Takes in what the source and the matcher sent, asking for a redraw
    /// when the view changed and emptying the panel when the selected row
    /// has nothing to preview, and keeps the rest for
    /// [`QuickPick::take_tick`], and blinks the query's caret. Returns
    /// when to tick again for work waiting on a pause or for the caret,
    /// whichever is first.
    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        let Tick {
            again,
            outcome,
            preview,
            redraw,
            saved,
        } = self.picker.tick(Instant::now());
        if matches!(preview, Some(PreviewUpdate::Clear)) {
            self.panel.clear();
        }
        if redraw {
            cx.request_redraw();
        }
        if outcome.is_some() || preview.is_some() || saved.is_some() {
            let later = QuickPickTick {
                outcome,
                preview,
                saved,
            };
            self.pending = Some(match self.pending.take() {
                Some(kept) => merge_ticks(kept, later),
                None => later,
            });
        }
        let caret = self.picker.field_mut().about_to_wait(cx);
        let tooltip = self.tooltip.about_to_wait(cx);
        again.into_iter().chain(caret).chain(tooltip).min()
    }
}

impl<O> Outcome for Reply<O> {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

/// How one row is drawn.
#[derive(Clone, Copy, Debug)]
struct RowLook {
    /// Whether some row on screen has an icon, so every label lines up
    /// after one.
    has_icons: bool,
    /// Whether the row is selected.
    lit: bool,
    /// Whether its buttons show: it is selected, or under the pointer.
    with_buttons: bool,
}

// ========================================================================
// Functions
// ========================================================================

/// A picker over the commands of `registry` that can run now, each
/// labelled `Group: Title` with its key hint, handing back the command's
/// id; titled, and its one action named, in `theme`'s words.
pub fn command_palette<M: Send + 'static>(
    waker: Waker<M>,
    registry: &CommandRegistry,
    theme: &Theme,
) -> QuickPick<&'static str> {
    let commands: Vec<_> = registry
        .commands()
        .filter(|command| registry.is_enabled(command.id))
        .collect();
    let rows = commands
        .iter()
        .zip(0..)
        .map(|(command, key)| {
            let row = Row::new(RowKey(key), format!("{}: {}", command.group, command.title));
            match registry.key_hint(command.id, &theme.strings) {
                Some(hint) => row.with_hint(hint),
                None => row,
            }
        })
        .collect();
    let ids: Vec<&'static str> = commands.iter().map(|command| command.id).collect();
    let source = FixedSource::new(
        "commands",
        theme.strings.command_palette.as_ref(),
        rows,
        theme.strings.picker_choose.as_ref(),
        move |row: RowKey| {
            usize::try_from(row.0)
                .ok()
                .and_then(|at| ids.get(at).copied())
        },
    );
    QuickPick::new(Picker::open(
        Box::new(source),
        PickerMemory::default(),
        waker,
    ))
}

/// `earlier` and what the tick after it took in, `later`, as one: the
/// first outcome and rows saved, and the latest preview.
fn merge_ticks<O, P>(
    earlier: QuickPickTick<O, P>,
    later: QuickPickTick<O, P>,
) -> QuickPickTick<O, P> {
    QuickPickTick {
        outcome: earlier.outcome.or(later.outcome),
        preview: later.preview.or(earlier.preview),
        saved: earlier.saved.or(later.saved),
    }
}

/// The room under a boxed query in lines `line` pixels high.
fn field_gap(line: f32) -> f32 {
    (line * FIELD_GAP_RATIO).round()
}

/// Draws the line naming the keys of what the picker offers now, at
/// `line`.
fn draw_hints(frame: &mut Frame, view: &SessionView, line: Rect, keys: HintKeys, theme: &Theme) {
    let mut hints = StyledText::new().with_ellipsis();
    for act in &view.hints {
        let word = match act {
            PickerAct::ActionMenu => &theme.strings.picker_actions,
            PickerAct::Save => &theme.strings.picker_save,
            PickerAct::TogglePanel => &theme.strings.picker_preview,
            PickerAct::Accept
            | PickerAct::Close
            | PickerAct::HistoryNext
            | PickerAct::HistoryPrevious
            | PickerAct::Next
            | PickerAct::PageDown
            | PickerAct::PageUp
            | PickerAct::Previous => continue,
        };
        if !hints.spans().is_empty() {
            hints.push(Span::new(HINT_SEPARATOR, QUIET_COLOR));
        }
        let key = match keys.named.get(act) {
            Some(named) => Some(named.clone()),
            None => keys
                .defaults
                .then(|| default_key(*act))
                .flatten()
                .map(|key| key.to_string()),
        };
        if let Some(key) = key {
            hints.push(Span::new(format!("{key} "), QUIET_COLOR));
        }
        hints.push(Span::new(word.as_ref(), QUIET_COLOR));
    }
    draw_styled_label(frame, theme, &hints, Point::new(line.x, line.y), line);
}

/// Draws `menu`, the action menu, hung from `row`, the selected row, inside
/// `area`; where its box and entries were drawn.
fn draw_menu(
    frame: &mut Frame,
    menu: &MenuView,
    row: Rect,
    area: Rect,
    theme: &Theme,
) -> (Rect, Vec<Rect>) {
    let metrics = frame.cell_metrics();
    let scale_factor = frame.scale_factor();
    let padding = metrics.width.round();
    let inset = Popup::inset(&theme.shape, scale_factor);
    let stroke = logical(SEPARATOR_STROKE, scale_factor);
    // The titles are labels, measured as they are drawn below.
    let widest = menu.actions.iter().map(|action| {
        let title = StyledText::plain(action.title.as_str(), LABEL_COLOR);
        styled_label_cells(frame, theme, &title)
    });
    let filter = text::width(PROMPT) + text::width(&menu.filter) + 1;
    let cols = widest.chain([filter]).max().unwrap_or(0).min(MENU_MAX_COLS);
    let width = (cols as f32 * metrics.width + 2.0 * (padding + inset)).ceil();
    let lines = 1 + menu.actions.len().max(1);
    let height = lines as f32 * metrics.height + stroke + 2.0 * inset;
    let anchor = Rect::new(row.x + 2.0 * padding, row.y, row.width, row.height);
    let popup = Popup::new(anchor, width, height, PopupSide::Below, area);
    let style = theme
        .shape
        .floating_box(theme.raised, theme.border, scale_factor);
    let content = popup.content(&theme.shape, scale_factor);
    let text_x = content.x + padding;
    let text_width = (content.width - 2.0 * padding).max(0.0);
    frame.draw_box(popup.rect(), &style);
    let line = Rect::new(text_x, content.y, text_width, metrics.height);
    let filter =
        StyledText::plain(PROMPT, PROMPT_COLOR).with(Span::new(menu.filter.as_str(), LABEL_COLOR));
    frame.draw_text(&filter, Point::new(text_x, content.y), line, theme);
    let separator = Rect::new(content.x, content.y + metrics.height, content.width, stroke);
    frame.fill_rect_over_text(separator, theme.border);
    let mut entries = Vec::new();
    for (at, action) in menu.actions.iter().enumerate() {
        let top = content.y + metrics.height + stroke + at as f32 * metrics.height;
        let band = Rect::new(content.x, top, content.width, metrics.height);
        if at == menu.selected {
            frame.draw_box(
                band,
                &theme.shape.highlight(theme.selected_row, scale_factor),
            );
        }
        let title = StyledText::plain(action.title.as_str(), LABEL_COLOR).with_ellipsis();
        let clip = Rect::new(text_x, top, text_width, metrics.height);
        draw_styled_label(frame, theme, &title, Point::new(text_x, top), clip);
        entries.push(band);
    }
    (popup.rect(), entries)
}

/// The first of `act`'s default keys, as a hint names it.
fn default_key(act: PickerAct) -> Option<Key> {
    act.default_keys()
        .first()
        .and_then(|spec| parse_key(spec).ok())
}

/// The cells a picker hung from an anchor takes: its widest row's label,
/// description, and hint, its heading and count (labels in `theme`'s
/// chrome font, measured as they are drawn), or its prompt.
fn anchored_cols(frame: &Frame, theme: &Theme, view: &SessionView, has_icons: bool) -> usize {
    let icon = usize::from(has_icons) * ICON_CELLS;
    let rows = view.rows.iter().map(|shown| {
        let row = &shown.row;
        let description = match row.description.is_empty() {
            true => 0,
            false => DESCRIPTION_GAP_CELLS + text::width(&row.description),
        };
        let hint = match row.hint.is_empty() {
            true => 0,
            false => HINT_GAP_CELLS + text::width(&row.hint),
        };
        let line = icon + text::width(&row.label) + description + hint;
        line.max(icon + text::width(&row.detail))
    });
    let label =
        |text: String| styled_label_cells(frame, theme, &StyledText::plain(text, QUIET_COLOR));
    // The widest count it can show: every match, the last of them at.
    let count = label(format!("{0}/{0}", view.total));
    let header = label(view.title.clone()) + HINT_GAP_CELLS + count;
    rows.chain([header, text::width(PROMPT) + 1])
        .max()
        .unwrap_or(0)
}

/// How far a row's text sits into its band: the shape's row padding, in
/// physical pixels on a display scaled by `scale_factor`.
fn row_pad(theme: &Theme, scale_factor: f64) -> f32 {
    (f32::from(theme.shape.row_padding) * scale_factor as f32).round()
}

/// The height each line of the rows takes in lines `line` pixels high: the
/// line with the shape's row padding above and below.
fn row_slot(line: f32, theme: &Theme, scale_factor: f64) -> f32 {
    line + 2.0 * row_pad(theme, scale_factor)
}

/// The lines of rows a picker hung from an anchor shows: as many as all its
/// rows take, up to [`VISIBLE_LINES`], so filtering never shrinks it.
fn anchored_lines(view: &SessionView) -> usize {
    let row_lines = view.rows.first().map_or(1, |shown| shown.row.lines());
    (view.total * row_lines).clamp(1, VISIBLE_LINES)
}

/// Where an icon is drawn on the line at `top`, from `x`: a square
/// [`ICON_SHARE`] of the line tall, in its middle.
fn icon_rect(x: f32, top: f32, line_height: f32) -> Rect {
    let size = (line_height * ICON_SHARE).round();
    Rect::new(x, (top + (line_height - size) / 2.0).round(), size, size)
}

/// `text` with the chars at `positions` (char indices, in order) in the
/// match color and bold, the rest in `color`.
fn highlighted(text: &str, positions: &[usize], color: TextColor) -> StyledText {
    let mut styled = StyledText::new();
    let mut run = String::new();
    let mut run_lit = false;
    for (at, c) in text.chars().enumerate() {
        let lit = positions.binary_search(&at).is_ok();
        if lit != run_lit && !run.is_empty() {
            styled.push(styled_run(std::mem::take(&mut run), run_lit, color));
        }
        run_lit = lit;
        run.push(c);
    }
    if !run.is_empty() || styled.spans().is_empty() {
        styled.push(styled_run(run, run_lit, color));
    }
    styled
}

/// `text` drawn in `runs` of `(characters, color)`, the characters past
/// them in `rest`, with the chars at `positions` lit in the match color.
fn colored(text: &str, positions: &[usize], runs: &[(usize, Rgb)], rest: TextColor) -> StyledText {
    let mut colors = runs
        .iter()
        .flat_map(|&(count, color)| std::iter::repeat_n(TextColor::Rgba(Rgba::from(color)), count));
    let mut styled = StyledText::new();
    let mut run = String::new();
    let mut run_look = None;
    for (at, c) in text.chars().enumerate() {
        let lit = positions.binary_search(&at).is_ok();
        let color = colors.next().unwrap_or(rest);
        let look = (lit, color);
        if run_look.is_some_and(|before| before != look) && !run.is_empty() {
            let (lit, color) = run_look.unwrap_or(look);
            styled.push(styled_run(std::mem::take(&mut run), lit, color));
        }
        run_look = Some(look);
        run.push(c);
    }
    if let Some((lit, color)) = run_look {
        styled.push(styled_run(run, lit, color));
    }
    styled
}

/// A run of `text`: lit in the match color and bold, or in `color`.
fn styled_run(text: String, lit: bool, color: TextColor) -> Span {
    match lit {
        true => Span::new(text, MATCH_COLOR).bold(),
        false => Span::new(text, color),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::offscreen;
    use crate::base::key::parse_key_sequence;
    use crate::input::event::ButtonInput;
    use crate::testing::{TestWindow, drawn_text};
    use crate::widgets::picker::{Action, Feed, MatchOn, Matching, Resume, Source};

    /// A source of fixed rows, each running `open` and `kill`, and handing
    /// back ten times its key, plus one for `kill`.
    struct Rows(Vec<Row>);

    impl Source<u64> for Rows {
        fn id(&self) -> &'static str {
            "rows"
        }

        fn title(&self) -> String {
            "Rows".to_string()
        }

        fn matching(&self) -> Matching {
            Matching::Words(MatchOn::LABEL)
        }

        fn resume(&self) -> Resume {
            Resume::Reread
        }

        fn feed(&mut self, _query: &str) -> Feed {
            Feed::Fixed(std::mem::take(&mut self.0))
        }

        fn actions(&self, _row: RowKey) -> Vec<Action> {
            ["open", "kill"]
                .into_iter()
                .map(|id| Action {
                    button: None,
                    id: ActionId(id),
                    title: id.to_string(),
                })
                .collect()
        }

        fn run(&mut self, action: ActionId, row: RowKey, _query: &str) -> Option<u64> {
            Some(row.0 * 10 + u64::from(action == ActionId("kill")))
        }
    }

    /// The view of a picker over rows labelled `labels`, keyed by their
    /// places, its rows taken in.
    fn view(labels: &[&str]) -> QuickPick<u64> {
        let rows = labels
            .iter()
            .zip(0..)
            .map(|(label, key)| Row::new(RowKey(key), *label))
            .collect();
        let picker = Picker::synchronous(Box::new(Rows(rows)), PickerMemory::default());
        let mut pick = QuickPick::new(picker);
        pick.about_to_wait(&mut TestWindow::new(800, 600).cx());
        pick
    }

    /// `pick` drawn in `frame`, as big as the frame.
    fn draw(pick: &mut QuickPick<u64>, frame: &mut Frame) {
        let bounds = frame.bounds();
        pick.draw(frame, bounds, &Theme::dark());
    }

    /// The replies to `pick` of `inputs`, one after the other, the picker
    /// ticked after each as an app's event loop does.
    fn send(pick: &mut QuickPick<u64>, inputs: &[Input]) -> Vec<Reply<u64>> {
        let mut window = TestWindow::new(800, 600);
        inputs
            .iter()
            .map(|input| {
                let reply = window.send(input, |input, cx| pick.handle(input, cx));
                pick.about_to_wait(&mut window.cx());
                reply
            })
            .collect()
    }

    /// The inputs of typing `keys`, in misty's key notation.
    fn keys(keys: &str) -> Vec<Input> {
        parse_key_sequence(keys)
            .expect("keys")
            .into_iter()
            .map(Input::Key)
            .collect()
    }

    /// A left press at `x`, `y`.
    fn press(x: f32, y: f32) -> Input {
        Input::PointerButton(ButtonInput::press(PointerButton::Left, Point::new(x, y)))
    }

    #[test]
    fn test_the_querys_caret_blinks_with_no_work_waiting() {
        let mut pick = view(&["apple"]);
        assert_eq!(
            pick.picker.tick(Instant::now()).again,
            None,
            "no work waits"
        );
        let next = pick.about_to_wait(&mut TestWindow::new(800, 600).cx());
        assert!(next.is_some(), "the caret's next blink");
    }

    #[test]
    fn test_ticks_that_take_in_only_rows_hand_the_app_nothing() {
        let mut pick = view(&["apple", "banana"]);
        send(&mut pick, &keys("a"));
        assert!(
            pick.take_tick().is_none(),
            "rows came in, nothing to carry out"
        );
    }

    #[test]
    fn test_ticks_not_taken_keep_the_first_outcome_and_the_latest_preview() {
        let tick = |outcome: Option<u64>, preview: Option<u64>| QuickPickTick::<u64, u64> {
            outcome,
            preview: preview.map(PreviewUpdate::Show),
            saved: None,
        };
        let merged = merge_ticks(tick(Some(1), Some(1)), tick(None, Some(2)));
        assert_eq!(merged.outcome, Some(1), "an outcome is never lost");
        assert_eq!(merged.preview, Some(PreviewUpdate::Show(2)));
        let merged = merge_ticks(tick(None, Some(1)), tick(Some(2), None));
        assert_eq!(merged.outcome, Some(2));
        assert_eq!(merged.preview, Some(PreviewUpdate::Show(1)), "kept");
    }

    #[test]
    fn test_typing_filters_and_enter_runs_the_selected_rows_first_action() {
        let mut pick = view(&["apple", "banana", "plum"]);
        send(&mut pick, &keys("a n"));
        assert_eq!(pick.picker().view().matched, 1, "only banana holds an");
        assert_eq!(send(&mut pick, &keys("RET")), [Reply::Run(10)]);
    }

    #[test]
    fn test_the_action_menu_opens_on_f4_and_runs_the_entry_clicked() {
        let mut frame = offscreen::plain_frame(800, 600);
        let mut pick = view(&["apple"]);
        send(&mut pick, &keys("<f4>"));
        draw(&mut pick, &mut frame);
        let layout = pick.layout.clone().expect("drawn");
        assert_eq!(layout.menu_rows.len(), 2, "open and kill");
        let kill = layout.menu_rows[1];
        assert_eq!(
            send(&mut pick, &[press(kill.x + 2.0, kill.y + 2.0)]),
            [Reply::Run(1)]
        );
    }

    #[test]
    fn test_a_boxed_query_lowers_the_rows_by_its_box_and_gap_and_they_still_take_clicks() {
        let mut frame = offscreen::plain_frame(800, 600);
        let line = frame.cell_metrics().height;
        // How far below the heading the first row starts.
        let mut rows_below = |style: QueryStyle| {
            let mut pick = view(&["apple", "banana"]).with_query_style(style);
            draw(&mut pick, &mut frame);
            let layout = pick.layout.clone().expect("drawn");
            (layout.rows[0].y - layout.heading.y, pick, layout.rows[1])
        };
        let (prompt, _, _) = rows_below(QueryStyle::Prompt);
        let (field, mut pick, banana) = rows_below(QueryStyle::Field);
        let top = logical(FIELD_TOP, frame.scale_factor());
        let boxed = (line * FIELD_QUERY_RATIO).round() + field_gap(line) + top;
        assert_eq!(field - prompt, boxed - line);
        let on_banana = press(banana.x + 4.0, banana.y + 4.0);
        assert_eq!(send(&mut pick, &[on_banana]), [Reply::Run(10)]);
    }

    #[test]
    fn test_rows_take_the_shapes_row_padding_and_the_grid_sets_them_close() {
        let mut frame = offscreen::plain_frame(800, 600);
        let line = frame.cell_metrics().height;
        let mut row_height = |theme: &Theme| {
            let mut pick = view(&["apple", "banana"]);
            let bounds = frame.bounds();
            pick.draw(&mut frame, bounds, theme);
            pick.layout.clone().expect("drawn").rows[0].height
        };
        let modern = Theme::dark();
        let padding = f32::from(modern.shape.row_padding);
        assert_eq!(row_height(&modern), line + 2.0 * padding);
        assert_eq!(row_height(&Theme::dark().with_grid_look()), line);
    }

    #[test]
    fn test_a_picker_given_a_window_share_takes_it_in_the_middle_grown_to_a_whole_row() {
        let mut frame = offscreen::plain_frame(800, 600);
        let slot = row_slot(frame.cell_metrics().height, &Theme::dark(), 1.0);
        let mut pick = view(&["apple", "banana"]).with_window_share(0.7, 0.6);
        draw(&mut pick, &mut frame);
        let popup = popup(&pick);
        assert_eq!(popup.width, 560.0);
        let grown = popup.height - 360.0;
        assert!(
            grown > 0.0 && grown < slot,
            "grown to the row that partly fit"
        );
        let middle = 120.0 + ((360.0 - popup.height) / 2.0).round();
        assert_eq!((popup.x, popup.y), (120.0, middle), "centered both ways");
    }

    #[test]
    fn test_moving_down_a_list_of_more_lines_than_the_default_keeps_to_the_drawn_rows() {
        // Each move told the session the list showed the default twelve
        // lines and each frame the lines it drew, so the session fetched
        // its rows twice a key and scrolled a row short of the bottom.
        let labels: Vec<String> = (0..60).map(|at| format!("row {at}")).collect();
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        let mut frame = offscreen::plain_frame(800, 900);
        let mut pick = view(&labels).with_window_share(0.7, 0.9);
        draw(&mut pick, &mut frame);
        let lines = pick.lines;
        assert!(lines > VISIBLE_LINES, "{lines} lines");
        for _ in 0..lines + 3 {
            pick.act(PickerAct::Next);
            pick.about_to_wait(&mut TestWindow::new(800, 900).cx());
            draw(&mut pick, &mut frame);
        }
        let shown = pick.picker().view();
        assert_eq!(shown.selected, Some(lines - 1), "on the last row drawn");
    }

    #[test]
    fn test_a_row_s_band_sits_evenly_in_a_box_of_any_width() {
        // A box whose width is no whole number of columns left the remainder
        // all on the right of the rows, past a column of padding the left
        // did not have.
        for share in [0.7, 0.71, 0.733] {
            let mut frame = offscreen::plain_frame(800, 600);
            let mut pick = view(&["apple", "banana"]).with_window_share(share, 0.6);
            draw(&mut pick, &mut frame);
            let popup = popup(&pick);
            let row = pick.layout.clone().expect("drawn").rows[0];
            let left = row.x - popup.x;
            let right = popup.x + popup.width - (row.x + row.width);
            assert!((left - right).abs() < 1.0, "{left} and {right} at {share}");
        }
    }

    #[test]
    fn test_the_icon_column_stays_when_the_query_filters_the_icons_out() {
        let rows = vec![
            Row::new(RowKey(0), "apple").with_icon("file"),
            Row::new(RowKey(1), "banana"),
        ];
        let picker = Picker::synchronous(Box::new(Rows(rows)), PickerMemory::default());
        let mut pick = QuickPick::new(picker);
        pick.about_to_wait(&mut TestWindow::new(800, 600).cx());
        let mut frame = offscreen::plain_frame(800, 600);
        draw(&mut pick, &mut frame);
        send(&mut pick, &keys("b a n"));
        draw(&mut pick, &mut frame);
        let view = pick.picker.view();
        assert!(
            view.rows.iter().all(|shown| shown.row.icon.is_none()),
            "only banana left"
        );
        assert!(
            pick.icon_column(&view),
            "the column kept, so banana does not jump left"
        );
    }

    #[test]
    fn test_a_click_on_a_row_runs_it_and_outside_closes() {
        let mut frame = offscreen::plain_frame(800, 600);
        let mut pick = view(&["apple", "banana"]);
        draw(&mut pick, &mut frame);
        let banana = pick.layout.clone().expect("drawn").rows[1];
        let on_banana = press(banana.x + 4.0, banana.y + 4.0);
        assert_eq!(send(&mut pick, &[on_banana]), [Reply::Run(10)]);
        assert_eq!(send(&mut pick, &[press(2.0, 590.0)]), [Reply::Close]);
    }

    #[test]
    fn test_the_hints_line_names_the_keys_the_picker_offers() {
        let mut frame = offscreen::plain_frame(800, 600);
        let mut pick = view(&["apple"]);
        draw(&mut pick, &mut frame);
        let text = drawn_text(&frame).join("");
        assert!(text.contains("<f4> "), "{text}");
        assert!(text.contains("actions"), "{text}");
        assert!(text.contains("save"), "{text}");
        assert!(!text.contains("preview"), "no panel to show: {text}");
    }

    #[test]
    fn test_an_app_with_its_own_keymap_names_its_keys_in_the_hints_line() {
        let mut frame = offscreen::plain_frame(800, 600);
        let mut pick = view(&["apple"]).without_default_keys();
        pick.set_hint_key(PickerAct::Save, "F3");
        draw(&mut pick, &mut frame);
        let text = drawn_text(&frame).join("");
        assert!(text.contains("F3 "), "{text}");
        assert!(!text.contains("<f4>"), "no default named: {text}");
    }

    #[test]
    fn test_an_app_with_its_own_keymap_turns_the_defaults_off() {
        let mut pick = view(&["apple", "banana"]).without_default_keys();
        send(&mut pick, &keys("C-n"));
        assert_eq!(pick.picker().view().selected, Some(0), "C-n is no act");
        assert_eq!(pick.act(PickerAct::Next), Reply::Consumed);
        assert_eq!(pick.act(PickerAct::Accept), Reply::Run(10));
    }

    #[test]
    fn test_an_anchored_picker_hangs_under_its_anchor_as_tall_as_its_rows() {
        let mut frame = offscreen::plain_frame(800, 600);
        let anchor = Rect::new(40.0, 100.0, 300.0, 20.0);
        let mut pick = view(&["Copy", "Rename", "Delete"]).with_anchor(anchor);
        draw(&mut pick, &mut frame);
        let layout = pick.layout.clone().expect("drawn");
        assert_eq!(
            (layout.popup.x, layout.popup.y),
            (40.0, 120.0),
            "under the row"
        );
        assert_eq!(layout.rows.len(), 3);
        assert!(
            layout.popup.width < 300.0,
            "as wide as its widest: {:?}",
            layout.popup
        );
        send(&mut pick, &keys("r e"));
        draw(&mut pick, &mut frame);
        let popup = pick.layout.expect("drawn").popup;
        assert_eq!(popup, layout.popup, "filtering keeps its size");
    }

    /// The box `pick` was last drawn in.
    fn popup(pick: &QuickPick<u64>) -> Rect {
        pick.layout.as_ref().expect("drawn").popup
    }

    /// The pointer moved to `x`, `y`.
    fn moved(x: f32, y: f32) -> Input {
        Input::PointerMoved(Point::new(x, y))
    }

    /// The left button let go at `x`, `y`.
    fn release(x: f32, y: f32) -> Input {
        let press = ButtonInput::press(PointerButton::Left, Point::new(x, y));
        Input::PointerButton(ButtonInput {
            pressed: false,
            ..press
        })
    }

    #[test]
    fn test_a_docked_picker_spans_its_panes_bottom_and_a_filling_one_its_rect() {
        let mut frame = offscreen::plain_frame(800, 600);
        let pane = Rect::new(100.0, 50.0, 500.0, 500.0);
        let mut pick = view(&["a", "b"]);
        pick.set_placement(QuickPickPlacement::DockedBottom(pane));
        draw(&mut pick, &mut frame);
        let docked = popup(&pick);
        assert_eq!((docked.x, docked.width), (pane.x, pane.width));
        assert_eq!(
            docked.y + docked.height,
            pane.y + pane.height,
            "on its bottom"
        );
        let card = Rect::new(200.0, 100.0, 300.0, 250.0);
        pick.set_placement(QuickPickPlacement::Filling(card));
        draw(&mut pick, &mut frame);
        assert_eq!(popup(&pick), card);
        assert!(pick.picker().view().rows.len() <= 2);
    }

    #[test]
    fn test_the_heading_moves_the_picker_and_the_grip_resizes_it() {
        let mut frame = offscreen::plain_frame(800, 600);
        let mut pick = view(&["a", "b"]);
        draw(&mut pick, &mut frame);
        let before = popup(&pick);
        let heading = pick.layout.as_ref().expect("drawn").heading;
        let (x, y) = (heading.x + 4.0, heading.y + 2.0);
        send(
            &mut pick,
            &[
                press(x, y),
                moved(x + 30.0, y + 20.0),
                release(x + 30.0, y + 20.0),
            ],
        );
        draw(&mut pick, &mut frame);
        let moved_box = popup(&pick);
        assert_eq!(
            (moved_box.x, moved_box.y),
            (before.x + 30.0, before.y + 20.0)
        );
        let grip = pick.layout.as_ref().expect("drawn").grip.expect("a grip");
        let (x, y) = (grip.x + 1.0, grip.y + 1.0);
        send(
            &mut pick,
            &[
                press(x, y),
                moved(x + 40.0, y + 60.0),
                release(x + 40.0, y + 60.0),
            ],
        );
        draw(&mut pick, &mut frame);
        let resized = popup(&pick);
        let slot = row_slot(frame.cell_metrics().height, &Theme::dark(), 1.0);
        let asked = moved_box.height + 60.0;
        assert_eq!(resized.width, moved_box.width + 40.0);
        assert!(
            resized.height > asked && resized.height - asked < slot,
            "grown to the row that partly fit"
        );
        assert!(
            pick.picker().view().rows.len() <= 2,
            "only two rows to show however tall"
        );
    }

    #[test]
    fn test_a_description_takes_its_color_runs_and_keeps_its_matches_lit() {
        let (red, blue) = (Rgb::new(255, 0, 0), Rgb::new(0, 0, 255));
        let styled = colored("let x", &[4], &[(3, red), (1, blue)], QUIET_COLOR);
        let runs: Vec<(&str, TextColor, bool)> = styled
            .spans()
            .iter()
            .map(|span| (span.text.as_str(), span.color, span.bold))
            .collect();
        assert_eq!(
            runs,
            [
                ("let", TextColor::Rgba(Rgba::from(red)), false),
                (" ", TextColor::Rgba(Rgba::from(blue)), false),
                ("x", MATCH_COLOR, true),
            ]
        );
    }

    #[test]
    fn test_matched_runs_are_lit_and_the_rest_left_plain() {
        let styled = highlighted("picker", &[0, 1, 4], LABEL_COLOR);
        let runs: Vec<(&str, bool)> = styled
            .spans()
            .iter()
            .map(|span| (span.text.as_str(), span.bold))
            .collect();
        assert_eq!(
            runs,
            [("pi", true), ("ck", false), ("e", true), ("r", false)]
        );
        assert_eq!(highlighted("é漢x", &[1], LABEL_COLOR).spans()[1].text, "漢");
        assert_eq!(highlighted("", &[], LABEL_COLOR).spans().len(), 1);
    }
}
