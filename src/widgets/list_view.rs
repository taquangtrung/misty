//! [`List`]: a virtualized list of one-line rows. The app supplies the rows
//! through [`ListSource`], asked only for the rows on screen, so a list of
//! a million rows costs no more to draw than one of twenty. A row is plain
//! text unless the source gives a [`RowContent`] with an icon, styled
//! text, and badges ([`ListSource::row_content`]). Rows are set on the
//! cell grid, so columns in them line up, unless the list is built
//! [`List::with_proportional_font`] for rows of prose.
//!
//! Keys, through the lent [`Widget::handle`](crate::Widget::handle), are the ones every list-like
//! widget shares, as Vim moves over lines: Up and Down (or `j` and `k`,
//! `C-p` and `C-n`) move the cursor a row, `C-f` and `C-b` (Page Down and
//! Page Up) a screen less two rows, `C-d` and `C-u` half a screen, `C-e`
//! and `C-y` scroll a row leaving the cursor, `H`, `M`, and `L` go to the
//! top, middle, and bottom of the screen, `g g` and `G` (Home and End) to
//! the ends, `z t`, `z z`, and `z b` scroll the cursor's row to the top,
//! middle, or bottom, and a count before a motion repeats it or names the
//! row (`5j`, `10G`); Space toggles the cursor row's selection, Shift
//! with Down, Up, Page Down, Page Up, Home, or End extends the selection
//! from its anchor to the cursor, and Enter activates it. [`List::keys`] tells what count or prefix is pending, for
//! a status bar or a which-key popup. An app that wants
//! its own keys drives the [`ListState`] methods
//! ([`ListState::move_cursor`], [`ListState::page`],
//! [`ListState::set_cursor`], [`ListState::toggle_selected`]) through
//! [`List::state_mut`] instead. The pointer is handled by
//! the lent [`Widget::handle`](crate::Widget::handle):
//! a click moves the cursor, Ctrl+click toggles a row's selection,
//! Shift+click selects the rows from the anchor (the row last clicked or
//! toggled) to the row clicked, on top of what was selected then, a double
//! click activates a row, and the wheel scrolls. A drag that starts on
//! empty space below the last row, or anywhere with Alt held, draws a
//! rubber band instead and selects every row it touches (on top of the
//! selection with Ctrl, else in place of it); held near the top or bottom
//! edge, it scrolls the list a row at a time. Dragging a row drags it:
//! a line marks the gap it would land in while the pointer is over the
//! list, its text follows the pointer, and the release answers
//! [`ListResponse::Dropped`] for the app to move the row (or the selection
//! it belongs to) or hand it to another drop target. As in Vim, a press
//! of any button or the wheel over the list drops a pending count or
//! prefix, as does the window losing focus.
//!
//! The rows glide to a new scroll position rather than jump; call the lent
//! list's [`Widget::about_to_wait`] so each step is drawn (and a rubber
//! band held at an edge scrolls on). While motion is reduced
//! ([`Frame::reduced_motion`]) they jump.
//!
//! [`List::row_rect`] and [`List::visible_range`] tell where rows were
//! drawn, to place an inline rename field over one or hint labels over
//! those in sight.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::ops::Range;
use std::time::{Duration, Instant};

use accesskit::{Action, Node, Role};

use super::super::app::context::EventCx;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::{Lent, Outcome, Widget};
use super::super::base::box_style::BoxStyle;
use super::super::base::clock;
use super::super::base::color::Rgba;
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::{Key, KeyScheme, Mods};
use super::super::base::list::ListState;
use super::super::base::theme::Theme;
use super::super::input::click::is_double_click;
use super::super::input::event::{ButtonInput, Input, PointerButton, WheelDelta};
use super::super::render::icon::logical;
use super::super::render::paragraph::TextAlign;
use super::band::{RubberBand, band_rect, draw_band};
use super::drag::{Drag, DragResponse, Dropped, draw_drop_target};
use super::glide::{RowGlide, rows_in_sight};
use super::list_keys::{CountedKey, ListKey, ListKeys, Motion, Read};
use super::row_content::{RowContent, RowFont, draw_content_in};

// ========================================================================
// Constants
// ========================================================================

/// The rows one notch of a mouse wheel scrolls.
const WHEEL_ROWS: f32 = 3.0;

/// The width of the accent bar marking the cursor's row, and the
/// thickness of the line marking where a dragged row would land, in
/// logical pixels.
const CURSOR_BAR: f32 = 2.0;

/// How near the top or bottom edge a held rubber band scrolls the list,
/// in logical pixels.
const AUTO_SCROLL_EDGE: f32 = 16.0;

/// How long a rubber band held at an edge waits between rows of scroll.
const AUTO_SCROLL_INTERVAL: Duration = Duration::from_millis(60);

// ========================================================================
// Data Structures
// ========================================================================

/// A list of one-line rows with a cursor and a selection. It keeps the
/// [`ListState`], the pointer state between events, and the layout of the
/// last frame it drew, which its hit testing uses.
#[derive(Clone, Debug, Default)]
pub struct List {
    /// The rubber band being drawn, if any.
    band: Option<RowBand>,
    /// A row being dragged.
    drag: Drag<usize>,
    /// The scroll as drawn, in rows, gliding to the state's scroll.
    glide: RowGlide,
    /// The rows that head a group of rows, such as a tree's sections: the
    /// cursor steps over them, a click on one does nothing, and they are
    /// never selected or highlighted. None unless a widget built on the
    /// list says.
    headers: BTreeSet<usize>,
    /// Whether a press on a row never starts a drag of it.
    is_drag_off: bool,
    /// Whether screen readers (and hint labels) are told each row can be
    /// clicked, which activates it.
    is_rows_clickable: bool,
    /// Whether a dragged row is dropped onto the row under it, which is
    /// marked, rather than into the gap between two rows.
    is_drop_onto: bool,
    /// The font the rows' text is set in.
    font: RowFont,
    /// The count and prefix typed toward a motion.
    keys: ListKeys,
    /// When and where the last press on a row was, for double clicks.
    last_press: Option<(Instant, Point)>,
    /// Where the rows were in the last frame drawn, or `None` before it.
    layout: Option<ListLayout>,
    /// Where the pointer is, or `None` when it is outside the window.
    pointer: Option<Point>,
    /// Wheel movement in pixels not yet turned into a whole row of scroll.
    scroll_rest: f32,
    /// The cursor, selection, and scroll.
    state: ListState,
}

/// A row dropped after a drag.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowDrop {
    /// The gap it was dropped in while over the list: the index of the row
    /// it now comes before (the row count for after the last row). `None`
    /// when dropped off the list, where the app's own drop targets look at
    /// `position`.
    pub before: Option<usize>,
    /// Where it was dropped, in physical pixels.
    pub position: Point,
    /// The row dragged.
    pub row: usize,
}

/// A rubber band being drawn over a list's rows, which scrolls the list
/// while held at an edge.
#[derive(Clone, Debug, PartialEq)]
struct RowBand {
    /// The band, started across in physical pixels on screen and down in
    /// pixels from the top of row 0.
    band: RubberBand,
    /// When a band held at an edge scrolls the next row, or `None` while
    /// it is away from the edges.
    next_step: Option<Instant>,
}

/// Where a list's rows were drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ListLayout {
    /// The whole list.
    area: Rect,
    /// The row at the top edge as drawn, with the share of it scrolled
    /// out of sight.
    offset: f32,
    /// How many leading rows stay at the top while the others scroll
    /// under them.
    pinned: usize,
    /// The height of one row.
    row_height: f32,
    /// How many whole rows fit.
    rows: usize,
    /// Physical pixels per logical pixel.
    scale_factor: f64,
}

/// The rows [`List::draw_rows`] draws.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RowsToDraw {
    /// Where.
    pub(crate) area: Rect,
    /// How many rows there are.
    pub(crate) count: usize,
    /// The time the glide is drawn at.
    pub(crate) now: Instant,
    /// How many leading rows stay at the top while the others scroll
    /// under them; 0 for none.
    pub(crate) pinned: usize,
    /// The height of one row, in physical pixels.
    pub(crate) row_height: f32,
}

/// What became of an input offered to a [`List`].
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum ListResponse {
    /// This row was double-clicked or Enter was pressed on it: open it, or
    /// whatever the app's activation means.
    Activated(usize),
    /// A click or a key moved the cursor to this row.
    CursorMoved(usize),
    /// A dragged row was dropped.
    Dropped(RowDrop),
    /// The list used the event; the app should not act on it again.
    Handled,
    /// The event is not the list's; the app handles it as usual.
    Ignored,
}

// ========================================================================
// Traits
// ========================================================================

/// The rows of a [`List`], supplied by the app.
pub trait ListSource {
    /// How many rows there are.
    fn row_count(&self) -> usize;

    /// The text of `row`, which is less than [`ListSource::row_count`].
    /// Asked only for rows on screen, each frame, and for what screen
    /// readers and a dragged row's label say.
    fn row_text(&self, row: usize) -> Cow<'_, str>;

    /// What `row` shows: an icon, styled text, and badges. Asked only for
    /// rows on screen, each frame. By default [`ListSource::row_text`] in
    /// the foreground color.
    fn row_content(&self, row: usize) -> RowContent {
        RowContent::plain(self.row_text(row))
    }
}

/// Any slice of text is a list's rows, one string a row: a `Vec<String>` or
/// a `&[&str]` needs no type of its own.
impl<T: AsRef<str>> ListSource for [T] {
    fn row_count(&self) -> usize {
        self.len()
    }

    fn row_text(&self, row: usize) -> Cow<'_, str> {
        Cow::Borrowed(self[row].as_ref())
    }
}

/// A `Vec` of text is a list's rows, as its slice is.
impl<T: AsRef<str>> ListSource for Vec<T> {
    fn row_count(&self) -> usize {
        self.len()
    }

    fn row_text(&self, row: usize) -> Cow<'_, str> {
        Cow::Borrowed(self[row].as_ref())
    }
}

// ========================================================================
// List Implementations
// ========================================================================

impl List {
    /// This widget taking the Vim and Emacs keys, for misty's tests of
    /// them.
    #[cfg(test)]
    pub(crate) fn with_vim_and_emacs(mut self) -> Self {
        self.keys.set_scheme(KeyScheme::VIM_AND_EMACS);
        self
    }

    /// An empty list; it reads the rows from the source it is lent.
    pub fn new() -> Self {
        Self::default()
    }

    /// This list with its rows' text, icons' gaps, and badges laid out in
    /// the proportional font, each character as wide as its shape, for
    /// rows of prose (a dictionary's headwords, search results with
    /// snippets) rather than columns to line up. Rows are a line of the
    /// UI font tall ([`Frame::ui_line_height`]), a cell while the
    /// interface font is the window font's size.
    pub fn with_proportional_font(self) -> Self {
        Self {
            font: RowFont::Proportional,
            ..self
        }
    }

    /// The cursor, selection, and scroll.
    pub fn state(&self) -> &ListState {
        &self.state
    }

    /// The cursor, selection, and scroll, to move and change. Ask for a
    /// redraw after changing them.
    pub fn state_mut(&mut self) -> &mut ListState {
        &mut self.state
    }

    /// The drag of a row, to light up drop targets while it runs: its
    /// [`Drag::payload`] is the row dragged.
    pub fn drag(&self) -> &Drag<usize> {
        &self.drag
    }

    /// The count and prefix typed toward a motion, to show in a status bar
    /// or a [`WhichKey`](crate::widgets::WhichKey) popup.
    pub fn keys(&self) -> &ListKeys {
        &self.keys
    }

    /// The count and prefix typed toward a motion, to drop them with
    /// [`ListKeys::reset`] when the list loses the keyboard.
    pub fn keys_mut(&mut self) -> &mut ListKeys {
        &mut self.keys
    }

    /// Reacts to `key`, as the module documentation lists. Hand it the
    /// keys of the window while the list has the focus, and ask for a
    /// redraw when it answers anything but [`ListResponse::Ignored`]: a
    /// count or prefix typed is [`ListResponse::Handled`], so a status bar
    /// showing [`List::keys`] follows it.
    pub(crate) fn handle_key(&mut self, key: Key) -> ListResponse {
        match self.read_key(key) {
            Read::Consumed => ListResponse::Handled,
            Read::Ignored => ListResponse::Ignored,
            Read::Key(CountedKey { count, key }) => self.run_key(key, count),
        }
    }

    /// Reads keys by `scheme` from now on, as [`List::handle`] does with
    /// the app's: for a widget built on the list.
    pub(crate) fn set_key_scheme(&mut self, scheme: KeyScheme) {
        self.keys.set_scheme(scheme);
    }

    /// Reads `key` toward a list key, for a widget built on the list that
    /// reads some list keys its own way.
    pub(crate) fn read_key(&mut self, key: Key) -> Read {
        self.keys.read(key)
    }

    /// Runs `key` with the `count` typed before it, as
    /// [`List::handle_key`] does.
    pub(crate) fn run_key(&mut self, key: ListKey, count: Option<usize>) -> ListResponse {
        let cursor = self.state.cursor();
        match key {
            ListKey::Activate => match self.state.count() {
                0 => ListResponse::Handled,
                _ => ListResponse::Activated(cursor),
            },
            ListKey::Left | ListKey::LineEnd | ListKey::LineStart | ListKey::Right => {
                ListResponse::Ignored
            }
            ListKey::Extend(motion) => {
                if self.state.anchor().is_none() {
                    self.state.set_anchor(Some(cursor));
                }
                self.move_cursor(motion, count);
                self.state.select_to(self.state.cursor());
                self.deselect_headers();
                match self.state.cursor() == cursor {
                    true => ListResponse::Handled,
                    false => ListResponse::CursorMoved(self.state.cursor()),
                }
            }
            ListKey::Move(motion) => {
                if !self.state.is_ranging() {
                    self.state.set_anchor(None);
                }
                self.move_cursor(motion, count);
                match self.state.cursor() == cursor {
                    true => ListResponse::Handled,
                    false => ListResponse::CursorMoved(self.state.cursor()),
                }
            }
            ListKey::Toggle => {
                self.state.toggle_selected(cursor);
                self.deselect_headers();
                if !self.state.is_ranging() {
                    self.state.set_anchor(Some(cursor));
                }
                ListResponse::Handled
            }
        }
    }

    /// Makes `headers` the rows that head a group, for a widget built on
    /// the list: the cursor steps off one it is on (to the next row, or the
    /// one before when none follows) and they leave the selection.
    pub(crate) fn set_headers(&mut self, headers: BTreeSet<usize>) {
        self.headers = headers;
        self.settle_cursor(self.state.cursor());
        self.deselect_headers();
    }

    /// Moves the cursor by `motion` with the `count` typed before it,
    /// stepping over header rows: a count of rows up or down counts only
    /// the rows that are not headers, and a motion that lands on a header
    /// goes on the way it went.
    fn move_cursor(&mut self, motion: Motion, count: Option<usize>) {
        let from = self.state.cursor();
        let count = match motion {
            Motion::Down | Motion::Up if !self.headers.is_empty() => {
                let times = count.unwrap_or(1);
                Some(self.rows_past_headers(motion == Motion::Down, times))
            }
            _ => count,
        };
        self.keys.move_cursor(&mut self.state, motion, count);
        self.settle_cursor(from);
        self.deselect_headers();
    }

    /// How many rows down (or up, unless `down`) the `times`th row from the
    /// cursor that is not a header is: as far as the last such row when
    /// fewer are there, and 0 when none is.
    fn rows_past_headers(&self, down: bool, times: usize) -> usize {
        let cursor = self.state.cursor();
        let rows: Box<dyn Iterator<Item = usize>> = match down {
            true => Box::new(cursor + 1..self.state.count()),
            false => Box::new((0..cursor).rev()),
        };
        rows.filter(|row| !self.headers.contains(row))
            .take(times)
            .last()
            .map_or(0, |row| row.abs_diff(cursor))
    }

    /// Steps the cursor off a header row it is on: on the way it moved from
    /// row `from` (down when it did not move), or the other way when no row
    /// there is not a header. It stays when every row is one.
    fn settle_cursor(&mut self, from: usize) {
        let cursor = self.state.cursor();
        if !self.headers.contains(&cursor) {
            return;
        }
        let below = (cursor + 1..self.state.count()).find(|row| !self.headers.contains(row));
        let above = (0..cursor).rev().find(|row| !self.headers.contains(row));
        let row = match cursor < from {
            true => above.or(below),
            false => below.or(above),
        };
        if let Some(row) = row {
            self.state.set_cursor(row);
        }
    }

    /// Deselects the header rows, which a range or a toggle took in.
    fn deselect_headers(&mut self) {
        let selected: Vec<usize> = self
            .state
            .selected()
            .filter(|row| self.headers.contains(row))
            .collect();
        for row in selected {
            self.state.toggle_selected(row);
        }
    }

    /// Reacts to pointer `input`: clicks on rows, drags of rows, and
    /// the wheel over the list. A press of any button or the wheel over
    /// the list, or the window losing focus, drops a pending count or
    /// prefix, as in Vim. Offer it every input before acting on it.
    pub(crate) fn handle_event(&mut self, input: &Input, cx: &mut EventCx<'_>) -> ListResponse {
        if self.keys.reset_on(input, self.is_pointer_inside()) {
            cx.request_redraw();
        }
        match self.drag.handle_event(input, cx) {
            DragResponse::Cancelled => return ListResponse::Handled,
            DragResponse::Dropped(dropped) => return self.drop_row(dropped),
            DragResponse::Ignored => {}
            DragResponse::Moved | DragResponse::Started => {
                self.pointer = self.drag.position();
                return ListResponse::Handled;
            }
        }
        match input {
            Input::PointerButton(button)
                if button.pressed && button.button == PointerButton::Left =>
            {
                self.press(cx, button)
            }
            Input::PointerButton(button)
                if button.pressed && button.button == PointerButton::Right =>
            {
                self.press_for_menu(cx, button)
            }
            Input::PointerButton(button)
                if button.button == PointerButton::Left && self.band.is_some() =>
            {
                self.band = None;
                cx.request_redraw();
                ListResponse::Handled
            }
            Input::PointerLeft => {
                self.pointer = None;
                ListResponse::Ignored
            }
            Input::PointerMoved(position) => {
                self.pointer = Some(*position);
                if self.band.is_none() {
                    return ListResponse::Ignored;
                }
                self.stretch_band();
                cx.request_redraw();
                ListResponse::Handled
            }
            Input::Wheel(wheel) if self.is_pointer_inside() => {
                self.scroll(cx, wheel.delta);
                ListResponse::Handled
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
            | Input::Key(_)
            | Input::PointerButton(_)
            | Input::Wheel(_) => ListResponse::Ignored,
        }
    }

    /// Asks for the next frame of a glide still under way, or of a rubber
    /// band held at an edge scrolling on, returning when it is due; `None`
    /// at rest: the lent list's [`Widget::about_to_wait`], and a table's or
    /// a tree's, which keep a list.
    pub(crate) fn about_to_wait(&self, cx: &mut EventCx<'_>) -> Option<Instant> {
        let glide = self.glide.about_to_wait(cx);
        let step = self.band.as_ref().and_then(|band| band.next_step);
        if let Some(step) = step {
            cx.request_redraw_at(step);
        }
        glide.into_iter().chain(step).min()
    }

    /// The list lent `rows`, as a [`Widget`] to hand inputs and draw.
    pub fn lend<'a, S: ListSource + ?Sized>(&'a mut self, rows: &'a S) -> Lent<'a, Self, &'a S> {
        Lent::new(self, rows)
    }

    /// Reacts to `input` as the list's [`Widget::handle`] does: keys
    /// through [`List::handle_key`], asking for a redraw when one was
    /// used, and the rest through [`List::handle_event`].
    pub(crate) fn handle_input(&mut self, input: &Input, cx: &mut EventCx<'_>) -> ListResponse {
        let Some(key) = input.key() else {
            return self.handle_event(input, cx);
        };
        let response = self.handle_key(key);
        if !matches!(response, ListResponse::Ignored) {
            cx.request_redraw();
        }
        response
    }

    /// Draws the rows of `source` that fit in `area`, as the list's
    /// [`Widget::draw`] does, its glide where it is at `now`.
    fn draw_at(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        source: &(impl ListSource + ?Sized),
        theme: &Theme,
        now: Instant,
    ) {
        let metrics = frame.cell_metrics();
        let scale_factor = frame.scale_factor() as f32;
        let shape = theme.shape;
        // In from the highlight's inset too, so text keeps its distance
        // from the highlight's edge.
        let inset = (f32::from(shape.highlight_inset) * scale_factor).round();
        let padding = metrics.width.round() + inset;
        let row_padding = (f32::from(shape.row_padding) * scale_factor).round();
        let font = self.font;
        // A proportional row is a line of the UI font tall, a grid row a
        // cell.
        let line = match font {
            RowFont::Proportional => frame.ui_line_height(),
            RowFont::Cells => metrics.height,
        };
        let draw_row = |frame: &mut Frame, row: usize, rect: Rect| {
            let content = source.row_content(row);
            let inner = Rect::new(
                rect.x + padding,
                rect.y + row_padding,
                (rect.width - 2.0 * padding).max(0.0),
                line,
            );
            draw_content_in(frame, &content, inner, TextAlign::Start, theme, font);
        };
        let rows = RowsToDraw {
            area,
            count: source.row_count(),
            now,
            pinned: 0,
            row_height: line + 2.0 * row_padding,
        };
        self.draw_rows(frame, rows, theme, draw_row, |row| source.row_text(row));
    }

    /// Draws `rows.count` rows into `rows.area` at the glide's offset at
    /// `rows.now`: each row's background (selection, or hover with an
    /// accent bar for the cursor's row, both in the shape's
    /// [`Shape::highlight_band`](crate::Shape::highlight_band), the bar a
    /// pill as tall as the band in its inset when it has one) and then `draw_row` with the row and
    /// its rect for its content; while a row is dragged, the gap it would
    /// land in and its `label` beside the pointer.
    pub(crate) fn draw_rows<'s>(
        &mut self,
        frame: &mut Frame,
        rows: RowsToDraw,
        theme: &Theme,
        mut draw_row: impl FnMut(&mut Frame, usize, Rect),
        label: impl Fn(usize) -> Cow<'s, str>,
    ) {
        let RowsToDraw {
            area,
            count,
            now,
            pinned,
            row_height,
        } = rows;
        let scale_factor = frame.scale_factor();
        let visible = whole_rows(area.height, row_height);
        let pinned = pinned.min(count).min(visible);
        self.state.set_count(count);
        self.state.set_visible(visible);
        self.state.set_pinned(pinned);
        self.step_band(area, scale_factor, now);
        self.glide.set_reduced_motion(frame.reduced_motion());
        let offset = self.glide.follow(self.state.scroll(), visible, now);
        let layout = ListLayout {
            area,
            offset,
            pinned,
            row_height,
            rows: visible,
            scale_factor,
        };
        self.layout = Some(layout);
        if self.band.is_some() {
            self.stretch_band();
        }
        let bar = logical(CURSOR_BAR, scale_factor);
        let (pinned_band, scrolled) = layout.split_pinned();
        let mut draw_one = |frame: &mut Frame, row: usize, y: f32| {
            let rect = Rect::new(area.x, y.round(), area.width, row_height);
            let band = theme.shape.highlight_band(rect, scale_factor);
            let is_header = self.headers.contains(&row);
            let is_cursor = row == self.state.cursor() && !is_header;
            let fill = match (self.state.is_selected(row) && !is_header, is_cursor) {
                (true, _) => Some(theme.selected_row),
                (false, true) => Some(theme.hover),
                (false, false) => None,
            };
            if let Some(fill) = fill {
                frame.draw_box(band, &theme.shape.highlight(fill, scale_factor));
            }
            draw_row(frame, row, rect);
            if is_cursor {
                draw_cursor_bar(frame, rect, band, bar, theme.accent.into());
            }
        };
        frame.clipped(scrolled, |frame| {
            for row in rows_in_sight(offset, visible, self.state.count()) {
                draw_one(frame, row, area.y + (row as f32 - offset) * row_height);
            }
        });
        if pinned > 0 {
            frame.clipped(pinned_band, |frame| {
                for row in 0..pinned {
                    draw_one(frame, row, area.y + row as f32 * row_height);
                }
            });
        }
        frame.clipped(area, |frame| self.draw_band(frame, layout, theme));
        self.draw_drag(frame, layout, theme, label);
    }

    /// Draws the rubber band, if one is being drawn: a shade of the accent
    /// color inside an accent outline.
    fn draw_band(&self, frame: &mut Frame, layout: ListLayout, theme: &Theme) {
        let (Some(band), Some(pointer)) = (&self.band, self.pointer) else {
            return;
        };
        let start = band.band.start();
        let start = Point::new(start.x, layout.screen_y(start.y));
        draw_band(frame, band_rect(start, pointer), theme);
    }

    /// Starts a rubber band at `point` in `layout`: in place of the
    /// selection, or on top of it when `keep` (Ctrl held).
    fn start_band(&mut self, point: Point, keep: bool, layout: ListLayout) {
        let start = Point::new(point.x, layout.content_y(point.y));
        if !keep {
            self.state.clear_selection();
        }
        self.band = Some(RowBand {
            band: RubberBand::new(start, self.state.selected()),
            next_step: None,
        });
    }

    /// Selects the rows the rubber band touches from its start to the
    /// pointer, on top of the rows it started with.
    fn stretch_band(&mut self) {
        let (Some(band), Some(layout), Some(pointer)) = (&self.band, self.layout, self.pointer)
        else {
            return;
        };
        let touched = layout.rows_between(
            band.band.start().y,
            layout.content_y(pointer.y),
            self.state.count(),
        );
        let headers = &self.headers;
        let selection = band.band.selection(touched);
        self.state
            .set_selected(selection.filter(|row| !headers.contains(row)));
    }

    /// Scrolls a row toward the edge a rubber band is held near in `area`
    /// when the step is due at `now`, so a band can reach rows out of
    /// sight; nothing while it is away from the edges.
    fn step_band(&mut self, area: Rect, scale_factor: f64, now: Instant) {
        let (Some(band), Some(pointer)) = (&mut self.band, self.pointer) else {
            return;
        };
        let edge = logical(AUTO_SCROLL_EDGE, scale_factor);
        let direction = if pointer.y < area.y + edge {
            -1
        } else if pointer.y > area.y + area.height - edge {
            1
        } else {
            band.next_step = None;
            return;
        };
        if band.next_step.is_some_and(|due| now < due) {
            return;
        }
        band.next_step = Some(now + AUTO_SCROLL_INTERVAL);
        let cursor = self.state.cursor();
        self.state.scroll_by(direction);
        self.settle_cursor(cursor);
    }

    /// Stops presses on rows from starting drags, for a widget whose rows
    /// cannot be moved.
    pub(crate) fn turn_drag_off(&mut self) {
        self.is_drag_off = true;
    }

    /// Describes each row to screen readers as clickable, so a hint label
    /// goes over it, for a widget that activates a row clicked that way
    /// (see [`List::access_row`]).
    pub(crate) fn make_rows_clickable(&mut self) {
        self.is_rows_clickable = true;
    }

    /// The row in sight that the screen reader node `target` stands for,
    /// on the list described as `id`.
    pub(crate) fn access_row(&self, id: FocusId, target: FocusId) -> Option<usize> {
        self.state
            .visible_rows()
            .find(|&row| id.child(row as u64) == target)
    }

    /// Lets rows be dragged onto other rows, the row under the pointer
    /// marked as the drop target in place of the gap line, for a widget
    /// whose rows take others (a tree's folders).
    pub(crate) fn drop_onto_rows(&mut self) {
        self.is_drag_off = false;
        self.is_drop_onto = true;
    }

    /// The row under `point` in the last frame drawn; `None` below the
    /// last row, outside the list, or before the first frame.
    pub(crate) fn row_at_point(&self, point: Point) -> Option<usize> {
        self.layout?.row_at(point, self.state.count())
    }

    /// Where `row` was drawn in the last frame, in physical pixels, whether
    /// in sight or not (above or below the list then): to place an inline
    /// rename field over a row or a hint label beside it. `None` before the
    /// first frame.
    pub fn row_rect(&self, row: usize) -> Option<Rect> {
        let layout = self.layout?;
        let slot = match row < layout.pinned {
            true => row as f32,
            false => row as f32 - layout.offset,
        };
        let top = layout.area.y + slot * layout.row_height;
        Some(Rect::new(
            layout.area.x,
            top,
            layout.area.width,
            layout.row_height,
        ))
    }

    /// The band the pinned rows were drawn in last, how many there were,
    /// and the rest of the list, where the others scroll; `None` before
    /// the first frame.
    pub(crate) fn pinned_split(&self) -> Option<(Rect, usize, Rect)> {
        let layout = self.layout?;
        let (band, scrolled) = layout.split_pinned();
        Some((band, layout.pinned, scrolled))
    }

    /// The rows drawn in the last frame, wholly or in part: the ones to put
    /// hint labels on or to load lazily. Empty before the first frame.
    pub fn visible_range(&self) -> Range<usize> {
        self.layout.map_or(0..0, |layout| {
            rows_in_sight(layout.offset, layout.rows, self.state.count())
        })
    }

    /// Draws, while a row is dragged, the line where it would land and its
    /// `label` beside the pointer.
    fn draw_drag<'s>(
        &self,
        frame: &mut Frame,
        layout: ListLayout,
        theme: &Theme,
        label: impl Fn(usize) -> Cow<'s, str>,
    ) {
        let (Some(&row), Some(pointer)) = (self.drag.payload(), self.drag.position()) else {
            return;
        };
        if self.is_drop_onto {
            let onto = layout.row_at(pointer, self.state.count());
            if let Some(rect) = onto.and_then(|onto| self.row_rect(onto)) {
                draw_drop_target(frame, rect, true, theme);
            }
        } else if let Some(gap) = layout.gap_at(pointer, self.state.count()) {
            let thickness = logical(CURSOR_BAR, layout.scale_factor);
            let y = layout.area.y + (gap as f32 - layout.offset) * layout.row_height;
            let line = Rect::new(
                layout.area.x,
                (y - thickness / 2.0).round(),
                layout.area.width,
                thickness,
            );
            frame.overlay(|frame| frame.fill_rect(line, theme.accent));
        }
        if row < self.state.count() {
            self.drag.draw(frame, &label(row), theme);
        }
    }

    /// The response to a dragged row dropped as `dropped`.
    fn drop_row(&self, dropped: Dropped<usize>) -> ListResponse {
        let before = self
            .layout
            .and_then(|layout| layout.gap_at(dropped.position, self.state.count()));
        ListResponse::Dropped(RowDrop {
            before,
            position: dropped.position,
            row: dropped.payload,
        })
    }

    /// Describes the list as last drawn to screen readers as `id`: a list
    /// holding the rows in sight (row `index` is the `index`th child of
    /// `id`), each named by its text and selected when selected; the
    /// cursor's row has the focus while the list does. Nothing before the
    /// first frame.
    pub fn access(&self, frame: &mut Frame, id: FocusId, source: &(impl ListSource + ?Sized)) {
        let Some(layout) = self.layout else {
            return;
        };
        frame.enter_access(id, layout.area, Node::new(Role::List));
        for row in self.state.visible_rows() {
            let rect = self.row_rect(row).unwrap_or(layout.area);
            let mut node = Node::new(Role::ListItem);
            node.set_label(source.row_text(row).into_owned());
            node.set_selected(self.state.is_selected(row));
            if self.is_rows_clickable {
                node.add_action(Action::Click);
            }
            frame.access(id.child(row as u64), rect, node);
        }
        frame.leave_access();
        if self.state.visible_rows().contains(&self.state.cursor()) {
            let cursor = id.child(self.state.cursor() as u64);
            frame.access.set_inner_focus(id, cursor);
        }
    }

    /// A left press: move the cursor to the row under the pointer, toggle
    /// its selection with Ctrl held, select from the anchor to it with
    /// Shift held, or activate it on a double click; on empty space or with
    /// Alt held, start a rubber band. A press on a header row does nothing.
    fn press(&mut self, cx: &mut EventCx<'_>, button: &ButtonInput) -> ListResponse {
        let point = button.position;
        self.pointer = Some(point);
        let Some(layout) = self.layout else {
            return ListResponse::Ignored;
        };
        if !layout.area.contains(point) {
            return ListResponse::Ignored;
        }
        cx.request_redraw();
        let ctrl = button.mods.contains(Mods::CTRL);
        let row = layout
            .row_at(point, self.state.count())
            .filter(|_| !button.mods.contains(Mods::ALT));
        let Some(row) = row else {
            self.start_band(point, ctrl, layout);
            return ListResponse::Handled;
        };
        if self.headers.contains(&row) {
            return ListResponse::Handled;
        }
        let now = clock::now();
        let same_row = self.state.cursor() == row;
        if same_row && is_double_click(self.last_press, now, point, layout.scale_factor) {
            self.last_press = None;
            return ListResponse::Activated(row);
        }
        self.last_press = Some((now, point));
        if button.mods.contains(Mods::SHIFT) {
            if self.state.anchor().is_none() {
                self.state.set_anchor(Some(self.state.cursor()));
            }
            self.state.set_cursor(row);
            self.state.select_to(row);
            return ListResponse::CursorMoved(row);
        }
        if !self.is_drag_off {
            self.drag.arm(row, point, layout.scale_factor);
        }
        self.state.set_cursor(row);
        if ctrl {
            self.state.toggle_selected(row);
        }
        if !self.state.is_ranging() {
            self.state.set_anchor(Some(row));
        }
        ListResponse::CursorMoved(row)
    }

    /// A right press: move the cursor to the row under the pointer, so the
    /// context menu the press opens is for that row; the selection stays
    /// as it is. A press on empty space or a header row does nothing.
    fn press_for_menu(&mut self, cx: &mut EventCx<'_>, button: &ButtonInput) -> ListResponse {
        let point = button.position;
        let Some(layout) = self.layout else {
            return ListResponse::Ignored;
        };
        if !layout.area.contains(point) {
            return ListResponse::Ignored;
        }
        let Some(row) = layout.row_at(point, self.state.count()) else {
            return ListResponse::Ignored;
        };
        if self.headers.contains(&row) {
            return ListResponse::Ignored;
        }
        cx.request_redraw();
        self.state.set_cursor(row);
        ListResponse::CursorMoved(row)
    }

    /// Scrolls by the wheel's `delta`: [`WHEEL_ROWS`] rows a notch, or a
    /// row per row height of pixels.
    fn scroll(&mut self, cx: &mut EventCx<'_>, delta: WheelDelta) {
        let Some(layout) = self.layout else {
            return;
        };
        let rows = match delta {
            WheelDelta::Lines(lines) => (-lines.y * WHEEL_ROWS).round(),
            WheelDelta::Pixels(pixels) => {
                self.scroll_rest -= pixels.y;
                let row_height = layout.row_height.max(1.0);
                let whole = (self.scroll_rest / row_height).trunc();
                self.scroll_rest -= whole * row_height;
                whole
            }
        };
        if rows != 0.0 {
            let cursor = self.state.cursor();
            self.state.scroll_by(rows as isize);
            self.settle_cursor(cursor);
            cx.request_redraw();
        }
    }

    /// Whether the pointer is over the list as last drawn.
    fn is_pointer_inside(&self) -> bool {
        match (self.pointer, self.layout) {
            (Some(point), Some(layout)) => layout.area.contains(point),
            (None, _) | (_, None) => false,
        }
    }
}

impl<S: ListSource + ?Sized> Widget for Lent<'_, List, &S> {
    type Output = ();
    type Response = ListResponse;

    /// Reacts to `input`: keys move the cursor, select, and activate, as
    /// the module documentation lists (hand them only while the list has
    /// the keyboard); the pointer clicks, drags rows, and scrolls. A press
    /// of any button or the wheel over the list, or the window losing
    /// focus, drops a pending count or prefix, as in Vim. Asks for a
    /// redraw whenever what is drawn changed.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> ListResponse {
        self.widget.keys.set_scheme(cx.key_scheme());
        self.widget.handle_input(input, cx)
    }

    /// Draws the rows that fit in `area`, in `theme`'s colors: selected
    /// rows on the selection color, the cursor's row on the hover color
    /// with an accent bar at its left.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        self.widget
            .draw_at(frame, area, self.model, theme, clock::now());
    }

    /// Asks for the next frame of a glide still under way, or of a rubber
    /// band held at an edge scrolling on, returning when it is due; `None`
    /// at rest.
    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        self.widget.about_to_wait(cx)
    }
}

impl Outcome for ListResponse {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

// ========================================================================
// ListLayout Implementations
// ========================================================================

impl ListLayout {
    /// The band the pinned rows are drawn in at the top, and the rest of
    /// the list, where the other rows scroll; the band is empty and the
    /// rest the whole list when no row is pinned.
    fn split_pinned(&self) -> (Rect, Rect) {
        let band = (self.pinned as f32 * self.row_height).min(self.area.height);
        let Rect {
            x,
            y,
            width,
            height,
        } = self.area;
        (
            Rect::new(x, y, width, band),
            Rect::new(x, y + band, width, height - band),
        )
    }

    /// `y` on screen in pixels from the top of row 0.
    fn content_y(&self, y: f32) -> f32 {
        y - self.area.y + self.offset * self.row_height
    }

    /// `y` in pixels from the top of row 0, on screen.
    fn screen_y(&self, y: f32) -> f32 {
        y + self.area.y - self.offset * self.row_height
    }

    /// The rows of `count` that the span from `from` to `to` (in pixels
    /// from the top of row 0, either way round) touches.
    fn rows_between(&self, from: f32, to: f32, count: usize) -> Range<usize> {
        if self.row_height <= 0.0 {
            return 0..0;
        }
        let row = |y: f32| (y.max(0.0) / self.row_height).floor() as usize;
        let first = row(from.min(to)).min(count);
        let end = (row(from.max(to)) + 1).min(count);
        first..end.max(first)
    }

    /// The row under `point` when the list has `count` rows; `None` below
    /// the last row or outside the list.
    fn row_at(&self, point: Point, count: usize) -> Option<usize> {
        if !self.area.contains(point) || self.row_height <= 0.0 {
            return None;
        }
        let slot = (point.y - self.area.y) / self.row_height;
        let row = match slot < self.pinned as f32 {
            true => slot.floor(),
            false => (self.offset + slot).floor(),
        };
        let row = row.max(0.0) as usize;
        (row < count).then_some(row)
    }

    /// The gap between rows nearest `point` when the list has `count` rows:
    /// the index of the row after it, up to `count` for the gap after the
    /// last row; `None` outside the list.
    fn gap_at(&self, point: Point, count: usize) -> Option<usize> {
        if !self.area.contains(point) || self.row_height <= 0.0 {
            return None;
        }
        let gap = (self.offset + (point.y - self.area.y) / self.row_height).round();
        Some((gap.max(0.0) as usize).min(count))
    }
}

// ========================================================================
// Functions
// ========================================================================

/// How many whole rows `row_height` high fit in `height`.
fn whole_rows(height: f32, row_height: f32) -> usize {
    if !(height > 0.0 && row_height > 0.0) {
        return 0;
    }
    // Truncating is the point: a partial row does not count.
    (height / row_height).floor() as usize
}

/// Draws the cursor's accent bar, `width` wide, beside `row`'s highlight
/// `band`: along the row's whole edge when the band fills the row, else a
/// pill as tall as the band, centered in the inset to its left, so the
/// band's rounded corners stay whole.
pub(crate) fn draw_cursor_bar(frame: &mut Frame, row: Rect, band: Rect, width: f32, color: Rgba) {
    if band == row {
        frame.fill_rect_over_text(Rect::new(row.x, row.y, width, row.height), color);
        return;
    }
    let x = (row.x + (band.x - row.x - width) / 2.0).round();
    let pill = BoxStyle {
        border: None,
        corner_radius: width / 2.0,
        fill: color,
        shadow: None,
    };
    frame.draw_box(Rect::new(x, band.y, width, band.height), &pill);
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::access::{Focusable, find_node, node_id};
    use crate::app::offscreen;
    use crate::base::color::Rgba;
    use crate::base::key::parse_key;
    use crate::base::styled::TextColor;
    use crate::base::theme::ThemeToken;
    use crate::input::event::WheelInput;
    use crate::render::paragraph::Paragraph;
    use crate::testing::{TestWindow, drawn_text};

    /// Rows 20 pixels high from y 100, scrolled to row 50.
    const LAYOUT: ListLayout = ListLayout {
        area: Rect::new(0.0, 100.0, 400.0, 200.0),
        offset: 50.0,
        pinned: 0,
        row_height: 20.0,
        rows: 10,
        scale_factor: 1.0,
    };

    /// [`LAYOUT`] scrolled to the top.
    const TOP: ListLayout = ListLayout {
        offset: 0.0,
        ..LAYOUT
    };

    #[test]
    fn test_a_point_finds_its_row_counting_from_the_scroll() {
        let at = |y: f32| LAYOUT.row_at(Point::new(10.0, y), 1000);
        assert_eq!(at(100.0), Some(50));
        assert_eq!(at(119.9), Some(50));
        assert_eq!(at(120.0), Some(51));
        assert_eq!(at(299.0), Some(59));
        assert_eq!(at(99.0), None, "above the list");
        assert_eq!(at(300.0), None, "below the list");
    }

    #[test]
    fn test_below_the_last_row_is_no_row() {
        assert_eq!(TOP.row_at(Point::new(10.0, 165.0), 3), None);
        assert_eq!(TOP.row_at(Point::new(10.0, 145.0), 3), Some(2));
    }

    #[test]
    fn test_a_drag_lands_in_the_gap_nearest_the_pointer_and_never_past_the_end() {
        let at = |y: f32| LAYOUT.gap_at(Point::new(10.0, y), 1000);
        assert_eq!(at(100.0), Some(50), "the top edge");
        assert_eq!(at(109.0), Some(50), "the upper half of the first row");
        assert_eq!(at(111.0), Some(51), "its lower half");
        assert_eq!(TOP.gap_at(Point::new(10.0, 290.0), 3), Some(3));
        assert_eq!(TOP.gap_at(Point::new(500.0, 150.0), 3), None);
    }

    #[test]
    fn test_a_point_over_the_pinned_rows_finds_them_and_below_them_a_scrolled_row() {
        let pinned = ListLayout {
            pinned: 2,
            ..LAYOUT
        };
        let at = |y: f32| pinned.row_at(Point::new(10.0, y), 1000);
        assert_eq!(
            (at(100.0), at(125.0)),
            (Some(0), Some(1)),
            "pinned at the top"
        );
        assert_eq!(at(140.0), Some(52), "the third slot, scrolled to row 50");
        let (band, scrolled) = pinned.split_pinned();
        assert_eq!((band.height, scrolled.y), (40.0, 140.0));
    }

    #[test]
    fn test_a_point_mid_glide_finds_the_row_drawn_under_it() {
        let gliding = ListLayout {
            offset: 50.5,
            ..LAYOUT
        };
        assert_eq!(gliding.row_at(Point::new(10.0, 100.0), 1000), Some(50));
        assert_eq!(
            gliding.row_at(Point::new(10.0, 111.0), 1000),
            Some(51),
            "row 51 starts at 110"
        );
    }

    #[test]
    fn test_keys_move_the_cursor_toggle_and_activate_its_row() {
        let mut list = List::new().with_vim_and_emacs();
        list.state_mut().set_count(10);
        list.state_mut().set_visible(4);
        let mut press = |spec| list.handle_key(parse_key(spec).expect("a key"));
        assert_eq!(press("j"), ListResponse::CursorMoved(1));
        assert_eq!(press("C-n"), ListResponse::CursorMoved(2));
        assert_eq!(press("G"), ListResponse::CursorMoved(9));
        assert_eq!(press("<end>"), ListResponse::Handled, "already there");
        assert_eq!(press("<space>"), ListResponse::Handled);
        assert_eq!(press("<cr>"), ListResponse::Activated(9));
        assert_eq!(press("l"), ListResponse::Ignored, "no columns to move in");
        assert_eq!(press("x"), ListResponse::Ignored);
        assert!(list.state().is_selected(9));
        assert_eq!(list.state().scroll(), 6, "the cursor scrolled into sight");
    }

    #[test]
    fn test_a_count_or_prefix_waits_handled_and_shows_until_its_motion() {
        let mut list = List::new().with_vim_and_emacs();
        list.state_mut().set_count(100);
        list.state_mut().set_visible(10);
        let press = |list: &mut List, spec| list.handle_key(parse_key(spec).expect("a key"));
        assert_eq!(press(&mut list, "5"), ListResponse::Handled);
        assert_eq!(list.keys().count(), Some(5));
        assert_eq!(press(&mut list, "j"), ListResponse::CursorMoved(5));
        assert_eq!(press(&mut list, "g"), ListResponse::Handled);
        assert_eq!(list.keys().pending_keys(), [parse_key("g").unwrap()]);
        assert_eq!(press(&mut list, "g"), ListResponse::CursorMoved(0));
        assert_eq!(press(&mut list, "0"), ListResponse::Ignored, "no columns");
    }

    #[test]
    fn test_enter_in_an_empty_list_activates_nothing() {
        let mut list = List::new().with_vim_and_emacs();
        let enter = parse_key("<cr>").expect("a key");
        assert_eq!(list.handle_key(enter), ListResponse::Handled);
    }

    /// Five rows, `row 0` to `row 4`.
    struct Rows;

    impl ListSource for Rows {
        fn row_count(&self) -> usize {
            5
        }

        fn row_text(&self, row: usize) -> Cow<'_, str> {
            Cow::Owned(format!("row {row}"))
        }
    }

    #[test]
    fn test_the_cursor_row_is_hovered_with_an_accent_bar_and_selected_rows_lit() {
        let theme = Theme::dark().with_grid_look();
        let mut list = List::new().with_vim_and_emacs();
        list.state_mut().set_count(Rows.row_count());
        list.state_mut().set_cursor(1);
        list.state_mut().toggle_selected(3);
        let area = Rect::new(0.0, 0.0, 200.0, 160.0);
        let Some(snapshot) = offscreen::render(200, 160, |frame| {
            frame.clear(Rgba::from(theme.background));
            list.lend(&Rows).draw(frame, area, &theme);
        }) else {
            return;
        };
        let height = snapshot.metrics().height;
        let middle = |row: f32| (row + 0.5) * height;
        let right = area.width - 1.0;
        snapshot.assert_pixel(right, middle(0.0), theme.background, "a plain row");
        snapshot.assert_pixel(right, middle(1.0), theme.hover, "the cursor row");
        snapshot.assert_pixel(0.0, middle(1.0), theme.accent, "the cursor bar");
        snapshot.assert_pixel(right, middle(3.0), theme.selection, "a selected row");
        snapshot.assert_pixel(0.0, middle(3.0), theme.selection, "no bar off the cursor");
        let text = Rect::new(0.0, height, area.width, height);
        assert!(
            snapshot.contains_color(text, theme.foreground),
            "the cursor row's text"
        );
    }

    #[test]
    fn test_a_modern_list_pads_its_rows_and_marks_the_cursor_with_a_pill_beside_its_band() {
        let theme = Theme::dark();
        let shape = theme.shape;
        let mut list = List::new().with_vim_and_emacs();
        list.state_mut().set_count(Rows.row_count());
        list.state_mut().set_cursor(1);
        list.state_mut().toggle_selected(3);
        let area = Rect::new(0.0, 0.0, 200.0, 200.0);
        let Some(snapshot) = offscreen::render(200, 200, |frame| {
            frame.clear(Rgba::from(theme.background));
            list.lend(&Rows).draw(frame, area, &theme);
        }) else {
            return;
        };
        let height = snapshot.metrics().height + 2.0 * f32::from(shape.row_padding);
        let top = |row: f32| row * height;
        let middle = |row: f32| top(row) + height / 2.0;
        let inset = f32::from(shape.highlight_inset);
        let right = area.width - inset - 2.0;
        snapshot.assert_pixel(right, middle(1.0), theme.hover, "the cursor row");
        let bar = inset / 2.0;
        snapshot.assert_pixel(bar, middle(1.0), theme.accent, "the bar in the inset");
        snapshot.assert_pixel(bar, top(1.0) + 3.0, theme.accent, "as tall as the band");
        snapshot.assert_pixel(bar, top(1.0), theme.background, "but no taller");
        snapshot.assert_pixel(inset + 1.0, middle(1.0), theme.hover, "the band beside it");
        snapshot.assert_pixel(right, middle(3.0), theme.selected_row, "a selected row");
        snapshot.assert_pixel(right, middle(2.0), theme.background, "rows padded apart");
    }

    #[test]
    fn test_the_list_describes_its_rows_in_sight_and_its_cursor_row_has_focus() {
        let id = FocusId::new("list");
        let mut list = List::new().with_vim_and_emacs();
        list.state_mut().set_count(5);
        list.state_mut().set_cursor(2);
        list.state_mut().toggle_selected(1);
        let mut frame = offscreen::plain_frame(400, 400);
        let area = Rect::new(0.0, 0.0, 400.0, 400.0);
        list.lend(&Rows).draw(&mut frame, area, &Theme::dark());
        list.access(&mut frame, id, &Rows);
        let focusable = Focusable {
            id,
            parent: None,
            rect: area,
        };
        frame.access.set_focus(vec![focusable], Some(id));
        let update = frame.access.tree_update("t");
        let node = |id| find_node(&update, id).expect("described");
        assert_eq!(node(id).role(), Role::List);
        let row = node(id.child(1));
        assert_eq!(
            (row.role(), row.label(), row.is_selected()),
            (Role::ListItem, Some("row 1"), Some(true))
        );
        assert_eq!(update.focus, node_id(id.child(2)), "the cursor's row");
    }

    /// Rows of prose, one too long for any list.
    struct Prose;

    impl ListSource for Prose {
        fn row_count(&self) -> usize {
            2
        }

        fn row_text(&self, row: usize) -> Cow<'_, str> {
            match row {
                0 => Cow::Borrowed("illicit"),
                _ => Cow::Owned("word ".repeat(200)),
            }
        }
    }

    #[test]
    fn test_a_proportional_list_shapes_its_rows_and_cuts_a_long_one_to_its_width() {
        let area = Rect::new(0.0, 0.0, 300.0, 200.0);
        let mut frame = offscreen::plain_frame(300, 200);
        List::new()
            .with_vim_and_emacs()
            .with_proportional_font()
            .lend(&Prose)
            .draw(&mut frame, area, &Theme::dark());
        let rows = frame.draw.text();
        assert_eq!(rows.len(), 2, "one shaped line a row");
        assert!(rows.iter().all(|row| row.wrap.is_some()), "proportional");
        let shown = drawn_text(&frame);
        assert_eq!(shown[0], "illicit");
        assert!(shown[1].ends_with('\u{2026}'), "cut with an ellipsis");
        let foreground = TextColor::Token(ThemeToken::Foreground);
        let wide = frame.paragraph_width(&Paragraph::plain(&shown[1], foreground));
        assert!(wide <= area.width, "{wide} fits the row");
        let mut cells = offscreen::plain_frame(300, 200);
        List::new()
            .with_vim_and_emacs()
            .lend(&Prose)
            .draw(&mut cells, area, &Theme::dark());
        assert!(cells.draw.text().iter().all(|row| row.wrap.is_none()));
    }

    /// A press of the left button at `x`, `y` with `mods` held, then its
    /// release.
    fn click(window: &mut TestWindow, list: &mut List, x: f32, y: f32, mods: Mods) -> ListResponse {
        let press = ButtonInput::press(PointerButton::Left, Point::new(x, y)).with_mods(mods);
        let pressed = window.send(&Input::PointerButton(press), |input, cx| {
            list.handle_event(input, cx)
        });
        let release = ButtonInput {
            pressed: false,
            ..press
        };
        window.send(&Input::PointerButton(release), |input, cx| {
            list.handle_event(input, cx)
        });
        pressed
    }

    #[test]
    fn test_a_click_moves_the_cursor_ctrl_click_toggles_and_a_double_click_activates() {
        let mut window = TestWindow::keyboard_first(400, 300);
        let mut list = List::new().with_vim_and_emacs();
        list.state_mut().set_count(10);
        list.layout = Some(TOP);
        assert_eq!(
            click(&mut window, &mut list, 10.0, 145.0, Mods::NONE),
            ListResponse::CursorMoved(2)
        );
        assert!(window.take_redraw());
        assert_eq!(
            click(&mut window, &mut list, 10.0, 185.0, Mods::CTRL),
            ListResponse::CursorMoved(4)
        );
        assert!(list.state().is_selected(4), "Ctrl toggles the row");
        assert_eq!(
            click(&mut window, &mut list, 10.0, 185.0, Mods::NONE),
            ListResponse::Activated(4)
        );
        assert_eq!(
            click(&mut window, &mut list, 10.0, 50.0, Mods::NONE),
            ListResponse::Ignored,
            "above the list"
        );
    }

    #[test]
    fn test_the_wheel_over_the_list_scrolls_three_rows_a_notch_and_elsewhere_nothing() {
        let mut window = TestWindow::keyboard_first(400, 400);
        let mut list = List::new().with_vim_and_emacs();
        list.state_mut().set_count(100);
        list.state_mut().set_visible(10);
        list.layout = Some(TOP);
        let wheel = |y: f32| {
            Input::Wheel(WheelInput::new(
                WheelDelta::Lines(Point::new(0.0, -1.0)),
                Point::new(10.0, y),
            ))
        };
        window.send(
            &Input::PointerMoved(Point::new(10.0, 350.0)),
            |input, cx| list.handle_event(input, cx),
        );
        assert_eq!(
            window.send(&wheel(350.0), |input, cx| list.handle_event(input, cx)),
            ListResponse::Ignored
        );
        window.send(
            &Input::PointerMoved(Point::new(10.0, 150.0)),
            |input, cx| list.handle_event(input, cx),
        );
        assert_eq!(
            window.send(&wheel(150.0), |input, cx| list.handle_event(input, cx)),
            ListResponse::Handled
        );
        assert_eq!(list.state().scroll(), 3);
    }

    #[test]
    fn test_a_lent_list_takes_keys_and_pointer_through_one_handle() {
        let mut window = TestWindow::keyboard_first(400, 300);
        let mut list = List::new().with_vim_and_emacs();
        list.state_mut().set_count(Rows.row_count());
        list.layout = Some(TOP);
        let j = Input::Key(parse_key("j").expect("a key"));
        assert_eq!(
            window.feed(&mut list.lend(&Rows), &j),
            ListResponse::CursorMoved(1)
        );
        assert!(window.take_redraw(), "a key used asks a redraw");
        let x = Input::Key(parse_key("x").expect("a key"));
        assert_eq!(
            window.feed(&mut list.lend(&Rows), &x),
            ListResponse::Ignored
        );
        assert!(!window.take_redraw());
        let press = ButtonInput::press(PointerButton::Left, Point::new(10.0, 165.0));
        let response = window.feed(&mut list.lend(&Rows), &Input::PointerButton(press));
        assert_eq!(response, ListResponse::CursorMoved(3));
    }

    #[test]
    fn test_a_right_press_moves_the_cursor_to_its_row_and_keeps_the_selection() {
        let mut window = TestWindow::keyboard_first(400, 300);
        let mut list = List::new().with_vim_and_emacs();
        list.state_mut().set_count(Rows.row_count());
        list.state_mut().set_selected([1]);
        list.layout = Some(TOP);
        let press = ButtonInput::press(PointerButton::Right, Point::new(10.0, 165.0));
        let response = window.feed(&mut list.lend(&Rows), &Input::PointerButton(press));
        assert_eq!(response, ListResponse::CursorMoved(3));
        assert_eq!(list.state().cursor(), 3);
        assert_eq!(list.state().selected().collect::<Vec<_>>(), [1]);
        let outside = ButtonInput::press(PointerButton::Right, Point::new(10.0, 5000.0));
        let response = window.feed(&mut list.lend(&Rows), &Input::PointerButton(outside));
        assert_eq!(response, ListResponse::Ignored);
    }

    /// The pointer moved to `x`, `y`, as the list sees it.
    fn move_to(window: &mut TestWindow, list: &mut List, x: f32, y: f32) -> ListResponse {
        window.send(&Input::PointerMoved(Point::new(x, y)), |input, cx| {
            list.handle_event(input, cx)
        })
    }

    #[test]
    fn test_shift_click_selects_from_the_row_last_clicked_on_top_of_the_selection() {
        let mut window = TestWindow::keyboard_first(400, 300);
        let mut list = List::new().with_vim_and_emacs();
        list.state_mut().set_count(10);
        list.layout = Some(TOP);
        click(&mut window, &mut list, 10.0, 105.0, Mods::CTRL);
        click(&mut window, &mut list, 10.0, 145.0, Mods::NONE);
        assert_eq!(
            click(&mut window, &mut list, 10.0, 205.0, Mods::SHIFT),
            ListResponse::CursorMoved(5)
        );
        assert_eq!(list.state().selected().collect::<Vec<_>>(), [0, 2, 3, 4, 5]);
        click(&mut window, &mut list, 10.0, 125.0, Mods::SHIFT);
        assert_eq!(
            list.state().selected().collect::<Vec<_>>(),
            [0, 1, 2],
            "the range shrinks back toward its anchor"
        );
    }

    #[test]
    fn test_shift_arrows_extend_from_the_cursor_and_a_plain_move_starts_afresh() {
        let mut list = List::new().with_vim_and_emacs();
        list.state_mut().set_count(20);
        list.state_mut().set_visible(10);
        let mut press = |spec| list.handle_key(parse_key(spec).expect("a key"));
        press("j");
        press("S-<down>");
        assert_eq!(press("S-<down>"), ListResponse::CursorMoved(3));
        press("j");
        press("j");
        press("S-<up>");
        assert_eq!(
            list.state().selected().collect::<Vec<_>>(),
            [1, 2, 3, 4, 5],
            "a second range from row 5 adds to the first"
        );
    }

    #[test]
    fn test_a_band_from_empty_space_selects_the_rows_it_touches() {
        let mut window = TestWindow::keyboard_first(400, 400);
        let mut list = List::new().with_vim_and_emacs();
        list.state_mut().set_count(5);
        list.state_mut().toggle_selected(0);
        list.layout = Some(TOP);
        let press = ButtonInput::press(PointerButton::Left, Point::new(300.0, 250.0));
        let pressed = window.send(&Input::PointerButton(press), |input, cx| {
            list.handle_event(input, cx)
        });
        assert_eq!(pressed, ListResponse::Handled);
        assert_eq!(
            list.state().selected().count(),
            0,
            "a plain band starts afresh"
        );
        assert_eq!(
            move_to(&mut window, &mut list, 50.0, 150.0),
            ListResponse::Handled
        );
        assert_eq!(list.state().selected().collect::<Vec<_>>(), [2, 3, 4]);
        assert!(window.take_redraw());
        move_to(&mut window, &mut list, 50.0, 190.0);
        assert_eq!(list.state().selected().collect::<Vec<_>>(), [4]);
        let release = ButtonInput {
            pressed: false,
            ..press
        };
        let released = window.send(&Input::PointerButton(release), |input, cx| {
            list.handle_event(input, cx)
        });
        assert_eq!(released, ListResponse::Handled);
        assert_eq!(
            move_to(&mut window, &mut list, 50.0, 110.0),
            ListResponse::Ignored
        );
        assert_eq!(list.state().selected().collect::<Vec<_>>(), [4]);
    }

    #[test]
    fn test_alt_starts_a_band_on_a_row_and_ctrl_keeps_the_selection_under_it() {
        let mut window = TestWindow::keyboard_first(400, 400);
        let mut list = List::new().with_vim_and_emacs();
        list.state_mut().set_count(10);
        list.state_mut().toggle_selected(9);
        list.layout = Some(TOP);
        let press = ButtonInput::press(PointerButton::Left, Point::new(10.0, 105.0))
            .with_mods(Mods::ALT | Mods::CTRL);
        window.send(&Input::PointerButton(press), |input, cx| {
            list.handle_event(input, cx)
        });
        assert!(list.drag().payload().is_none(), "no row drag");
        move_to(&mut window, &mut list, 10.0, 145.0);
        assert_eq!(list.state().selected().collect::<Vec<_>>(), [0, 1, 2, 9]);
    }

    #[test]
    fn test_a_band_held_at_the_bottom_edge_scrolls_a_row_each_interval() {
        let mut window = TestWindow::keyboard_first(400, 400);
        let mut list = List::new().with_vim_and_emacs();
        let theme = Theme::dark().with_grid_look();
        let mut frame = offscreen::plain_frame(400, 400);
        let height = frame.cell_metrics().height;
        let area = Rect::new(0.0, 0.0, 400.0, height * 10.0);
        let rows = Many;
        frame.set_reduced_motion(true);
        let start = Instant::now();
        list.draw_at(&mut frame, area, &rows, &theme, start);
        let press =
            ButtonInput::press(PointerButton::Left, Point::new(10.0, 1.0)).with_mods(Mods::ALT);
        window.send(&Input::PointerButton(press), |input, cx| {
            list.handle_event(input, cx)
        });
        move_to(&mut window, &mut list, 10.0, area.height - 1.0);
        list.draw_at(&mut frame, area, &rows, &theme, start);
        assert_eq!(list.state().scroll(), 1);
        let due = window.send(&Input::FocusGained, |_, cx| list.about_to_wait(cx));
        assert_eq!(due, Some(start + AUTO_SCROLL_INTERVAL));
        list.draw_at(
            &mut frame,
            area,
            &rows,
            &theme,
            start + AUTO_SCROLL_INTERVAL / 2,
        );
        assert_eq!(list.state().scroll(), 1, "not due yet");
        list.draw_at(
            &mut frame,
            area,
            &rows,
            &theme,
            start + AUTO_SCROLL_INTERVAL,
        );
        assert_eq!(list.state().scroll(), 2);
        assert_eq!(
            list.state().selected().collect::<Vec<_>>(),
            (0..=11).collect::<Vec<_>>(),
            "the band reaches the rows scrolled in"
        );
        move_to(&mut window, &mut list, 10.0, area.height / 2.0);
        list.draw_at(
            &mut frame,
            area,
            &rows,
            &theme,
            start + AUTO_SCROLL_INTERVAL * 2,
        );
        assert_eq!(list.state().scroll(), 2, "away from the edge it rests");
    }

    /// A thousand rows.
    struct Many;

    impl ListSource for Many {
        fn row_count(&self) -> usize {
            1000
        }

        fn row_text(&self, row: usize) -> Cow<'_, str> {
            Cow::Owned(row.to_string())
        }
    }

    #[test]
    fn test_row_rect_and_the_visible_range_follow_the_scroll() {
        let mut list = List::new().with_vim_and_emacs();
        let theme = Theme::dark().with_grid_look();
        let mut frame = offscreen::plain_frame(400, 400);
        let height = frame.cell_metrics().height;
        let area = Rect::new(0.0, 50.0, 400.0, height * 10.0);
        frame.set_reduced_motion(true);
        list.state_mut().set_count(1000);
        list.state_mut().set_cursor(500);
        list.draw_at(&mut frame, area, &Many, &theme, Instant::now());
        let range = list.visible_range();
        assert_eq!(range.len(), 10);
        assert!(range.contains(&500));
        let rect = list.row_rect(range.start).expect("drawn");
        assert_eq!((rect.y, rect.height), (50.0, height));
        assert_eq!(
            List::new().with_vim_and_emacs().visible_range(),
            0..0,
            "nothing before a frame"
        );
    }

    /// One row with an icon-less name cut short and two badges.
    struct Badged;

    impl ListSource for Badged {
        fn row_count(&self) -> usize {
            1
        }

        fn row_text(&self, _row: usize) -> Cow<'_, str> {
            Cow::Borrowed("a_rather_long_file_name.txt")
        }

        fn row_content(&self, _row: usize) -> RowContent {
            use crate::base::styled::Span;
            use crate::base::theme::ThemeToken;
            use crate::widgets::row_content::Badge;
            RowContent::plain(self.row_text(0))
                .with_badge(Badge::Text(Span::new("M", ThemeToken::Warning.into())))
                .with_badge(Badge::Dot(ThemeToken::Accent))
        }
    }

    #[test]
    fn test_badges_keep_their_place_at_the_end_and_the_text_is_cut_before_them() {
        let mut list = List::new().with_vim_and_emacs();
        let mut frame = offscreen::plain_frame(400, 400);
        let width = frame.cell_metrics().width;
        let area = Rect::new(0.0, 0.0, (width * 20.0).round(), 100.0);
        list.lend(&Badged)
            .draw(&mut frame, area, &Theme::dark().with_grid_look());
        let drawn = crate::testing::drawn_text(&frame);
        assert_eq!(
            drawn[0], "\u{25cf}",
            "the last badge drawn first, at the end"
        );
        assert_eq!(drawn[1], "M");
        let name = drawn[2..].concat();
        assert_eq!(
            name,
            format!("a_rather_long{}", crate::base::text::ELLIPSIS),
            "20 cells less 2 of padding and 4 of badges and gaps"
        );
    }
}
