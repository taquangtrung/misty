//! [`IconGrid`]: a virtualized grid of thumbnails with labels, laid out in
//! as many columns as the width holds, such as a folder of photos. Only
//! the cells in sight are drawn; their thumbnails come from a
//! [`Thumbnails`] cache that loads them on worker threads, a placeholder
//! showing until each arrives.
//!
//! Keys, through the lent [`Widget::handle`](crate::Widget::handle), are the ones every list-like
//! widget shares, read across a grid as Vim reads lines (see
//! [`List`](crate::List) for the whole set): Up and Down (`k`, `j`,
//! `C-p`, `C-n`) and the paging, scrolling, screen (`H`, `M`, `L`), and
//! `z` keys move over rows keeping the cursor's column; Left and Right
//! (`h`, `l`) move a cell within the row, stopping at its ends as Vim's
//! `h` and `l` do, and `0` and `$` go to the row's first and last cell;
//! `g g` and `G` (Home and End) go to the first and last item, or with a
//! count to the first cell of the count's row; a count repeats a motion;
//! Space toggles the cursor's selection, and Enter activates it; `+` (or
//! `=`) and `-` zoom between the thumbnail sizes. [`IconGrid::keys`]
//! tells what count or prefix is pending. The pointer, through
//! the lent [`Widget::handle`](crate::Widget::handle): a
//! click moves the cursor (Ctrl+click toggles the selection), a double
//! click activates, a drag from empty space draws a rubber band selecting
//! every cell it touches (with Ctrl, added to the selection), the wheel
//! scrolls and Ctrl with the wheel zooms. As in Vim, a press of any
//! button or the wheel over the grid drops a pending count or prefix, as
//! does the window losing focus.
//!
//! A label is plain text unless the source gives a [`RowContent`] with an
//! icon, styled text, and badges ([`GridSource::item_content`]).
//! [`IconGrid::item_rect`] and [`IconGrid::visible_range`] tell where
//! items were drawn, to place hint labels or a rename field.
//!
//! The rows glide to a new scroll rather than jump, as a
//! [`List`](crate::List)'s do; call the lent grid's
//! [`Widget::about_to_wait`] so each step is drawn. While motion is
//! reduced ([`Frame::reduced_motion`]) they jump.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::hash::Hash;
use std::ops::Range;
use std::time::Instant;

use accesskit::{Node, Role};

use super::super::app::context::EventCx;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::{Lent, Outcome, Widget};
use super::super::base::box_style::{Border, BoxStyle};
use super::super::base::clock;
use super::super::base::color::Rgba;
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::{Key, KeyCode, Mods};
use super::super::base::list::{ListState, scroll_to_show};
use super::super::base::theme::Theme;
use super::super::input::click::is_double_click;
use super::super::input::event::{Input, PointerButton, WheelDelta};
use super::super::render::icon::{logical, outline};
use super::super::render::paragraph::TextAlign;
use super::band::{RubberBand, band_rect, draw_band};
use super::glide::{RowGlide, rows_in_sight};
use super::list_keys::{CountedKey, ListKey, ListKeys, Motion, Read};
use super::row_content::{RowContent, RowFont, RowIcon, draw_content_in, draw_row_icon};
use super::thumbnails::{Thumbnail, Thumbnails};

// ========================================================================
// Constants
// ========================================================================

/// The fill of the cursor's outline, which leaves the cell showing.
const CLEAR: Rgba = Rgba::new(0, 0, 0, 0);

/// The sides of the thumbnails at each zoom step, in logical pixels,
/// unless the app gives its own ([`IconGrid::with_zoom_steps`]).
const ZOOM_STEPS: [f32; 5] = [48.0, 72.0, 96.0, 128.0, 192.0];

/// The zoom step a grid starts at.
const DEFAULT_ZOOM: usize = 2;

/// The space around a thumbnail inside its cell, in logical pixels.
const CELL_PADDING: f32 = 6.0;

/// The thickness of the cursor's outline and the placeholders', in
/// logical pixels.
const STROKE: f32 = 1.0;

/// The thickness of the cursor's outline, in logical pixels.
const CURSOR_STROKE: f32 = 2.0;

/// The rows one notch of a mouse wheel scrolls.
const WHEEL_ROWS: f32 = 1.0;

// ========================================================================
// Data Structures
// ========================================================================

/// A grid of thumbnails with a cursor and a selection. It keeps the
/// cursor, selection, scroll, and zoom, the pointer state between events,
/// and the layout of the last frame it drew. It is a grid, not a list: its
/// cursor and selection are read and set through its own methods
/// ([`IconGrid::set_cursor`], [`IconGrid::set_selected`]), which know the
/// grid's columns, and not through a [`ListState`].
#[derive(Clone, Debug)]
pub struct IconGrid {
    /// The rubber band being drawn, if any.
    band: Option<GridBand>,
    /// How many items there were at the last frame.
    count: usize,
    /// The font the labels are set in.
    font: RowFont,
    /// The index of the item at the cursor.
    cursor: usize,
    /// The scroll as drawn, in rows, gliding to `scroll`.
    glide: RowGlide,
    /// When and where the last press on an item was, for double clicks.
    last_press: Option<(Instant, Point)>,
    /// The count and prefix typed toward a motion.
    keys: ListKeys,
    /// Where the cells were in the last frame, or `None` before it.
    layout: Option<GridLayout>,
    /// The narrowest a cell is, in logical pixels, so a label has room
    /// under a small picture.
    min_cell_width: f32,
    /// Where the pointer is, or `None` when it is outside the window.
    pointer: Option<Point>,
    /// Whether the next frame scrolls the cursor into sight.
    reveal: bool,
    /// The top row to show, which the drawn scroll glides to.
    scroll: usize,
    /// Wheel movement in pixels not yet turned into a whole row of scroll.
    scroll_rest: f32,
    /// The indices of the selected items, kept when the items shrink.
    selected: BTreeSet<usize>,
    /// The index of the zoom step in `zoom_steps`.
    zoom: usize,
    /// The sides of the pictures at each zoom step, in logical pixels,
    /// smallest first.
    zoom_steps: Vec<f32>,
}

/// What an [`IconGrid`] is lent to show: the items and the pool their
/// thumbnails load in. Made by [`IconGrid::lend`].
pub struct GridData<'a, S: GridSource + ?Sized> {
    /// The items.
    pub(crate) source: &'a S,
    /// Their thumbnails.
    pub(crate) thumbnails: &'a mut Thumbnails<S::Key>,
}

/// What became of a key or event offered to an [`IconGrid`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum IconGridResponse {
    /// This item was activated (Enter, a double click): open it.
    Activated(usize),
    /// The grid used the key or event.
    Handled,
    /// Not the grid's.
    Ignored,
}

/// A rubber band being drawn over a grid's cells.
#[derive(Clone, Debug, PartialEq)]
struct GridBand {
    /// The band, started in content pixels (from the top of row 0).
    band: RubberBand,
    /// Where the pointer is, in content pixels.
    end: Point,
}

/// Where a grid's cells were drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
struct GridLayout {
    /// The whole grid.
    area: Rect,
    /// The height of a cell.
    cell_height: f32,
    /// The width of a cell.
    cell_width: f32,
    /// How many cells a row holds.
    columns: usize,
    /// The row at the top edge as drawn, with the share of it scrolled
    /// out of sight.
    offset: f32,
    /// Physical pixels per logical pixel.
    scale_factor: f64,
}

// ========================================================================
// Traits
// ========================================================================

/// The items of an [`IconGrid`], supplied by the app.
pub trait GridSource {
    /// What the items' thumbnails are cached by.
    type Key: Clone + Eq + Hash + Send + 'static;

    /// How many items there are.
    fn item_count(&self) -> usize;

    /// What the thumbnail of item `index` is cached and loaded by.
    fn item_key(&self, index: usize) -> Self::Key;

    /// The label under item `index`. Asked only for items in sight, and
    /// for what screen readers say.
    fn item_label(&self, index: usize) -> Cow<'_, str>;

    /// The icon drawn in item `index`'s square in place of a thumbnail,
    /// for an item with no picture of its own (a folder, a document); the
    /// thumbnail pool is never asked for it. Asked only for items in
    /// sight. By default none: every item is a thumbnail.
    fn item_icon(&self, _index: usize) -> Option<RowIcon> {
        None
    }

    /// What the line under item `index` shows: an icon, styled text, and
    /// badges at its end. Asked only for items in sight. By default
    /// [`GridSource::item_label`] in the foreground color.
    fn item_content(&self, index: usize) -> RowContent {
        RowContent::plain(self.item_label(index))
    }
}

// ========================================================================
// IconGrid Implementations
// ========================================================================

impl IconGrid {
    /// The grid lent `source`'s items and the `thumbnails` pool they load
    /// in, as a [`Widget`] to hand inputs and draw.
    pub fn lend<'a, S: GridSource + ?Sized>(
        &'a mut self,
        source: &'a S,
        thumbnails: &'a mut Thumbnails<S::Key>,
    ) -> Lent<'a, Self, GridData<'a, S>> {
        Lent::new(self, GridData { source, thumbnails })
    }

    // --------------------------------------------------------------------
    // State
    // --------------------------------------------------------------------

    /// An empty grid at the middle zoom step.
    pub fn new() -> Self {
        Self {
            band: None,
            count: 0,
            cursor: 0,
            font: RowFont::Cells,
            glide: RowGlide::default(),
            keys: ListKeys::default(),
            last_press: None,
            layout: None,
            min_cell_width: 0.0,
            pointer: None,
            reveal: false,
            scroll: 0,
            scroll_rest: 0.0,
            selected: BTreeSet::new(),
            zoom: DEFAULT_ZOOM,
            zoom_steps: ZOOM_STEPS.to_vec(),
        }
    }

    /// This grid zooming through `steps`, the sides of its pictures in
    /// logical pixels, smallest first, in place of misty's own; its zoom
    /// held to the steps there are. Empty `steps` are ignored.
    pub fn with_zoom_steps(mut self, steps: impl Into<Vec<f32>>) -> Self {
        let steps = steps.into();
        if !steps.is_empty() {
            self.zoom = self.zoom.min(steps.len() - 1);
            self.zoom_steps = steps;
        }
        self
    }

    /// This grid's cells at least `width` logical pixels wide, the picture
    /// centered in a wider one, so a label has room under a small picture.
    pub fn with_min_cell_width(self, width: f32) -> Self {
        Self {
            min_cell_width: width.max(0.0),
            ..self
        }
    }

    /// This grid with its labels laid out in the proportional font, each
    /// character as wide as its shape, as a desktop file manager sets them.
    pub fn with_proportional_font(self) -> Self {
        Self {
            font: RowFont::Proportional,
            ..self
        }
    }

    /// The index of the item at the cursor.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Moves the cursor to item `index` now (held to the items there are at
    /// the next frame) and scrolls it into sight then.
    pub fn set_cursor(&mut self, index: usize) {
        self.cursor = index;
        self.reveal = true;
    }

    /// The indices of the selected items, in order.
    pub fn selected(&self) -> impl Iterator<Item = usize> + '_ {
        self.selected.iter().copied()
    }

    /// Makes the items at `indices` the selection, in place of what was
    /// selected; for restoring a saved selection. Indices past the items
    /// are kept, and show as selected if the items grow to reach them.
    pub fn set_selected(&mut self, indices: impl IntoIterator<Item = usize>) {
        self.selected = indices.into_iter().collect();
    }

    /// Unselects every item.
    pub fn clear_selection(&mut self) {
        self.selected.clear();
    }

    /// The zoom step, from 0 (the smallest thumbnails).
    pub fn zoom(&self) -> usize {
        self.zoom
    }

    /// Goes to zoom step `zoom`, held to the steps there are; for
    /// restoring a saved zoom.
    pub fn set_zoom(&mut self, zoom: usize) {
        let zoom = zoom.min(self.zoom_steps.len() - 1);
        if zoom != self.zoom {
            // Rows of another size: the drawn scroll means nothing now.
            self.glide.jump_next_frame();
        }
        self.zoom = zoom;
        self.reveal = true;
    }

    /// The largest thumbnail side misty's own zoom steps draw, in logical
    /// pixels: the size to give a [`Thumbnails`] cache (times the scale
    /// factor). A grid given its own steps uses its largest instead.
    pub fn max_thumbnail_side() -> f32 {
        ZOOM_STEPS[ZOOM_STEPS.len() - 1]
    }

    // --------------------------------------------------------------------
    // Input
    // --------------------------------------------------------------------

    /// The count and prefix typed toward a motion, to show in a status bar
    /// or a [`WhichKey`](crate::WhichKey) popup.
    pub fn keys(&self) -> &ListKeys {
        &self.keys
    }

    /// The count and prefix typed toward a motion, to drop them with
    /// [`ListKeys::reset`] when the grid loses the keyboard.
    pub fn keys_mut(&mut self) -> &mut ListKeys {
        &mut self.keys
    }

    /// Reacts to `key`, as the module documentation lists. Hand it the
    /// keys of the window while the grid has the focus.
    pub(crate) fn handle_key(&mut self, key: Key) -> IconGridResponse {
        if key.mods == Mods::NONE && self.keys.pending_keys().is_empty() {
            let zoom = match key.code {
                KeyCode::Char('+' | '=') => Some(1),
                KeyCode::Char('-') => Some(-1),
                KeyCode::Backspace
                | KeyCode::Char(_)
                | KeyCode::Delete
                | KeyCode::Down
                | KeyCode::End
                | KeyCode::Enter
                | KeyCode::Esc
                | KeyCode::F(_)
                | KeyCode::Home
                | KeyCode::Insert
                | KeyCode::Left
                | KeyCode::PageDown
                | KeyCode::PageUp
                | KeyCode::Right
                | KeyCode::Tab
                | KeyCode::Up => None,
            };
            if let Some(delta) = zoom {
                self.keys.reset();
                return self.zoom_by(delta);
            }
        }
        let CountedKey { count, key } = match self.keys.read(key) {
            Read::Consumed => return IconGridResponse::Handled,
            Read::Ignored => return IconGridResponse::Ignored,
            Read::Key(counted) => counted,
        };
        let columns = self.layout.map_or(1, |layout| layout.columns);
        let times = count.unwrap_or(1);
        let cursor = self.cursor;
        let last = self.count.saturating_sub(1);
        let row_start = cursor - cursor % columns;
        let row_end = (row_start + columns - 1).min(last);
        let target = match key {
            ListKey::Activate => return IconGridResponse::Activated(cursor),
            ListKey::Extend(_) => return IconGridResponse::Ignored,
            ListKey::Left => cursor.saturating_sub(times).max(row_start),
            ListKey::LineEnd => {
                let row = row_start + (times - 1).saturating_mul(columns);
                (row.min(last - last % columns) + columns - 1).min(last)
            }
            ListKey::LineStart => row_start,
            ListKey::Move(Motion::First) if count.is_none() => 0,
            ListKey::Move(Motion::Last) if count.is_none() => last,
            ListKey::Move(motion) => self.move_rows(motion, count),
            ListKey::Right => cursor.saturating_add(times).min(row_end),
            ListKey::Toggle => {
                toggle(&mut self.selected, cursor);
                return IconGridResponse::Handled;
            }
        };
        self.set_cursor(target);
        IconGridResponse::Handled
    }

    /// Moves the cursor and the scroll by `motion` over whole rows, as a
    /// list moves over its rows, keeping the cursor's column (the first
    /// column for a row a count names); the cursor's new index.
    fn move_rows(&mut self, motion: Motion, count: Option<usize>) -> usize {
        let cursor = self.cursor;
        let Some(layout) = self.layout else {
            return cursor;
        };
        let columns = layout.columns;
        // The rows as a list of their own, for the motions to move over.
        let mut rows = ListState::new(layout.rows(self.count));
        rows.set_margin(0);
        rows.set_visible(layout.visible_rows().max(1));
        rows.scroll_by(isize::try_from(self.scroll).unwrap_or(isize::MAX));
        rows.set_cursor(cursor / columns);
        self.keys.move_cursor(&mut rows, motion, count);
        self.scroll = rows.scroll();
        let names_row = count.is_some() && matches!(motion, Motion::First | Motion::Last);
        let column = if names_row { 0 } else { cursor % columns };
        (rows.cursor() * columns + column).min(self.count.saturating_sub(1))
    }

    /// Reacts to pointer `input`, as the module documentation lists.
    fn handle_pointer(&mut self, input: &Input, cx: &mut EventCx<'_>) -> IconGridResponse {
        if self.keys.reset_on(input, self.is_pointer_inside()) {
            cx.request_redraw();
        }
        let response = match input {
            Input::PointerButton(button) if button.button == PointerButton::Left => {
                self.pointer = Some(button.position);
                match (button.pressed, self.band.is_some()) {
                    (true, _) => {
                        let ctrl = button.mods.contains(Mods::CTRL);
                        self.press_at(button.position, ctrl, clock::now())
                    }
                    (false, true) => {
                        self.band = None;
                        IconGridResponse::Handled
                    }
                    (false, false) => IconGridResponse::Ignored,
                }
            }
            Input::PointerLeft => {
                self.pointer = None;
                IconGridResponse::Ignored
            }
            Input::PointerMoved(point) => {
                self.pointer = Some(*point);
                self.stretch_band(*point)
            }
            Input::Wheel(wheel) if self.is_pointer_inside() => {
                match wheel.mods.contains(Mods::CTRL) {
                    true => {
                        let notches = match wheel.delta {
                            WheelDelta::Lines(lines) => lines.y,
                            WheelDelta::Pixels(pixels) => pixels.y,
                        };
                        self.zoom_by(notches.signum() as isize)
                    }
                    false => {
                        let rows = self.wheel_rows(wheel.delta);
                        self.scroll = self
                            .scroll
                            .saturating_add_signed(rows)
                            .min(self.max_scroll());
                        IconGridResponse::Handled
                    }
                }
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
            | Input::Wheel(_) => IconGridResponse::Ignored,
        };
        if !matches!(response, IconGridResponse::Ignored) {
            cx.request_redraw();
        }
        response
    }

    /// A left press at `point` at `now`, with Ctrl held when `ctrl`: on an
    /// item, moves the cursor there (toggling it with Ctrl) or activates it
    /// on a double click; on empty space in the grid, starts a rubber band.
    fn press_at(&mut self, point: Point, ctrl: bool, now: Instant) -> IconGridResponse {
        let Some(layout) = self.layout.filter(|layout| layout.area.contains(point)) else {
            return IconGridResponse::Ignored;
        };
        let Some(index) = layout.index_at(point, self.count) else {
            let start = layout.content_point(point);
            if !ctrl {
                self.selected.clear();
            }
            self.band = Some(GridBand {
                band: RubberBand::new(start, self.selected.iter().copied()),
                end: start,
            });
            return IconGridResponse::Handled;
        };
        if index == self.cursor && is_double_click(self.last_press, now, point, layout.scale_factor)
        {
            self.last_press = None;
            return IconGridResponse::Activated(index);
        }
        self.last_press = Some((now, point));
        self.cursor = index;
        if ctrl {
            toggle(&mut self.selected, index);
        }
        IconGridResponse::Handled
    }

    /// Stretches the rubber band to `point`, selecting what it touches;
    /// [`IconGridResponse::Ignored`] when none is drawn.
    fn stretch_band(&mut self, point: Point) -> IconGridResponse {
        let (Some(band), Some(layout)) = (&mut self.band, self.layout) else {
            return IconGridResponse::Ignored;
        };
        band.end = layout.content_point(point);
        let rect = band_rect(band.band.start(), band.end);
        let touched = layout.indices_touching(rect, self.count);
        self.selected = band.band.selection(touched).collect();
        IconGridResponse::Handled
    }

    /// Zooms `delta` steps, keeping the cursor in sight.
    fn zoom_by(&mut self, delta: isize) -> IconGridResponse {
        self.set_zoom(self.zoom.saturating_add_signed(delta));
        IconGridResponse::Handled
    }

    /// The whole rows the wheel's `delta` scrolls: [`WHEEL_ROWS`] a notch,
    /// or a row per cell height of pixels, the rest kept for the next.
    fn wheel_rows(&mut self, delta: WheelDelta) -> isize {
        match delta {
            WheelDelta::Lines(lines) => (-lines.y * WHEEL_ROWS).round() as isize,
            WheelDelta::Pixels(pixels) => {
                let height = self
                    .layout
                    .map_or(1.0, |layout| layout.cell_height.max(1.0));
                self.scroll_rest -= pixels.y;
                let whole = (self.scroll_rest / height).trunc();
                self.scroll_rest -= whole * height;
                whole as isize
            }
        }
    }

    /// The last top row that still fills the grid.
    fn max_scroll(&self) -> usize {
        self.layout.map_or(0, |layout| {
            layout
                .rows(self.count)
                .saturating_sub(layout.visible_rows())
        })
    }

    /// Whether the pointer is over the grid as last drawn.
    fn is_pointer_inside(&self) -> bool {
        match (self.pointer, self.layout) {
            (Some(point), Some(layout)) => layout.area.contains(point),
            (None, _) | (_, None) => false,
        }
    }

    // --------------------------------------------------------------------
    // Drawing
    // --------------------------------------------------------------------

    /// Draws the cells of `source` in sight in `area`, as the grid's
    /// [`Widget::draw`] does, its glide where it is at `now`.
    fn draw_at<S: GridSource + ?Sized>(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        source: &S,
        thumbnails: &mut Thumbnails<S::Key>,
        theme: &Theme,
        now: Instant,
    ) {
        let metrics = frame.cell_metrics();
        let scale_factor = frame.scale_factor();
        self.count = source.item_count();
        self.cursor = self.cursor.min(self.count.saturating_sub(1));
        let side = logical(self.zoom_steps[self.zoom], scale_factor);
        let padding = logical(CELL_PADDING, scale_factor);
        let cell_width = (side + 2.0 * padding).max(logical(self.min_cell_width, scale_factor));
        let mut layout = GridLayout {
            area,
            cell_height: side + metrics.height + 3.0 * padding,
            cell_width,
            columns: ((area.width / cell_width).floor() as usize).max(1),
            offset: 0.0,
            scale_factor,
        };
        let count = self.count;
        self.scroll = self
            .scroll
            .min(layout.rows(count).saturating_sub(layout.visible_rows()));
        if self.reveal {
            self.reveal = false;
            let row = self.cursor / layout.columns;
            let visible = layout.visible_rows().max(1);
            self.scroll = scroll_to_show(self.scroll, row, visible);
        }
        self.glide.set_reduced_motion(frame.reduced_motion());
        layout.offset = self.glide.follow(self.scroll, layout.visible_rows(), now);
        self.layout = Some(layout);
        thumbnails.begin_frame();
        let stroke = logical(STROKE, scale_factor);
        let cursor_stroke = logical(CURSOR_STROKE, scale_factor);
        frame.clipped(area, |frame| {
            for index in layout.indices_in_sight(count) {
                let cell = layout.cell_rect(index);
                if self.selected.contains(&index) {
                    frame.draw_box(cell, &theme.shape.highlight(theme.selection, scale_factor));
                }
                let left = (cell.x + (cell.width - side) / 2.0).round();
                let square = Rect::new(left, cell.y + padding, side, side);
                if let Some(icon) = source.item_icon(index) {
                    draw_row_icon(frame, &icon, square, theme);
                } else {
                    match thumbnails.get(&source.item_key(index)) {
                        Thumbnail::Ready(image) => frame.draw_image(image, image.fit(square)),
                        Thumbnail::Loading => {
                            frame.fill_rect(square, theme.surface);
                            for edge in outline(square, stroke) {
                                frame.fill_rect_over_text(edge, theme.border);
                            }
                        }
                        Thumbnail::Failed(_) => {
                            for edge in outline(square, stroke) {
                                frame.fill_rect_over_text(edge, theme.danger);
                            }
                        }
                    }
                }
                let label = Rect::new(
                    cell.x + (padding / 2.0).round(),
                    square.y + side + padding,
                    cell.width - padding,
                    metrics.height,
                );
                draw_content_in(
                    frame,
                    &source.item_content(index),
                    label,
                    TextAlign::Center,
                    theme,
                    self.font,
                );
                if index == self.cursor {
                    frame.draw_box(cell, &cursor_outline(theme, cursor_stroke, scale_factor));
                }
            }
            if let Some(band) = &self.band {
                let rect = layout.screen_rect(band_rect(band.band.start(), band.end));
                draw_band(frame, rect, theme);
            }
        });
    }

    /// Where item `index`'s cell was drawn in the last frame, in physical
    /// pixels, whether in sight or not; `None` before the first frame.
    pub fn item_rect(&self, index: usize) -> Option<Rect> {
        self.layout.map(|layout| layout.cell_rect(index))
    }

    /// The items drawn in the last frame, wholly or in part. Empty before
    /// the first frame.
    pub fn visible_range(&self) -> Range<usize> {
        self.layout
            .map_or(0..0, |layout| layout.indices_in_sight(self.count))
    }

    /// Describes the grid as last drawn to screen readers as `id`: a list
    /// of the items in sight (item `index` is the `index`th child of `id`)
    /// named by their labels, selected ones marked; the cursor's item has
    /// the focus while the grid does. Nothing before the first frame.
    pub fn access(&self, frame: &mut Frame, id: FocusId, source: &(impl GridSource + ?Sized)) {
        let Some(layout) = self.layout else {
            return;
        };
        frame.enter_access(id, layout.area, Node::new(Role::List));
        let in_sight = layout.indices_in_sight(self.count);
        for index in in_sight.clone() {
            let mut node = Node::new(Role::ListItem);
            node.set_label(source.item_label(index).into_owned());
            node.set_selected(self.selected.contains(&index));
            frame.access(id.child(index as u64), layout.cell_rect(index), node);
        }
        frame.leave_access();
        let cursor = self.cursor;
        if in_sight.contains(&cursor) {
            frame.access.set_inner_focus(id, id.child(cursor as u64));
        }
    }
}

impl<S: GridSource + ?Sized> Widget for Lent<'_, IconGrid, GridData<'_, S>> {
    type Output = ();
    type Response = IconGridResponse;

    /// Reacts to `input`, as the module documentation lists: keys while
    /// the grid has the keyboard, and the pointer. Asks for a redraw
    /// whenever what is drawn changed.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> IconGridResponse {
        let response = match input {
            Input::Key(key) => self.widget.handle_key(*key),
            Input::KeyReleased(_)
            | Input::AppearanceChanged(_)
            | Input::CloseRequested
            | Input::FileDropped(_)
            | Input::FileHovered(_)
            | Input::FileHoverCancelled
            | Input::FocusGained
            | Input::FocusLost
            | Input::Ime(_)
            | Input::PointerButton(_)
            | Input::PointerLeft
            | Input::PointerMoved(_)
            | Input::Wheel(_) => return self.widget.handle_pointer(input, cx),
        };
        if !matches!(response, IconGridResponse::Ignored) {
            cx.request_redraw();
        }
        response
    }

    /// Draws the cells in sight in `area` in `theme`'s colors: each
    /// thumbnail fitted in its square (a placeholder while it loads,
    /// outlined in the danger color when it failed), its label under it,
    /// selected cells on the selection color, the cursor's cell outlined
    /// in the accent color, and the rubber band.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let GridData { source, thumbnails } = &mut self.model;
        self.widget
            .draw_at(frame, area, *source, thumbnails, theme, clock::now());
    }

    /// Asks for the next frame of a glide still under way, returning when
    /// it is due; `None` at rest.
    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        self.widget.glide.about_to_wait(cx)
    }
}

impl Outcome for IconGridResponse {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

impl Default for IconGrid {
    fn default() -> Self {
        Self::new()
    }
}

// ========================================================================
// GridLayout Implementations
// ========================================================================

impl GridLayout {
    /// How many rows `count` items fill.
    fn rows(&self, count: usize) -> usize {
        count.div_ceil(self.columns)
    }

    /// How many whole rows fit in the grid.
    fn visible_rows(&self) -> usize {
        (self.area.height / self.cell_height.max(1.0)).floor() as usize
    }

    /// The items with a cell at least partly in sight.
    fn indices_in_sight(&self, count: usize) -> Range<usize> {
        let rows = (self.area.height / self.cell_height.max(1.0)).ceil() as usize;
        let in_sight = rows_in_sight(self.offset, rows, self.rows(count));
        (in_sight.start * self.columns).min(count)..(in_sight.end * self.columns).min(count)
    }

    /// Where item `index`'s cell is on screen.
    fn cell_rect(&self, index: usize) -> Rect {
        let row = (index / self.columns) as f32 - self.offset;
        let column = (index % self.columns) as f32;
        Rect::new(
            self.area.x + column * self.cell_width,
            self.area.y + row * self.cell_height,
            self.cell_width,
            self.cell_height,
        )
    }

    /// The item whose cell holds `point` among `count` items, or `None`
    /// over empty space.
    fn index_at(&self, point: Point, count: usize) -> Option<usize> {
        let content = self.content_point(point);
        if content.x < 0.0 || content.y < 0.0 {
            return None;
        }
        let column = (content.x / self.cell_width) as usize;
        let row = (content.y / self.cell_height) as usize;
        let index = row * self.columns + column;
        (column < self.columns && index < count).then_some(index)
    }

    /// The items among `count` whose cells `rect`, in content pixels,
    /// touches.
    fn indices_touching(&self, rect: Rect, count: usize) -> Vec<usize> {
        let first_column = (rect.x / self.cell_width).floor().max(0.0) as usize;
        let last_column = ((rect.x + rect.width) / self.cell_width).floor().max(0.0) as usize;
        let first_row = (rect.y / self.cell_height).floor().max(0.0) as usize;
        let last_row = ((rect.y + rect.height) / self.cell_height).floor().max(0.0) as usize;
        let mut indices = Vec::new();
        for row in first_row..=last_row {
            for column in first_column..=last_column.min(self.columns - 1) {
                let index = row * self.columns + column;
                if index < count {
                    indices.push(index);
                }
            }
        }
        indices
    }

    /// `point` on screen in content pixels: from the grid's left edge and
    /// the top of row 0.
    fn content_point(&self, point: Point) -> Point {
        Point::new(
            point.x - self.area.x,
            point.y - self.area.y + self.offset * self.cell_height,
        )
    }

    /// `rect` in content pixels on screen.
    fn screen_rect(&self, rect: Rect) -> Rect {
        Rect::new(
            rect.x + self.area.x,
            rect.y + self.area.y - self.offset * self.cell_height,
            rect.width,
            rect.height,
        )
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The cursor's outline, `stroke` thick in the accent color, rounded as the
/// theme rounds its highlights.
fn cursor_outline(theme: &Theme, stroke: f32, scale_factor: f64) -> BoxStyle {
    BoxStyle {
        border: Some(Border {
            color: Rgba::from(theme.accent),
            width: stroke,
        }),
        fill: CLEAR,
        ..theme.shape.highlight(theme.accent, scale_factor)
    }
}

/// Selects `index` when it is not selected, unselects it when it is.
fn toggle(selected: &mut BTreeSet<usize>, index: usize) {
    if !selected.remove(&index) {
        selected.insert(index);
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::app::access::find_node;
    use crate::app::offscreen;
    use crate::base::color::Rgba;
    use crate::base::key::{parse_key, parse_key_sequence};
    use crate::render::image::Image;

    /// Photos numbered from 0.
    struct Photos(usize);

    impl GridSource for Photos {
        type Key = usize;

        fn item_count(&self) -> usize {
            self.0
        }

        fn item_key(&self, index: usize) -> usize {
            index
        }

        fn item_label(&self, index: usize) -> Cow<'_, str> {
            Cow::Owned(format!("photo {index}"))
        }
    }

    /// Cells 100 by 120 in 4 columns, 3 rows in sight, scrolled down
    /// `offset` rows.
    fn layout(offset: f32) -> GridLayout {
        GridLayout {
            area: Rect::new(0.0, 0.0, 450.0, 360.0),
            cell_height: 120.0,
            cell_width: 100.0,
            columns: 4,
            offset,
            scale_factor: 1.0,
        }
    }

    /// A grid of `count` items laid out as [`layout`] at the top.
    fn grid(count: usize) -> IconGrid {
        let mut grid = IconGrid::new();
        grid.layout = Some(layout(0.0));
        grid.count = count;
        grid
    }

    /// A cache whose thumbnails never finish loading.
    fn never_loading() -> Thumbnails<usize> {
        Thumbnails::with_wake(1, 4, 16, |_: &usize| Err("never".to_owned()), || {})
    }

    #[test]
    fn test_a_point_finds_its_cell_counting_the_scroll_and_empty_space_finds_none() {
        let layout = layout(2.0);
        assert_eq!(
            layout.index_at(Point::new(150.0, 10.0), 50),
            Some(9),
            "row 2, column 1"
        );
        assert_eq!(
            layout.index_at(Point::new(420.0, 10.0), 50),
            None,
            "past the columns"
        );
        assert_eq!(
            layout.index_at(Point::new(150.0, 10.0), 9),
            None,
            "past the last item"
        );
        assert_eq!(layout.cell_rect(9), Rect::new(100.0, 0.0, 100.0, 120.0));
        assert_eq!(layout.indices_in_sight(50), 8..20);
    }

    #[test]
    fn test_a_point_mid_glide_finds_the_cell_drawn_under_it() {
        let gliding = layout(2.5);
        assert_eq!(gliding.index_at(Point::new(10.0, 10.0), 50), Some(8));
        assert_eq!(
            gliding.index_at(Point::new(10.0, 70.0), 50),
            Some(12),
            "row 3 starts at 60"
        );
        assert_eq!(gliding.cell_rect(12).y, 60.0);
        assert_eq!(
            gliding.indices_in_sight(50),
            8..24,
            "part rows at both edges"
        );
    }

    #[test]
    fn test_a_rubber_band_mid_glide_selects_the_cells_drawn_under_it() {
        let mut grid = grid(50);
        grid.layout = Some(layout(0.5));
        grid.press_at(Point::new(420.0, 50.0), false, Instant::now());
        grid.stretch_band(Point::new(350.0, 70.0));
        assert_eq!(
            grid.selected().collect::<Vec<_>>(),
            [3, 7],
            "half a row down, y 50 to 70 spans rows 0 and 1"
        );
    }

    #[test]
    fn test_moving_the_cursor_far_glides_the_rows_there_unless_motion_is_reduced() {
        let mut frame = offscreen::plain_frame(800, 600);
        let photos = Photos(1000);
        let area = Rect::new(0.0, 0.0, 800.0, 600.0);
        let theme = Theme::dark();
        let start = Instant::now();
        let mut thumbnails = never_loading();
        let mut grid = IconGrid::new();
        grid.draw_at(&mut frame, area, &photos, &mut thumbnails, &theme, start);
        grid.set_cursor(500);
        grid.draw_at(&mut frame, area, &photos, &mut thumbnails, &theme, start);
        let first = grid.layout.expect("drawn").offset;
        assert!(first < grid.scroll as f32, "a glide starts short: {first}");
        let later = start + Duration::from_secs(5);
        grid.draw_at(&mut frame, area, &photos, &mut thumbnails, &theme, later);
        assert_eq!(grid.layout.expect("drawn").offset, grid.scroll as f32);
        frame.set_reduced_motion(true);
        grid.set_cursor(0);
        grid.draw_at(&mut frame, area, &photos, &mut thumbnails, &theme, later);
        assert_eq!(grid.layout.expect("drawn").offset, 0.0, "a jump");
    }

    #[test]
    fn test_keys_move_by_cells_and_rows_and_stop_at_the_ends() {
        let mut grid = grid(10);
        let mut press = |spec| grid.handle_key(parse_key(spec).expect("a valid key"));
        press("j");
        press("l");
        assert_eq!(grid.cursor(), 5);
        grid.handle_key(parse_key("j").expect("a key"));
        assert_eq!(grid.cursor(), 9, "a short last row: the last item");
        for spec in ["k", "<left>", "<left>"] {
            grid.handle_key(parse_key(spec).expect("a key"));
        }
        assert_eq!(grid.cursor(), 4, "Left stops at the row's start");
        for spec in ["g", "<left>"] {
            grid.handle_key(parse_key(spec).expect("a key"));
        }
        assert_eq!(grid.cursor(), 4, "Left after g only drops the g");
        for spec in ["g", "g"] {
            grid.handle_key(parse_key(spec).expect("a key"));
        }
        assert_eq!(grid.cursor(), 0);
        assert_eq!(
            grid.handle_key(parse_key("<cr>").expect("a key")),
            IconGridResponse::Activated(0)
        );
        assert_eq!(
            grid.handle_key(parse_key("C-j").expect("a key")),
            IconGridResponse::Ignored
        );
    }

    #[test]
    fn test_emacs_keys_move_by_rows_and_half_pages_as_in_a_list() {
        let mut grid = grid(40);
        let mut press = |spec| grid.handle_key(parse_key(spec).expect("a valid key"));
        press("C-n");
        assert_eq!(grid.cursor(), 4, "a row of 4 down");
        grid.handle_key(parse_key("C-d").expect("a key"));
        assert_eq!(grid.cursor(), 8, "half of 3 rows is 1 row");
        for spec in ["3", "C-f"] {
            grid.handle_key(parse_key(spec).expect("a key"));
        }
        assert_eq!(grid.cursor(), 16, "3 screens of 3 rows less 2");
        grid.handle_key(parse_key("C-p").expect("a key"));
        assert_eq!(grid.cursor(), 12);
    }

    #[test]
    fn test_counts_screen_rows_and_line_keys_move_over_rows_and_cells() {
        let mut grid = grid(40);
        let press = |grid: &mut IconGrid, spec: &str| {
            for key in parse_key_sequence(spec).expect("valid keys") {
                grid.handle_key(key);
            }
            grid.cursor()
        };
        assert_eq!(press(&mut grid, "l 2 j"), 9, "the column kept");
        assert_eq!(press(&mut grid, "$"), 11);
        assert_eq!(press(&mut grid, "0"), 8);
        assert_eq!(press(&mut grid, "2 $"), 15, "the end of the next row");
        assert_eq!(press(&mut grid, "L"), 15, "the bottom row in sight");
        assert_eq!(press(&mut grid, "H"), 7, "the top row in sight");
        assert_eq!(press(&mut grid, "5 G"), 16, "row 5, its first cell");
        assert_eq!(press(&mut grid, "l G"), 39, "the last item");
        assert_eq!(press(&mut grid, "1 0 l"), 39, "the row's end");
        assert_eq!(press(&mut grid, "5 -"), 39, "zoom drops the count");
        assert_eq!(press(&mut grid, "j"), 39);
    }

    #[test]
    fn test_zoom_steps_stop_at_the_smallest_and_largest() {
        let mut grid = grid(10);
        for _ in 0..10 {
            grid.handle_key(parse_key("+").expect("a key"));
        }
        assert_eq!(grid.zoom(), ZOOM_STEPS.len() - 1);
        for _ in 0..10 {
            grid.handle_key(parse_key("-").expect("a key"));
        }
        assert_eq!(grid.zoom(), 0);
    }

    #[test]
    fn test_a_rubber_band_from_empty_space_selects_every_cell_it_touches() {
        let mut grid = grid(10);
        grid.selected.insert(9);
        assert_eq!(
            grid.press_at(Point::new(420.0, 10.0), false, Instant::now()),
            IconGridResponse::Handled
        );
        assert!(grid.band.is_some(), "past the columns is empty space");
        grid.stretch_band(Point::new(250.0, 130.0));
        assert_eq!(
            grid.selected().collect::<Vec<_>>(),
            [2, 3, 6, 7],
            "without Ctrl, a new selection"
        );
        grid.stretch_band(Point::new(380.0, 20.0));
        assert_eq!(
            grid.selected().collect::<Vec<_>>(),
            [3],
            "shrunk back to one cell"
        );
    }

    #[test]
    fn test_moving_the_cursor_out_of_sight_scrolls_to_it_and_items_are_described() {
        let id = FocusId::new("photos");
        let mut frame = offscreen::plain_frame(800, 600);
        frame.set_reduced_motion(true);
        let mut grid = IconGrid::new();
        let mut thumbnails = never_loading();
        let photos = Photos(1000);
        let area = Rect::new(0.0, 0.0, 800.0, 600.0);
        grid.lend(&photos, &mut thumbnails)
            .draw(&mut frame, area, &Theme::dark());
        grid.set_cursor(500);
        grid.lend(&photos, &mut thumbnails)
            .draw(&mut frame, area, &Theme::dark());
        let layout = grid.layout.expect("drawn");
        let row = 500 / layout.columns;
        assert!(
            grid.scroll <= row && row < grid.scroll + layout.visible_rows(),
            "in sight"
        );
        grid.access(&mut frame, id, &photos);
        let update = frame.access.tree_update("t");
        let item = find_node(&update, id.child(500)).expect("described");
        assert_eq!(
            (item.role(), item.label()),
            (Role::ListItem, Some("photo 500"))
        );
    }

    #[test]
    fn test_a_loaded_thumbnail_is_drawn_in_its_cell_and_the_cursor_outlined() {
        let theme = Theme::dark();
        let red = vec![[0xff, 0, 0, 0xff]; 64 * 64].concat();
        let image = Image::from_rgba(64, 64, red).expect("an image");
        let mut thumbnails =
            Thumbnails::with_wake(1, 4, 256, move |_: &usize| Ok(image.clone()), || {});
        let start = Instant::now();
        while !matches!(thumbnails.get(&0), Thumbnail::Ready(_)) {
            assert!(start.elapsed().as_secs() < 10, "never loaded");
            thumbnails.poll();
            std::thread::yield_now();
        }
        let mut grid = IconGrid::new();
        let area = Rect::new(0.0, 0.0, 400.0, 300.0);
        let Some(snapshot) = offscreen::render(400, 300, |frame| {
            frame.clear(Rgba::from(theme.background));
            grid.lend(&Photos(3), &mut thumbnails)
                .draw(frame, area, &theme);
        }) else {
            return;
        };
        let middle = CELL_PADDING + ZOOM_STEPS[DEFAULT_ZOOM] / 2.0;
        snapshot.assert_pixel(
            middle,
            middle,
            crate::base::color::Rgb::new(0xff, 0, 0),
            "the photo",
        );
        snapshot.assert_pixel(0.0, middle, theme.accent, "the cursor's outline");
        let second = Rect::new(middle + 100.0, middle - 4.0, 8.0, 8.0);
        assert!(
            snapshot.contains_color(second, theme.surface),
            "a placeholder while loading"
        );
    }

    #[test]
    fn test_a_click_after_set_cursor_before_a_frame_keeps_the_clicked_item() {
        let mut grid = grid(50);
        grid.set_cursor(5);
        assert_eq!(grid.cursor(), 5, "set at once");
        grid.press_at(Point::new(150.0, 10.0), false, Instant::now());
        let mut frame = offscreen::plain_frame(450, 360);
        let area = Rect::new(0.0, 0.0, 450.0, 360.0);
        let mut thumbnails = never_loading();
        let theme = Theme::dark();
        grid.draw_at(
            &mut frame,
            area,
            &Photos(50),
            &mut thumbnails,
            &theme,
            Instant::now(),
        );
        assert_eq!(grid.cursor(), 1, "the click, not the earlier set_cursor");
    }

    #[test]
    fn test_state_set_before_a_frame_survives_and_shrinking_keeps_the_selection() {
        let mut grid = IconGrid::new();
        grid.set_cursor(5);
        grid.set_selected([3, 40]);
        assert_eq!(grid.cursor(), 5, "no frame has counted the items yet");
        let mut frame = offscreen::plain_frame(450, 360);
        let area = Rect::new(0.0, 0.0, 450.0, 360.0);
        let mut thumbnails = never_loading();
        let theme = Theme::dark();
        let now = Instant::now();
        grid.draw_at(&mut frame, area, &Photos(50), &mut thumbnails, &theme, now);
        assert_eq!(grid.cursor(), 5);
        assert_eq!(grid.selected().collect::<Vec<_>>(), [3, 40]);
        grid.draw_at(&mut frame, area, &Photos(10), &mut thumbnails, &theme, now);
        assert_eq!(
            grid.selected().collect::<Vec<_>>(),
            [3, 40],
            "kept for when they return"
        );
    }

    /// Folders numbered from 0, each drawn as an icon.
    struct Folders(usize);

    impl GridSource for Folders {
        type Key = usize;

        fn item_count(&self) -> usize {
            self.0
        }

        fn item_key(&self, index: usize) -> usize {
            index
        }

        fn item_label(&self, index: usize) -> Cow<'_, str> {
            Cow::Owned(format!("folder {index}"))
        }

        fn item_icon(&self, _index: usize) -> Option<RowIcon> {
            let svg = crate::render::image::Svg::parse(
                b"<svg xmlns='http://www.w3.org/2000/svg' width='1' height='1'/>",
            )
            .expect("an svg");
            Some(RowIcon::tinted(svg, crate::base::theme::ThemeToken::Muted))
        }
    }

    #[test]
    fn test_items_drawn_as_icons_never_ask_the_thumbnail_pool() {
        let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = std::sync::Arc::clone(&asked);
        let mut thumbnails = Thumbnails::with_wake(
            1,
            4,
            16,
            move |_: &usize| {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err("unused".to_owned())
            },
            || {},
        );
        let mut grid = IconGrid::new();
        let mut frame = offscreen::plain_frame(400, 300);
        let area = Rect::new(0.0, 0.0, 400.0, 300.0);
        grid.lend(&Folders(6), &mut thumbnails)
            .draw(&mut frame, area, &Theme::dark());
        thumbnails.begin_frame();
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn test_an_apps_zoom_steps_and_minimum_width_shape_the_cells() {
        let mut grid = IconGrid::new()
            .with_zoom_steps([16.0, 32.0])
            .with_min_cell_width(80.0);
        assert_eq!(grid.zoom(), 1, "held to the steps there are");
        grid.set_zoom(5);
        assert_eq!(grid.zoom(), 1);
        grid.set_zoom(0);
        let mut thumbnails = never_loading();
        let mut frame = offscreen::plain_frame(400, 300);
        let area = Rect::new(0.0, 0.0, 400.0, 300.0);
        grid.lend(&Folders(6), &mut thumbnails)
            .draw(&mut frame, area, &Theme::dark());
        let cell = grid.item_rect(1).expect("drawn");
        assert_eq!(cell.width, 80.0, "a 16 pixel picture in an 80 pixel cell");
        assert_eq!(cell.x, 80.0);
    }

    #[test]
    fn test_a_proportional_grid_shapes_its_labels_and_a_plain_one_keeps_them_on_the_grid() {
        let shaped = |mut grid: IconGrid| {
            let mut thumbnails = never_loading();
            let mut frame = offscreen::plain_frame(400, 300);
            let area = Rect::new(0.0, 0.0, 400.0, 300.0);
            grid.lend(&Folders(2), &mut thumbnails)
                .draw(&mut frame, area, &Theme::dark());
            frame
                .draw
                .text()
                .iter()
                .filter(|segment| segment.wrap.is_some())
                .count()
        };
        assert_eq!(shaped(IconGrid::new()), 0, "on the cell grid");
        assert_eq!(
            shaped(IconGrid::new().with_proportional_font()),
            2,
            "a label an item"
        );
    }
}
