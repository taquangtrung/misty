//! [`FlowList`]: a virtualized list of rows of different heights, such as
//! wrapped paragraphs of a book, library entries with a cover and a
//! blurb, or dictionary results. The app supplies the rows through
//! [`FlowSource`]: how tall each row is at a width, measured only when the
//! list needs it, and how to draw it. Only the rows in sight are measured
//! and drawn, so a list of a million rows costs no more than one of
//! twenty.
//!
//! It is a widget of its own rather than a mode of [`List`](crate::List):
//! a list's rows are one cell high, so its scroll, glide, page keys, and
//! hit testing count whole rows, which a [`Table`](crate::Table) and a
//! [`TreeView`](crate::TreeView) share. Here the scroll is a row at the
//! top and how many pixels of it are scrolled out of sight, so the row at
//! the top stays where it is when rows are measured again (after
//! [`FlowList::invalidate_heights`]) or the width changes, and the reader
//! keeps their place.
//!
//! Keys, through the lent [`Widget::handle`](crate::Widget::handle), are
//! the ones every list-like widget shares (see [`List`](crate::List)):
//! Up and Down (`k`, `j`, `C-p`, `C-n`) move the cursor a row and scroll
//! it into sight, `C-f` and `C-b` (Page Down and Page Up) scroll a screen
//! less two rows, `C-d` and `C-u` half a screen, `C-e` and `C-y` a row,
//! each dragging the cursor along when it would leave the screen, `H`,
//! `M`, and `L` go to the top, middle, and bottom rows in sight, `g g` and
//! `G` (Home and End) to the ends, `z t`, `z z`, and `z b` scroll the
//! cursor's row to the top, middle, or bottom, and a count repeats a
//! motion or names a row; Space toggles the cursor row's selection, Shift
//! with a motion key extends it from its anchor, and Enter activates the
//! row. The pointer: a click moves the cursor, Ctrl+click toggles a row's
//! selection, Shift+click selects from the anchor to the row, a double
//! click activates a row, and the wheel scrolls by pixels.
//!
//! Built [`FlowList::with_row_pointer`], the list hands presses and drags
//! inside its rows to the app instead: each comes back as
//! [`FlowListResponse::RowPointer`] with the row and a point in it, for
//! selecting text inside rows. The cursor and the selection are then
//! moved only by keys (and by the app through [`FlowList::state_mut`]),
//! which work as above; the wheel scrolls as ever.
//!
//! [`FlowList::scroll_to`] brings a row to the top, the middle, or just
//! into sight; [`FlowList::visible_range`] tells which rows were drawn, to
//! load what comes next, and [`FlowList::row_rect`] where.
//! [`FlowList::scroll_position`] and [`FlowList::set_scroll_position`]
//! save and restore the exact place scrolled to, such as a reader's place
//! in a book.

use std::borrow::Cow;
use std::collections::HashMap;
use std::ops::Range;
use std::time::Instant;

use accesskit::{Node, Role};

use super::super::app::context::EventCx;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::{Lent, Outcome, Widget};
use super::super::base::clock;
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::{Key, Mods};
use super::super::base::list::ListState;
use super::super::base::theme::Theme;
use super::super::input::click::is_double_click;
use super::super::input::event::{ButtonInput, Input, PointerButton, WheelDelta};
use super::super::render::icon::logical;
use super::list_keys::{CountedKey, ListKey, ListKeys, Motion, Read};

// ========================================================================
// Constants
// ========================================================================

/// The lines of text one notch of a mouse wheel scrolls.
const WHEEL_LINES: f32 = 3.0;

/// The rows of the old screen `C-f` and `C-b` keep in sight, at the
/// average row height.
const PAGE_OVERLAP_ROWS: f32 = 2.0;

/// The width of the accent bar marking the cursor's row, in logical
/// pixels.
const CURSOR_BAR: f32 = 2.0;

/// The least height a row is given, in physical pixels, whatever the
/// source says, so every row can be scrolled past.
const MIN_ROW_HEIGHT: f32 = 1.0;

/// The sign of a scroll toward the end.
const DOWN: f32 = 1.0;

/// The sign of a scroll toward the start.
const UP: f32 = -1.0;

// ========================================================================
// Data Structures
// ========================================================================

/// A list of rows of different heights with a cursor and a selection. It
/// keeps the [`ListState`] (whose cursor and selection it uses; its row
/// scroll is not used), the scroll in pixels, the heights it measured, and
/// where it drew each row in the last frame.
#[derive(Clone, Debug, Default)]
pub struct FlowList {
    /// The rows' heights at `width`, as measured.
    heights: HashMap<usize, f32>,
    /// Whether the cursor is moved into sight at the next frame, after
    /// the scroll was set.
    is_cursor_settling: bool,
    /// Whether presses and drags in rows go to the app.
    is_row_pointer: bool,
    /// Whether a press in a row handed to the app is still held.
    is_row_pressed: bool,
    /// The count and prefix typed toward a motion.
    keys: ListKeys,
    /// When and where the last press on a row was, for double clicks.
    last_press: Option<(Instant, Point)>,
    /// Where the rows were in the last frame drawn, or `None` before it.
    layout: Option<FlowLayout>,
    /// Where the pointer is, or `None` when it is outside the window.
    pointer: Option<Point>,
    /// A row to bring into sight at the next frame, and where.
    reveal: Option<(usize, ScrollAlign)>,
    /// The row at the top and how far it is scrolled.
    scroll: FlowScroll,
    /// The cursor and the selection.
    state: ListState,
    /// The width the heights were measured at, in physical pixels.
    width: f32,
}

/// Where [`FlowList::scroll_to`] brings a row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScrollAlign {
    /// Its bottom at the bottom edge.
    Bottom,
    /// Its middle at the middle.
    Center,
    /// Just into sight, at whichever edge is nearer; not at all when it is
    /// in sight already.
    Nearest,
    /// Its top at the top edge.
    Top,
}

/// What became of an input offered to a [`FlowList`].
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum FlowListResponse {
    /// This row was double-clicked or Enter was pressed on it.
    Activated(usize),
    /// A click or a key moved the cursor to this row.
    CursorMoved(usize),
    /// The list used the input.
    Handled,
    /// Not the list's.
    Ignored,
    /// A press, drag, or release of the left button inside a row, in a
    /// list built [`FlowList::with_row_pointer`].
    RowPointer(RowPointer),
}

/// A press, drag, or release of the left button inside a row of a
/// [`FlowList`] built [`FlowList::with_row_pointer`].
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct RowPointer {
    /// How many presses in a row the press made (2 for a double click), as
    /// [`EventCx::click_count`] says; for a drag or release, the count of
    /// the press that began it.
    pub clicks: u8,
    /// Whether the button went down, the pointer moved while it is held,
    /// or the button went up.
    pub kind: RowPointerKind,
    /// The modifiers held.
    pub mods: Mods,
    /// Where the pointer is from the row's top left corner, in physical
    /// pixels. A drag or release beyond the rows drawn is given in the
    /// nearest row drawn, so the point may lie outside it (above the first
    /// row, `y` is negative).
    pub point: Point,
    /// The row under the pointer, or the nearest row drawn.
    pub row: usize,
}

/// What the left button did in a [`RowPointer`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RowPointerKind {
    /// The pointer moved while the button pressed in a row is held.
    Drag,
    /// The button went down over a row.
    Press,
    /// The button pressed in a row went up, wherever the pointer is.
    Release,
}

/// The scroll of a flow list: the row at the top edge and how many pixels
/// of it are above the edge.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct FlowScroll {
    /// How far the top row is scrolled out of sight, in pixels; less than
    /// its height.
    offset: f32,
    /// The row at the top edge.
    top: usize,
}

/// Where a flow list's rows were drawn.
#[derive(Clone, Debug, PartialEq)]
struct FlowLayout {
    /// The whole list.
    area: Rect,
    /// The height of a line of text, for wheel notches.
    line_height: f32,
    /// The rows in sight, top to bottom, with where each was drawn.
    rows: Vec<(usize, Rect)>,
    /// Physical pixels per logical pixel.
    scale_factor: f64,
}

// ========================================================================
// Traits
// ========================================================================

/// The rows of a [`FlowList`], supplied by the app.
pub trait FlowSource {
    /// How many rows there are.
    fn row_count(&self) -> usize;

    /// How tall `row` is when drawn `width` physical pixels wide, in
    /// physical pixels (as [`Frame::measure_paragraph`] measures). Asked
    /// once per row and width, when the list first needs it; the list
    /// keeps the answer until [`FlowList::invalidate_heights`].
    fn row_height(&self, frame: &Frame, row: usize, width: f32) -> f32;

    /// Draws `row` into `rect`, over the selection and cursor highlight
    /// the list drew. Asked only for rows in sight, each frame.
    fn draw_row(&self, frame: &mut Frame, row: usize, rect: Rect, theme: &Theme);

    /// What screen readers say for `row`.
    fn row_label(&self, row: usize) -> Cow<'_, str>;
}

// ========================================================================
// FlowList Implementations
// ========================================================================

impl FlowList {
    /// An empty list; it reads the rows from the source it is lent.
    pub fn new() -> Self {
        Self::default()
    }

    /// This list handing presses and drags of the left button inside its
    /// rows to the app as [`FlowListResponse::RowPointer`], for selecting
    /// text inside rows, instead of moving the cursor, selecting, and
    /// activating rows with the pointer. Keys still move the cursor and
    /// the selection as the module documentation lists; move the cursor to
    /// a pressed row with [`FlowList::state_mut`] if keys should go on from
    /// there.
    pub fn with_row_pointer(self) -> Self {
        Self {
            is_row_pointer: true,
            ..self
        }
    }

    /// The cursor and the selection (its row scroll is not this list's).
    pub fn state(&self) -> &ListState {
        &self.state
    }

    /// The cursor and the selection, to move and change. Ask for a redraw
    /// after changing them, and [`FlowList::scroll_to`] the cursor to show
    /// it.
    pub fn state_mut(&mut self) -> &mut ListState {
        &mut self.state
    }

    /// The count and prefix typed toward a motion, to show in a status bar
    /// or a [`WhichKey`](crate::WhichKey) popup.
    pub fn keys(&self) -> &ListKeys {
        &self.keys
    }

    /// The count and prefix typed toward a motion, to drop them with
    /// [`ListKeys::reset`] when the list loses the keyboard.
    pub fn keys_mut(&mut self) -> &mut ListKeys {
        &mut self.keys
    }

    /// The list lent `rows`, as a [`Widget`] to hand inputs and draw.
    pub fn lend<'a, S: FlowSource + ?Sized>(&'a mut self, rows: &'a S) -> Lent<'a, Self, &'a S> {
        Lent::new(self, rows)
    }

    /// Brings `row` into sight at the next frame, placed as `align` says.
    /// Ask for a redraw after it.
    pub fn scroll_to(&mut self, row: usize, align: ScrollAlign) {
        self.reveal = Some((row, align));
    }

    /// The place scrolled to: the row at the top edge and how many
    /// physical pixels of it are scrolled out of sight above the edge, to
    /// save and give back to [`FlowList::set_scroll_position`].
    pub fn scroll_position(&self) -> (usize, f32) {
        (self.scroll.top, self.scroll.offset)
    }

    /// Scrolls to `row` at the top edge with `offset` physical pixels of
    /// it out of sight above the edge, as [`FlowList::scroll_position`]
    /// said, from the next frame; it may come before the first frame. The
    /// next frame holds the place within the rows (an offset past the
    /// row's height goes on into the rows after it, and a place past the
    /// end is pulled back) and moves the cursor to the nearest row in
    /// sight when it is out of sight. Ask for a redraw after it.
    pub fn set_scroll_position(&mut self, row: usize, offset: f32) {
        self.scroll = FlowScroll {
            offset: offset.max(0.0),
            top: row,
        };
        self.reveal = None;
        self.is_cursor_settling = true;
    }

    /// Forgets every row's height, to measure them again at the next frame
    /// (after the rows changed, were inserted, or their font did); the row
    /// at the top stays there.
    pub fn invalidate_heights(&mut self) {
        self.heights.clear();
    }

    /// Forgets `row`'s height, to measure it again at the next frame after
    /// it changed.
    pub fn invalidate_row(&mut self, row: usize) {
        self.heights.remove(&row);
    }

    /// The rows drawn in the last frame, wholly or in part, top to bottom:
    /// the ones to load, or to put hint labels on. Empty before the first
    /// frame.
    pub fn visible_range(&self) -> Range<usize> {
        let rows = self.layout.as_ref().map_or(&[][..], |layout| &layout.rows);
        match (rows.first(), rows.last()) {
            (Some(&(first, _)), Some(&(last, _))) => first..last + 1,
            (None, _) | (_, None) => 0..0,
        }
    }

    /// Where `row` was drawn in the last frame, in physical pixels; `None`
    /// when it was not in sight or before the first frame.
    pub fn row_rect(&self, row: usize) -> Option<Rect> {
        let layout = self.layout.as_ref()?;
        layout
            .rows
            .iter()
            .find(|(drawn, _)| *drawn == row)
            .map(|&(_, rect)| rect)
    }

    // --------------------------------------------------------------------
    // Keys
    // --------------------------------------------------------------------

    /// Reacts to `key`, as the module documentation lists.
    fn handle_key(&mut self, key: Key) -> FlowListResponse {
        let CountedKey { count, key } = match self.keys.read(key) {
            Read::Consumed => return FlowListResponse::Handled,
            Read::Ignored => return FlowListResponse::Ignored,
            Read::Key(counted) => counted,
        };
        let cursor = self.state.cursor();
        match key {
            ListKey::Activate => {
                return match self.state.count() {
                    0 => FlowListResponse::Handled,
                    _ => FlowListResponse::Activated(cursor),
                };
            }
            ListKey::Extend(motion) => {
                if self.state.anchor().is_none() {
                    self.state.set_anchor(Some(cursor));
                }
                self.run_motion(motion, count);
                self.state.select_to(self.state.cursor());
            }
            ListKey::Left | ListKey::LineEnd | ListKey::LineStart | ListKey::Right => {
                return FlowListResponse::Ignored;
            }
            ListKey::Move(motion) => {
                if !self.state.is_ranging() {
                    self.state.set_anchor(None);
                }
                self.run_motion(motion, count);
            }
            ListKey::Toggle => {
                self.state.toggle_selected(cursor);
                if !self.state.is_ranging() {
                    self.state.set_anchor(Some(cursor));
                }
                return FlowListResponse::Handled;
            }
        }
        match self.state.cursor() == cursor {
            true => FlowListResponse::Handled,
            false => FlowListResponse::CursorMoved(self.state.cursor()),
        }
    }

    /// Moves the cursor or the view by `motion`, `count` times or to the
    /// count's row, as the module documentation lists.
    fn run_motion(&mut self, motion: Motion, count: Option<usize>) {
        let times = count.unwrap_or(1);
        let last = self.state.count().saturating_sub(1);
        match motion {
            Motion::Down => self.move_cursor(signed(times)),
            Motion::Up => self.move_cursor(-signed(times)),
            Motion::First => self.go_to(count.map_or(0, |count| count - 1)),
            Motion::Last => self.go_to(count.map_or(last, |count| count - 1)),
            Motion::PageDown => self.page(DOWN, times),
            Motion::PageUp => self.page(UP, times),
            Motion::HalfPageDown => self.half_page(DOWN),
            Motion::HalfPageUp => self.half_page(UP),
            Motion::ScrollDown => {
                let top = self.scroll.top;
                let below: f32 = (0..times)
                    .map(|step| self.height_or_estimate(top + step))
                    .sum();
                self.scroll_by(below);
            }
            Motion::ScrollUp => {
                let top = self.scroll.top;
                let above: f32 = (1..=times.min(top))
                    .map(|step| self.height_or_estimate(top - step))
                    .sum();
                self.scroll_by(-above.max(self.scroll.offset));
            }
            Motion::ScreenTop => {
                self.cursor_to_screen_row(|whole| (whole.start + times - 1).min(whole.end - 1));
            }
            Motion::ScreenMiddle => {
                self.cursor_to_screen_row(|whole| whole.start + (whole.len() - 1) / 2)
            }
            Motion::ScreenBottom => {
                self.cursor_to_screen_row(|whole| whole.end.saturating_sub(times).max(whole.start));
            }
            Motion::PlaceBottom => self.place_cursor(count, ScrollAlign::Bottom),
            Motion::PlaceCenter => self.place_cursor(count, ScrollAlign::Center),
            Motion::PlaceTop => self.place_cursor(count, ScrollAlign::Top),
        }
    }

    /// Scrolls `times` screens less [`PAGE_OVERLAP_ROWS`] rows the way
    /// `direction` says, or goes to the end that way when the view cannot
    /// scroll.
    fn page(&mut self, direction: f32, times: usize) {
        let estimate = self.estimate();
        let page = (self.viewport() - PAGE_OVERLAP_ROWS * estimate).max(estimate);
        if !self.scroll_by(page * times as f32 * direction) {
            let last = self.state.count().saturating_sub(1);
            self.go_to(if direction > 0.0 { last } else { 0 });
        }
    }

    /// Scrolls half a screen the way `direction` says and moves the cursor
    /// as many rows as scrolled past the top, at least one.
    fn half_page(&mut self, direction: f32) {
        let top = self.scroll.top;
        self.scroll_by(self.viewport() / 2.0 * direction);
        let rows = signed(self.scroll.top.abs_diff(top).max(1));
        self.state
            .move_cursor(if direction > 0.0 { rows } else { -rows });
        self.cursor_into_sight();
    }

    /// Puts the cursor on the row `pick` chooses among those wholly in
    /// sight.
    fn cursor_to_screen_row(&mut self, pick: impl Fn(Range<usize>) -> usize) {
        let whole = self.whole_rows_in_sight();
        if !whole.is_empty() {
            self.state.set_cursor(pick(whole));
        }
    }

    /// Scrolls the cursor's row (first the count's row, with a count) to
    /// where `align` says.
    fn place_cursor(&mut self, count: Option<usize>, align: ScrollAlign) {
        if let Some(count) = count {
            self.state.set_cursor(count - 1);
        }
        self.scroll_to(self.state.cursor(), align);
    }

    /// Moves the cursor `delta` rows and brings it into sight.
    fn move_cursor(&mut self, delta: isize) {
        self.state.move_cursor(delta);
        self.scroll_to(self.state.cursor(), ScrollAlign::Nearest);
    }

    /// Puts the cursor on `row` and brings it into sight.
    fn go_to(&mut self, row: usize) {
        self.state.set_cursor(row);
        self.scroll_to(self.state.cursor(), ScrollAlign::Nearest);
    }

    // --------------------------------------------------------------------
    // Pointer
    // --------------------------------------------------------------------

    /// Reacts to pointer `input`: clicks on rows and the wheel over the
    /// list. A press or the wheel over the list, or the window losing
    /// focus, drops a pending count or prefix.
    fn handle_pointer(&mut self, input: &Input, cx: &mut EventCx<'_>) -> FlowListResponse {
        let over = self.is_pointer_inside();
        if self.keys.reset_on(input, over) {
            cx.request_redraw();
        }
        match input {
            Input::PointerButton(button)
                if button.pressed && button.button == PointerButton::Left =>
            {
                match self.is_row_pointer {
                    true => self.press_in_row(cx, button),
                    false => self.press(cx, button),
                }
            }
            Input::PointerButton(button)
                if self.is_row_pressed && button.button == PointerButton::Left =>
            {
                self.is_row_pressed = false;
                self.pointer = Some(button.position);
                self.row_pointer(RowPointerKind::Release, button.position, button.mods, cx)
            }
            Input::PointerLeft => {
                self.pointer = None;
                FlowListResponse::Ignored
            }
            Input::PointerMoved(position) => {
                self.pointer = Some(*position);
                match self.is_row_pressed {
                    true => self.row_pointer(RowPointerKind::Drag, *position, cx.mods(), cx),
                    false => FlowListResponse::Ignored,
                }
            }
            Input::Wheel(wheel) if over => {
                let line = self
                    .layout
                    .as_ref()
                    .map_or(MIN_ROW_HEIGHT, |layout| layout.line_height);
                let delta = match wheel.delta {
                    WheelDelta::Lines(lines) => -lines.y * WHEEL_LINES * line,
                    WheelDelta::Pixels(pixels) => -pixels.y,
                };
                if self.scroll_by(delta) {
                    cx.request_redraw();
                }
                FlowListResponse::Handled
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
            | Input::Wheel(_) => FlowListResponse::Ignored,
        }
    }

    /// A left press: moves the cursor to the row under the pointer, toggles
    /// it with Ctrl, selects from the anchor with Shift, or activates it on
    /// a double click.
    fn press(&mut self, cx: &mut EventCx<'_>, button: &ButtonInput) -> FlowListResponse {
        let point = button.position;
        self.pointer = Some(point);
        let Some(layout) = &self.layout else {
            return FlowListResponse::Ignored;
        };
        if !layout.area.contains(point) {
            return FlowListResponse::Ignored;
        }
        let scale_factor = layout.scale_factor;
        let Some(row) = layout.row_at(point) else {
            return FlowListResponse::Handled;
        };
        cx.request_redraw();
        let now = clock::now();
        let same_row = self.state.cursor() == row;
        if same_row && is_double_click(self.last_press, now, point, scale_factor) {
            self.last_press = None;
            return FlowListResponse::Activated(row);
        }
        self.last_press = Some((now, point));
        if button.mods.contains(Mods::SHIFT) {
            if self.state.anchor().is_none() {
                self.state.set_anchor(Some(self.state.cursor()));
            }
            self.state.set_cursor(row);
            self.state.select_to(row);
            return FlowListResponse::CursorMoved(row);
        }
        self.state.set_cursor(row);
        if button.mods.contains(Mods::CTRL) {
            self.state.toggle_selected(row);
        }
        if !self.state.is_ranging() {
            self.state.set_anchor(Some(row));
        }
        FlowListResponse::CursorMoved(row)
    }

    /// A left press in a list built [`FlowList::with_row_pointer`]: handed
    /// to the app with the row under the pointer, starting a drag.
    fn press_in_row(&mut self, cx: &mut EventCx<'_>, button: &ButtonInput) -> FlowListResponse {
        let point = button.position;
        self.pointer = Some(point);
        let Some(layout) = &self.layout else {
            return FlowListResponse::Ignored;
        };
        if !layout.area.contains(point) {
            return FlowListResponse::Ignored;
        }
        if layout.row_at(point).is_none() {
            return FlowListResponse::Handled;
        }
        self.is_row_pressed = true;
        self.row_pointer(RowPointerKind::Press, point, button.mods, cx)
    }

    /// The [`RowPointer`] of `kind` at `point` with `mods` held, in the row
    /// under it or the nearest row drawn.
    fn row_pointer(
        &self,
        kind: RowPointerKind,
        point: Point,
        mods: Mods,
        cx: &EventCx<'_>,
    ) -> FlowListResponse {
        let Some((row, rect)) = self
            .layout
            .as_ref()
            .and_then(|layout| layout.nearest_row(point))
        else {
            return FlowListResponse::Handled;
        };
        FlowListResponse::RowPointer(RowPointer {
            clicks: cx.click_count(),
            kind,
            mods,
            point: Point::new(point.x - rect.x, point.y - rect.y),
            row,
        })
    }

    /// Whether the pointer is over the list as last drawn.
    fn is_pointer_inside(&self) -> bool {
        match (self.pointer, &self.layout) {
            (Some(point), Some(layout)) => layout.area.contains(point),
            (None, _) | (_, None) => false,
        }
    }

    // --------------------------------------------------------------------
    // Scroll
    // --------------------------------------------------------------------

    /// Scrolls `delta` pixels down (up when negative), with the heights
    /// known or estimated, dragging the cursor into sight; whether the
    /// view moved.
    fn scroll_by(&mut self, delta: f32) -> bool {
        let before = self.scroll;
        let count = self.state.count();
        let viewport = self.viewport();
        let estimate = self.estimate();
        let heights = &self.heights;
        let mut height = |row: usize| heights.get(&row).copied().unwrap_or(estimate);
        self.scroll.by(delta, count, &mut height);
        self.scroll.clamp_end(count, viewport, &mut height);
        self.reveal = None;
        self.cursor_into_sight();
        self.scroll != before
    }

    /// Moves the cursor to the nearest row wholly in sight when it is not
    /// in sight, as the view scrolled away from it.
    fn cursor_into_sight(&mut self) {
        let whole = self.whole_rows_in_sight();
        if whole.is_empty() {
            return;
        }
        let cursor = self.state.cursor();
        let row = cursor.clamp(whole.start, whole.end - 1);
        if row != cursor {
            self.state.set_cursor(row);
        }
    }

    /// The rows wholly in sight at the scroll now, with the heights known
    /// or estimated; the top row alone when none is whole.
    fn whole_rows_in_sight(&self) -> Range<usize> {
        let count = self.state.count();
        if count == 0 {
            return 0..0;
        }
        let viewport = self.viewport();
        let top = self.scroll.top;
        let mut whole: Option<Range<usize>> = None;
        let mut y = -self.scroll.offset;
        for row in top..count {
            let row_top = y;
            y += self.height_or_estimate(row);
            if row_top >= 0.0 && y <= viewport {
                let start = whole.map_or(row, |whole| whole.start);
                whole = Some(start..row + 1);
            }
            if y >= viewport {
                break;
            }
        }
        whole.unwrap_or(top..top + 1)
    }

    /// The height of `row` as measured, or the average when it was not.
    fn height_or_estimate(&self, row: usize) -> f32 {
        self.heights
            .get(&row)
            .copied()
            .unwrap_or_else(|| self.estimate())
    }

    /// The height of a row not measured yet: the average of those that
    /// were, or a line of text before any was.
    fn estimate(&self) -> f32 {
        match self.heights.len() {
            0 => self
                .layout
                .as_ref()
                .map_or(MIN_ROW_HEIGHT, |layout| layout.line_height)
                .max(MIN_ROW_HEIGHT),
            measured => self.heights.values().sum::<f32>() / measured as f32,
        }
    }

    /// The height of the list as last drawn; 0 before the first frame.
    fn viewport(&self) -> f32 {
        self.layout
            .as_ref()
            .map_or(0.0, |layout| layout.area.height)
    }

    // --------------------------------------------------------------------
    // Drawing
    // --------------------------------------------------------------------

    /// Draws the rows of `source` in sight in `area`, as the list's
    /// [`Widget::draw`] does.
    fn draw_at(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        source: &(impl FlowSource + ?Sized),
        theme: &Theme,
    ) {
        let count = source.row_count();
        self.state.set_count(count);
        if count == 0 {
            self.scroll = FlowScroll::default();
        }
        self.scroll.top = self.scroll.top.min(count.saturating_sub(1));
        let width = area.width;
        if width != self.width {
            let old = self.heights.get(&self.scroll.top).copied();
            self.heights.clear();
            self.width = width;
            if let (Some(old), true) = (old, count > 0) {
                let new = measured(&mut self.heights, frame, source, self.scroll.top, width);
                // The same share of the top row stays scrolled away.
                self.scroll.offset *= new / old.max(MIN_ROW_HEIGHT);
            }
        }
        let rows = {
            let heights = &mut self.heights;
            let frame: &Frame = frame;
            let mut height = |row: usize| measured(heights, frame, source, row, width);
            if let Some((row, align)) = self.reveal.take()
                && row < count
            {
                self.scroll.place(row, align, area.height, &mut height);
            }
            self.scroll.by(0.0, count, &mut height);
            self.scroll.clamp_end(count, area.height, &mut height);
            self.scroll.rows_in_sight(count, area.height, &mut height)
        };
        let scale_factor = frame.scale_factor();
        let bar = logical(CURSOR_BAR, scale_factor);
        let drawn: Vec<(usize, Rect)> = rows
            .into_iter()
            .map(|(row, y, height)| (row, Rect::new(area.x, area.y + y, area.width, height)))
            .collect();
        frame.clipped(area, |frame| {
            for &(row, rect) in &drawn {
                let is_cursor = row == self.state.cursor();
                if self.state.is_selected(row) {
                    frame.draw_box(
                        rect,
                        &theme.shape.highlight(theme.selected_row, scale_factor),
                    );
                } else if is_cursor {
                    frame.draw_box(rect, &theme.shape.highlight(theme.hover, scale_factor));
                }
                source.draw_row(frame, row, rect, theme);
                // After the row's content, which may fill all of the row.
                if is_cursor {
                    frame.fill_rect_over_text(
                        Rect::new(rect.x, rect.y, bar, rect.height),
                        theme.accent,
                    );
                }
            }
        });
        self.layout = Some(FlowLayout {
            area,
            line_height: frame.cell_metrics().height,
            rows: drawn,
            scale_factor,
        });
        if std::mem::take(&mut self.is_cursor_settling) {
            self.cursor_into_sight();
        }
    }

    /// Describes the list as last drawn to screen readers as `id`: a list
    /// holding the rows in sight (row `index` is the `index`th child of
    /// `id`), each named by [`FlowSource::row_label`] and selected when
    /// selected; the cursor's row has the focus while the list does.
    /// Nothing before the first frame.
    pub fn access(&self, frame: &mut Frame, id: FocusId, source: &(impl FlowSource + ?Sized)) {
        let Some(layout) = &self.layout else {
            return;
        };
        frame.enter_access(id, layout.area, Node::new(Role::List));
        for &(row, rect) in &layout.rows {
            let mut node = Node::new(Role::ListItem);
            node.set_label(source.row_label(row).into_owned());
            node.set_selected(self.state.is_selected(row));
            frame.access(id.child(row as u64), rect, node);
        }
        frame.leave_access();
        if self.visible_range().contains(&self.state.cursor()) {
            let cursor = id.child(self.state.cursor() as u64);
            frame.access.set_inner_focus(id, cursor);
        }
    }
}

impl<S: FlowSource + ?Sized> Widget for Lent<'_, FlowList, &S> {
    type Output = ();
    type Response = FlowListResponse;

    /// Reacts to `input`, as the module documentation lists: keys while
    /// the list has the keyboard, and the pointer. Asks for a redraw
    /// whenever what is drawn changed.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> FlowListResponse {
        self.widget.keys.set_scheme(cx.key_scheme());
        let Some(key) = input.key() else {
            return self.widget.handle_pointer(input, cx);
        };
        let response = self.widget.handle_key(key);
        if !matches!(response, FlowListResponse::Ignored) {
            cx.request_redraw();
        }
        response
    }

    /// Draws the rows in sight in `area`, each by the source over its
    /// background: selected rows on the selection color, the cursor's row
    /// on the hover color with an accent bar at its left.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        self.widget.draw_at(frame, area, self.model, theme);
    }
}

impl Outcome for FlowListResponse {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

// ========================================================================
// FlowScroll Implementations
// ========================================================================

impl FlowScroll {
    /// Scrolls `delta` pixels down (up when negative) over `count` rows as
    /// tall as `height` says, stopping at the first row's top and the last
    /// row's top.
    fn by(&mut self, delta: f32, count: usize, height: &mut impl FnMut(usize) -> f32) {
        if count == 0 {
            *self = Self::default();
            return;
        }
        let mut offset = self.offset + delta;
        while offset < 0.0 && self.top > 0 {
            self.top -= 1;
            offset += height(self.top);
        }
        while self.top + 1 < count && offset >= height(self.top) {
            offset -= height(self.top);
            self.top += 1;
        }
        self.offset = offset.clamp(0.0, (height(self.top) - MIN_ROW_HEIGHT).max(0.0));
    }

    /// Scrolls back so the last row's bottom is no higher than the bottom
    /// of a list `viewport` pixels tall, unless all the rows fit.
    fn clamp_end(&mut self, count: usize, viewport: f32, height: &mut impl FnMut(usize) -> f32) {
        let mut below = -self.offset;
        for row in self.top..count {
            below += height(row);
            if below >= viewport {
                return;
            }
        }
        self.by(below - viewport, count, height);
    }

    /// Scrolls `row` into a list `viewport` pixels tall as `align` says.
    fn place(
        &mut self,
        row: usize,
        align: ScrollAlign,
        viewport: f32,
        height: &mut impl FnMut(usize) -> f32,
    ) {
        let tall = height(row);
        let align = match align {
            ScrollAlign::Nearest => match self.nearest_edge(row, viewport, height) {
                Some(_) if tall >= viewport => ScrollAlign::Top,
                Some(edge) => edge,
                None => return,
            },
            ScrollAlign::Bottom | ScrollAlign::Center | ScrollAlign::Top => align,
        };
        let above = match align {
            ScrollAlign::Bottom => viewport - tall,
            ScrollAlign::Center => (viewport - tall) / 2.0,
            ScrollAlign::Nearest | ScrollAlign::Top => 0.0,
        };
        *self = Self {
            offset: 0.0,
            top: row,
        };
        let count = row + 1;
        self.by(-above.max(0.0), count, height);
        if above < 0.0 {
            // Taller than the list: scrolled into it as far as `align` says.
            self.offset = -above;
        }
    }

    /// The edge `row` must be brought to to show it whole in a list
    /// `viewport` pixels tall, or `None` when it shows whole already.
    fn nearest_edge(
        &self,
        row: usize,
        viewport: f32,
        height: &mut impl FnMut(usize) -> f32,
    ) -> Option<ScrollAlign> {
        if row < self.top || (row == self.top && self.offset > 0.0) {
            return Some(ScrollAlign::Top);
        }
        let mut bottom = -self.offset;
        for above in self.top..=row {
            bottom += height(above);
            if bottom > viewport {
                return Some(ScrollAlign::Bottom);
            }
        }
        None
    }

    /// The rows of `count` in sight in a list `viewport` pixels tall: each
    /// row with its top, from the list's top edge, and its height.
    fn rows_in_sight(
        &self,
        count: usize,
        viewport: f32,
        height: &mut impl FnMut(usize) -> f32,
    ) -> Vec<(usize, f32, f32)> {
        let mut rows = Vec::new();
        let mut y = -self.offset;
        let mut row = self.top;
        while row < count && y < viewport {
            let tall = height(row);
            rows.push((row, y, tall));
            y += tall;
            row += 1;
        }
        rows
    }
}

// ========================================================================
// FlowLayout Implementations
// ========================================================================

impl FlowLayout {
    /// The row drawn under `point`, or `None` below the last row.
    fn row_at(&self, point: Point) -> Option<usize> {
        self.rows
            .iter()
            .find(|(_, rect)| rect.contains(point))
            .map(|&(row, _)| row)
    }

    /// The row drawn under `point`'s height with where it was drawn: the
    /// first row above them all, the last below them all; `None` when no
    /// row was drawn.
    fn nearest_row(&self, point: Point) -> Option<(usize, Rect)> {
        let (first, last) = (self.rows.first()?, self.rows.last()?);
        let found = self
            .rows
            .iter()
            .find(|(_, rect)| point.y >= rect.y && point.y < rect.y + rect.height);
        Some(*found.unwrap_or(match point.y < first.1.y {
            true => first,
            false => last,
        }))
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The height of `row` of `source` at `width`, from `heights` or measured
/// in `frame` and kept there.
fn measured(
    heights: &mut HashMap<usize, f32>,
    frame: &Frame,
    source: &(impl FlowSource + ?Sized),
    row: usize,
    width: f32,
) -> f32 {
    *heights
        .entry(row)
        .or_insert_with(|| source.row_height(frame, row, width).max(MIN_ROW_HEIGHT))
}

/// `rows` as a signed row delta, held to the largest one.
fn signed(rows: usize) -> isize {
    isize::try_from(rows).unwrap_or(isize::MAX)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::offscreen;
    use crate::base::key::parse_key;
    use crate::testing::TestWindow;

    /// Rows 10, 20, then 30 pixels tall, over and over, unless `tall`
    /// says another height for every row.
    struct Rows {
        /// How many rows there are.
        count: usize,
        /// A height for every row, in place of the pattern.
        tall: Option<f32>,
    }

    impl FlowSource for Rows {
        fn row_count(&self) -> usize {
            self.count
        }

        fn row_height(&self, _frame: &Frame, row: usize, width: f32) -> f32 {
            let base = self.tall.unwrap_or(((row % 3) + 1) as f32 * 10.0);
            // Narrower rows wrap taller, as paragraphs do.
            base * (400.0 / width)
        }

        fn draw_row(&self, _frame: &mut Frame, _row: usize, _rect: Rect, _theme: &Theme) {}

        fn row_label(&self, row: usize) -> Cow<'_, str> {
            Cow::Owned(format!("row {row}"))
        }
    }

    /// Draws `list` over `rows` in a list `width` by `height` pixels.
    fn draw(list: &mut FlowList, rows: &Rows, width: f32, height: f32) {
        let mut frame = offscreen::plain_frame(800, 800);
        let area = Rect::new(0.0, 0.0, width, height);
        list.lend(rows).draw(&mut frame, area, &Theme::dark());
    }

    /// Feeds the key of `spec` to `list` lent `rows`.
    fn press_key(
        window: &mut TestWindow,
        list: &mut FlowList,
        rows: &Rows,
        spec: &str,
    ) -> FlowListResponse {
        let key = Input::Key(parse_key(spec).expect("a key"));
        window.feed(&mut list.lend(rows), &key)
    }

    #[test]
    fn test_rows_are_laid_out_by_their_own_heights() {
        let rows = Rows {
            count: 100,
            tall: None,
        };
        let mut list = FlowList::new();
        draw(&mut list, &rows, 400.0, 100.0);
        assert_eq!(
            list.visible_range(),
            0..6,
            "10 + 20 + 30 + 10 + 20 reach 90"
        );
        assert_eq!(list.row_rect(2), Some(Rect::new(0.0, 30.0, 400.0, 30.0)));
        assert_eq!(list.row_rect(5).map(|rect| rect.y), Some(90.0));
        assert_eq!(list.row_rect(6), None);
    }

    #[test]
    fn test_moving_down_scrolls_the_cursor_row_just_into_sight() {
        let rows = Rows {
            count: 100,
            tall: None,
        };
        let mut window = TestWindow::keyboard_first(800, 800);
        let mut list = FlowList::new();
        draw(&mut list, &rows, 400.0, 100.0);
        assert_eq!(
            press_key(&mut window, &mut list, &rows, "5"),
            FlowListResponse::Handled
        );
        assert_eq!(
            press_key(&mut window, &mut list, &rows, "j"),
            FlowListResponse::CursorMoved(5)
        );
        draw(&mut list, &rows, 400.0, 100.0);
        assert_eq!(list.visible_range(), 1..6, "row 5 ran from 90 to 120");
        let rect = list.row_rect(5).expect("in sight");
        assert_eq!(rect.y + rect.height, 100.0, "its bottom at the bottom edge");
        press_key(&mut window, &mut list, &rows, "g");
        press_key(&mut window, &mut list, &rows, "g");
        draw(&mut list, &rows, 400.0, 100.0);
        assert_eq!(list.row_rect(0).map(|rect| rect.y), Some(0.0));
    }

    #[test]
    fn test_scroll_to_places_a_row_at_the_top_middle_or_bottom() {
        let rows = Rows {
            count: 100,
            tall: Some(20.0),
        };
        let mut list = FlowList::new();
        draw(&mut list, &rows, 400.0, 100.0);
        let top_of = |list: &FlowList, row| list.row_rect(row).map(|rect| rect.y);
        list.scroll_to(50, ScrollAlign::Top);
        draw(&mut list, &rows, 400.0, 100.0);
        assert_eq!(top_of(&list, 50), Some(0.0));
        list.scroll_to(50, ScrollAlign::Center);
        draw(&mut list, &rows, 400.0, 100.0);
        assert_eq!(top_of(&list, 50), Some(40.0));
        list.scroll_to(50, ScrollAlign::Bottom);
        draw(&mut list, &rows, 400.0, 100.0);
        assert_eq!(top_of(&list, 50), Some(80.0));
        list.scroll_to(48, ScrollAlign::Nearest);
        draw(&mut list, &rows, 400.0, 100.0);
        assert_eq!(
            top_of(&list, 50),
            Some(80.0),
            "48 was in sight: nothing moves"
        );
        list.scroll_to(99, ScrollAlign::Top);
        draw(&mut list, &rows, 400.0, 100.0);
        assert_eq!(top_of(&list, 99), Some(80.0), "never past the last row");
    }

    #[test]
    fn test_the_top_row_stays_on_top_when_the_width_changes() {
        let rows = Rows {
            count: 100,
            tall: None,
        };
        let mut list = FlowList::new();
        draw(&mut list, &rows, 400.0, 100.0);
        list.scroll_to(40, ScrollAlign::Top);
        draw(&mut list, &rows, 400.0, 100.0);
        assert_eq!(list.visible_range().start, 40);
        draw(&mut list, &rows, 200.0, 100.0);
        assert_eq!(list.visible_range().start, 40, "rows twice as tall now");
        assert_eq!(list.row_rect(40).map(|rect| rect.height), Some(40.0));
    }

    #[test]
    fn test_the_wheel_scrolls_by_pixels_and_drags_the_cursor_along() {
        let rows = Rows {
            count: 100,
            tall: Some(20.0),
        };
        let mut window = TestWindow::keyboard_first(800, 800);
        let mut list = FlowList::new();
        draw(&mut list, &rows, 400.0, 100.0);
        window.feed(
            &mut list.lend(&rows),
            &Input::PointerMoved(Point::new(10.0, 10.0)),
        );
        let wheel = Input::Wheel(crate::input::event::WheelInput::new(
            WheelDelta::Pixels(Point::new(0.0, -50.0)),
            Point::new(10.0, 10.0),
        ));
        assert_eq!(
            window.feed(&mut list.lend(&rows), &wheel),
            FlowListResponse::Handled
        );
        draw(&mut list, &rows, 400.0, 100.0);
        assert_eq!(list.row_rect(2).map(|rect| rect.y), Some(-10.0));
        assert_eq!(list.state().cursor(), 3, "the first row wholly in sight");
    }

    #[test]
    fn test_shift_down_extends_and_page_down_reaches_the_end() {
        let rows = Rows {
            count: 30,
            tall: Some(20.0),
        };
        let mut window = TestWindow::keyboard_first(800, 800);
        let mut list = FlowList::new();
        draw(&mut list, &rows, 400.0, 100.0);
        press_key(&mut window, &mut list, &rows, "S-<down>");
        press_key(&mut window, &mut list, &rows, "S-<down>");
        assert_eq!(list.state().selected().collect::<Vec<_>>(), [0, 1, 2]);
        for _ in 0..20 {
            press_key(&mut window, &mut list, &rows, "C-f");
            draw(&mut list, &rows, 400.0, 100.0);
        }
        assert_eq!(list.state().cursor(), 29);
        assert_eq!(list.visible_range(), 25..30);
    }

    #[test]
    fn test_a_saved_scroll_position_restores_the_same_place_in_a_new_list() {
        let rows = Rows {
            count: 100,
            tall: None,
        };
        let mut window = TestWindow::keyboard_first(800, 800);
        let mut list = FlowList::new();
        draw(&mut list, &rows, 400.0, 100.0);
        window.feed(
            &mut list.lend(&rows),
            &Input::PointerMoved(Point::new(10.0, 10.0)),
        );
        let wheel = Input::Wheel(crate::input::event::WheelInput::new(
            WheelDelta::Pixels(Point::new(0.0, -1234.0)),
            Point::new(10.0, 10.0),
        ));
        window.feed(&mut list.lend(&rows), &wheel);
        draw(&mut list, &rows, 400.0, 100.0);
        let saved = list.scroll_position();
        assert_eq!(saved, (61, 14.0), "61 rows of 10, 20, 30 fill 1220");
        let mut restored = FlowList::new();
        restored.set_scroll_position(saved.0, saved.1);
        draw(&mut restored, &rows, 400.0, 100.0);
        assert_eq!(restored.scroll_position(), saved);
        assert_eq!(restored.row_rect(61).map(|rect| rect.y), Some(-14.0));
        assert!(
            restored
                .visible_range()
                .contains(&restored.state().cursor()),
            "the cursor follows into sight"
        );
        restored.set_scroll_position(61, 500.0);
        draw(&mut restored, &rows, 400.0, 100.0);
        assert_eq!(
            restored.scroll_position(),
            (86, 0.0),
            "500 pixels past row 61's top: on through rows of 20, 30, 10"
        );
        restored.set_scroll_position(5000, 0.0);
        draw(&mut restored, &rows, 400.0, 100.0);
        assert_eq!(restored.visible_range().end, 100, "pulled back to the end");
    }

    #[test]
    fn test_row_pointer_hands_presses_and_drags_in_rows_to_the_app() {
        let rows = Rows {
            count: 100,
            tall: None,
        };
        let mut window = TestWindow::keyboard_first(800, 800);
        let mut list = FlowList::new().with_row_pointer();
        draw(&mut list, &rows, 400.0, 100.0);
        let button = |pressed, x, y| {
            let press = ButtonInput::press(PointerButton::Left, Point::new(x, y));
            Input::PointerButton(ButtonInput { pressed, ..press })
        };
        let pointer = |response| match response {
            FlowListResponse::RowPointer(pointer) => {
                (pointer.kind, pointer.row, pointer.point, pointer.clicks)
            }
            other => panic!("not a row pointer: {other:?}"),
        };
        let press = window.feed(&mut list.lend(&rows), &button(true, 15.0, 35.0));
        assert_eq!(
            pointer(press),
            (RowPointerKind::Press, 2, Point::new(15.0, 5.0), 1),
            "row 2 starts at 30"
        );
        let moved = Input::PointerMoved(Point::new(20.0, -5.0));
        let drag = window.feed(&mut list.lend(&rows), &moved);
        assert_eq!(
            pointer(drag),
            (RowPointerKind::Drag, 0, Point::new(20.0, -5.0), 1),
            "above the rows: in the first row drawn"
        );
        let release = window.feed(&mut list.lend(&rows), &button(false, 20.0, 15.0));
        assert_eq!(pointer(release).0, RowPointerKind::Release);
        assert_eq!(pointer(release).1, 1);
        let after = Input::PointerMoved(Point::new(20.0, 40.0));
        assert_eq!(
            window.feed(&mut list.lend(&rows), &after),
            FlowListResponse::Ignored,
            "no drag once released"
        );
        window.feed(&mut list.lend(&rows), &button(true, 20.0, 40.0));
        window.feed(&mut list.lend(&rows), &button(false, 20.0, 40.0));
        let double = window.feed(&mut list.lend(&rows), &button(true, 20.0, 40.0));
        assert_eq!(
            pointer(double).3,
            2,
            "a double click is counted, not activated"
        );
        assert_eq!(list.state().cursor(), 0, "the pointer moved no cursor");
        assert_eq!(list.state().selected().count(), 0);
        assert_eq!(
            press_key(&mut window, &mut list, &rows, "j"),
            FlowListResponse::CursorMoved(1),
            "keys still move the cursor"
        );
    }
}
