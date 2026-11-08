//! [`Table`]: a virtualized table of one-line rows in columns, under a
//! header naming each column. The app supplies the cells through
//! [`TableSource`], asked only for the rows on screen, so a table of a
//! hundred thousand files draws as fast as one of twenty.
//!
//! The rows behave as a [`List`]'s do: the lent [`Widget::handle`](crate::Widget::handle) takes the
//! same Vim list keys, with [`Table::keys`] telling what count or prefix
//! is pending (or the app drives the [`ListState`] methods through
//! [`Table::state_mut`] with its own); a click moves the
//! cursor, Ctrl+click toggles a row's selection, Shift+click and Shift
//! with the arrows select a range from the anchor, a drag from empty space
//! below the rows (or with Alt) draws a rubber band, a double click
//! activates a row, the wheel scrolls, rows glide to a new scroll, and a
//! dragged row is dropped in a gap or elsewhere
//! ([`TableResponse::Dropped`]).
//!
//! The header adds two things. Dragging the border at a column's right
//! edge resizes the column, never below its minimum width;
//! [`Table::column_widths`] and [`Table::set_column_widths`] read and
//! restore the widths, to keep them across runs. A click on a sortable
//! column's title asks to sort by it ([`TableResponse::Sorted`]): the
//! same column again flips the direction. The table sorts nothing itself;
//! the app reorders its rows and the table marks the column it is told is
//! sorted with an arrow.
//!
//! Each cell's text is cut to its column with an ellipsis and aligned as
//! its column says. A cell is plain text unless the source gives a
//! [`RowContent`] with an icon, styled text, and badges
//! ([`TableSource::cell_content`]). [`Table::cell_rect`],
//! [`Table::row_rect`], and [`Table::visible_range`] tell where cells were
//! drawn, to place an inline rename field over one.
//!
//! Columns wider together than the table scroll sideways, the header with
//! the rows: with the wheel while Shift is held, a sideways wheel or
//! touchpad swipe, and Left and Right (`h`, `l`). One column made with
//! [`TableColumn::with_fill`] stretches to take the width the others leave;
//! it still resizes, and what [`Table::column_widths`] saves is the width
//! it was given or last dragged to, never the stretch.

use std::borrow::Cow;
use std::iter;
use std::ops::Range;
use std::time::Instant;

use accesskit::{Node, Role, SortDirection as AccessSortDirection};

use super::super::app::context::EventCx;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::{Lent, Outcome, Widget};
use super::super::base::cursor::CursorIcon;
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::{Key, Mods};
use super::super::base::list::ListState;
use super::super::base::styled::{StyledText, TextColor};
use super::super::base::theme::{Theme, ThemeToken};
use super::super::input::event::{Input, PointerButton, WheelDelta};
use super::super::render::icon::logical;
use super::super::render::paragraph::TextAlign;
use super::drag::Drag;
use super::label::{draw_styled_label, fit_label, styled_label_width};
use super::list_keys::{CountedKey, ListKey, ListKeys, Read};
use super::list_view::{List, ListResponse, RowDrop, RowsToDraw};
use super::row_content::{RowContent, draw_content};

// ========================================================================
// Constants
// ========================================================================

/// How far either side of a column's right edge a press grabs it, in
/// logical pixels.
const GRAB_MARGIN: f32 = 3.0;

/// The narrowest a column may be made unless it says otherwise, in
/// logical pixels.
const DEFAULT_MIN_WIDTH: f32 = 24.0;

/// How far a wheel notch or a Left or Right key scrolls the columns
/// sideways, in logical pixels.
const SIDEWAYS_STEP: f32 = 48.0;

/// The blank cells at each side of a cell's text.
const PADDING_CELLS: f32 = 0.5;

/// The thickness of the header's lines, in logical pixels.
const STROKE: f32 = 1.0;

/// What marks a column sorted ascending.
const ASCENDING_MARK: &str = "▲";

/// What marks a column sorted descending.
const DESCENDING_MARK: &str = "▼";

/// The node id under a table's id of its header row, apart from the
/// rows' ids (their indices).
const HEADER_ROW: u64 = u64::MAX;

/// The color of the column titles.
const TITLE_COLOR: TextColor = TextColor::Token(ThemeToken::Muted);

/// The color of the sort arrow.
const MARK_COLOR: TextColor = TextColor::Token(ThemeToken::Accent);

// ========================================================================
// Data Structures
// ========================================================================

/// A table of rows in columns with a cursor and a selection. It keeps the
/// columns, the sort it shows, the row state, and the layout of the last
/// frame it drew.
#[derive(Clone, Debug, Default)]
pub struct Table {
    /// The columns, left to right.
    columns: Vec<TableColumn>,
    /// Whether the pointer is over a column border, as last told the
    /// window.
    edge_hovered: bool,
    /// Where the header was drawn last, or `None` before the first frame.
    layout: Option<HeaderLayout>,
    /// The rows: cursor, selection, scroll, glide, clicks, and drags.
    list: List,
    /// Where the pointer is, or `None` when it is outside the window.
    pointer: Option<Point>,
    /// The column being resized, while its border is dragged.
    resizing: Option<Resize>,
    /// How far the columns are scrolled sideways, in logical pixels.
    scroll_x: f32,
    /// The column the rows are sorted by, as the app said.
    sort: Option<SortKey>,
}

/// One column of a [`Table`].
#[derive(Clone, Debug, PartialEq)]
pub struct TableColumn {
    /// Where the cells' text sits in the column.
    align: TextAlign,
    /// Whether it is left out of the table, as a column chooser hides one.
    hidden: bool,
    /// Whether it stretches to take the width the other columns leave.
    is_fill: bool,
    /// The narrowest a resize may make it, in logical pixels.
    min_width: f32,
    /// Whether a click on its title asks to sort by it.
    sortable: bool,
    /// The title in the header.
    title: String,
    /// The width, in logical pixels.
    width: f32,
}

/// Which column the rows are sorted by, and which way.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SortKey {
    /// The index of the column.
    pub column: usize,
    /// Which way.
    pub direction: SortDirection,
}

/// Which way rows are sorted.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SortDirection {
    /// Smallest first.
    Ascending,
    /// Largest first.
    Descending,
}

/// What became of an input offered to a [`Table`].
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum TableResponse {
    /// This row was double-clicked or Enter was pressed on it: open it.
    Activated(usize),
    /// A drag of this column's border ended: save the widths.
    ColumnResized(usize),
    /// A click or a key moved the cursor to this row.
    CursorMoved(usize),
    /// A dragged row was dropped.
    Dropped(RowDrop),
    /// The table used the event; the app should not act on it again.
    Handled,
    /// The event is not the table's.
    Ignored,
    /// A right click on the header asks for its menu: show a
    /// [`Menu`](crate::Menu) of the columns there (checked when shown,
    /// toggled with [`Table::set_column_visible`]) or of the column's
    /// actions.
    HeaderMenu(HeaderMenu),
    /// A click on a column title asks for the rows sorted so: reorder them
    /// and tell the table with [`Table::set_sort`].
    Sorted(SortKey),
}

/// Where a right click on a [`Table`]'s header landed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeaderMenu {
    /// The column under the pointer, or `None` past the last.
    pub column: Option<usize>,
    /// Where the click was, in physical pixels: where to open the menu.
    pub position: Point,
}

/// A column border being dragged.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Resize {
    /// The index of the column whose right border it is.
    column: usize,
    /// Where the press was, in physical pixels across.
    start_x: f32,
    /// The column's width as drawn at the press, in logical pixels.
    start_width: f32,
}

/// Where the header was drawn.
#[derive(Clone, Debug, PartialEq)]
struct HeaderLayout {
    /// The whole table, header and rows.
    area: Rect,
    /// Each column's right edge, in physical pixels across, scrolled.
    edges: Vec<f32>,
    /// The header row.
    header: Rect,
    /// The furthest the columns scroll sideways, in physical pixels; 0
    /// when they fit.
    max_scroll: f32,
    /// Physical pixels per logical pixel.
    scale_factor: f64,
}

// ========================================================================
// Traits
// ========================================================================

/// The rows of a [`Table`], supplied by the app.
pub trait TableSource {
    /// How many rows there are.
    fn row_count(&self) -> usize;

    /// The text of the cell of `row` in `column`. Asked only for rows on
    /// screen, each frame, and for what screen readers and a dragged row's
    /// label say.
    fn cell_text(&self, row: usize, column: usize) -> Cow<'_, str>;

    /// What the cell of `row` in `column` shows: an icon, styled text, and
    /// badges at its end. Asked only for rows on screen, each frame. By
    /// default [`TableSource::cell_text`] in the foreground color.
    fn cell_content(&self, row: usize, column: usize) -> RowContent {
        RowContent::plain(self.cell_text(row, column))
    }
}

// ========================================================================
// TableColumn Implementations
// ========================================================================

impl TableColumn {
    /// A sortable column titled `title`, `width` logical pixels wide, its
    /// text at the start of each cell.
    pub fn new(title: impl Into<String>, width: f32) -> Self {
        Self {
            align: TextAlign::Start,
            hidden: false,
            is_fill: false,
            min_width: DEFAULT_MIN_WIDTH,
            sortable: true,
            title: title.into(),
            width: width.max(DEFAULT_MIN_WIDTH),
        }
    }

    /// This column with its cells' text placed by `align` (sizes read
    /// best at the end).
    pub fn with_align(self, align: TextAlign) -> Self {
        Self { align, ..self }
    }

    /// This column stretching (`true`) to take the width the other columns
    /// leave, its own width being the least it is drawn at. Only the first
    /// such column stretches.
    pub fn with_fill(self, is_fill: bool) -> Self {
        Self { is_fill, ..self }
    }

    /// This column never resized narrower than `min_width` logical pixels.
    pub fn with_min_width(self, min_width: f32) -> Self {
        Self {
            min_width,
            width: self.width.max(min_width),
            ..self
        }
    }

    /// This column asking to sort on a click on its title (`true`, the
    /// default) or not.
    pub fn with_sortable(self, sortable: bool) -> Self {
        Self { sortable, ..self }
    }

    /// Whether a column chooser hid it.
    pub fn is_hidden(&self) -> bool {
        self.hidden
    }

    /// The title in the header.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The width, in logical pixels.
    pub fn width(&self) -> f32 {
        self.width
    }
}

// ========================================================================
// SortDirection Implementations
// ========================================================================

impl SortDirection {
    /// The other way.
    fn flipped(self) -> Self {
        match self {
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Ascending,
        }
    }
}

// ========================================================================
// Table Implementations
// ========================================================================

impl Table {
    // --------------------------------------------------------------------
    // State
    // --------------------------------------------------------------------

    /// A table of `columns`, unsorted.
    pub fn new(columns: Vec<TableColumn>) -> Self {
        Self {
            columns,
            ..Self::default()
        }
    }

    /// The columns, left to right, hidden ones included.
    pub fn columns(&self) -> &[TableColumn] {
        &self.columns
    }

    /// Shows column `index` (`true`) or hides it, as a column chooser
    /// does; whether there is such a column. Columns keep their indexes
    /// hidden or not, so a source's cells and a sort key stay the same.
    pub fn set_column_visible(&mut self, index: usize, visible: bool) -> bool {
        let Some(column) = self.columns.get_mut(index) else {
            return false;
        };
        column.hidden = !visible;
        true
    }

    /// Every column's width in logical pixels, left to right, to save.
    pub fn column_widths(&self) -> Vec<f32> {
        self.columns.iter().map(|column| column.width).collect()
    }

    /// Gives the columns the saved `widths`, left to right, each held to
    /// its column's minimum; widths past the last column are ignored and
    /// columns past the last width keep theirs.
    pub fn set_column_widths(&mut self, widths: &[f32]) {
        for (column, &width) in self.columns.iter_mut().zip(widths) {
            column.width = width.max(column.min_width);
        }
    }

    /// The column the rows are sorted by, as last set.
    pub fn sort(&self) -> Option<SortKey> {
        self.sort
    }

    /// Marks the rows as sorted by `sort` (`None`: in no column's order),
    /// after the app reordered them.
    pub fn set_sort(&mut self, sort: Option<SortKey>) {
        self.sort = sort;
    }

    /// The cursor, selection, and scroll of the rows.
    pub fn state(&self) -> &ListState {
        self.list.state()
    }

    /// The cursor, selection, and scroll of the rows, to move and change.
    /// Ask for a redraw after changing them.
    pub fn state_mut(&mut self) -> &mut ListState {
        self.list.state_mut()
    }

    /// The count and prefix typed toward a motion, to show in a status bar
    /// or a [`WhichKey`](crate::WhichKey) popup.
    pub fn keys(&self) -> &ListKeys {
        self.list.keys()
    }

    /// The count and prefix typed toward a motion, to drop them with
    /// [`ListKeys::reset`] when the table loses the keyboard.
    pub fn keys_mut(&mut self) -> &mut ListKeys {
        self.list.keys_mut()
    }

    /// The drag of a row, to light up drop targets while it runs: its
    /// [`Drag::payload`] is the row dragged.
    pub fn drag(&self) -> &Drag<usize> {
        self.list.drag()
    }

    // --------------------------------------------------------------------
    // Input
    // --------------------------------------------------------------------

    /// Reacts to `key` as a [`List`] does.
    ///
    /// Left and Right (`h`, `l`) scroll the columns sideways when they are
    /// wider than the table, a step per count, and `0` and `$` to their
    /// first and last edge, as Vim scrolls a line that does not wrap; they
    /// are [`TableResponse::Ignored`] when the columns fit.
    pub(crate) fn handle_key(&mut self, key: Key) -> TableResponse {
        let CountedKey { count, key } = match self.list.read_key(key) {
            Read::Consumed => return TableResponse::Handled,
            Read::Ignored => return TableResponse::Ignored,
            Read::Key(counted) => counted,
        };
        let steps = count.unwrap_or(1) as f32;
        let step = match key {
            ListKey::Left => -SIDEWAYS_STEP * steps,
            ListKey::LineEnd => f32::INFINITY,
            ListKey::LineStart => f32::NEG_INFINITY,
            ListKey::Right => SIDEWAYS_STEP * steps,
            ListKey::Activate | ListKey::Extend(_) | ListKey::Move(_) | ListKey::Toggle => {
                return table_response(self.list.run_key(key, count));
            }
        };
        match self
            .layout
            .as_ref()
            .is_some_and(|layout| layout.max_scroll > 0.0)
        {
            true => {
                self.scroll_sideways(step);
                TableResponse::Handled
            }
            false => TableResponse::Ignored,
        }
    }

    /// Reacts to pointer `input`: drags of column borders and clicks
    /// on titles in the header, and everything a [`List`] does with the
    /// rows; a press or the wheel over the header drops pending keys as
    /// one over the rows does. Offer it every input before acting on it.
    fn handle_pointer(&mut self, input: &Input, cx: &mut EventCx<'_>) -> TableResponse {
        let over = self.is_pointer_inside();
        if self.list.keys_mut().reset_on(input, over) {
            cx.request_redraw();
        }
        match input {
            Input::PointerButton(button)
                if button.button == PointerButton::Right
                    && button.pressed
                    && self
                        .layout
                        .as_ref()
                        .is_some_and(|layout| layout.header.contains(button.position)) =>
            {
                let column = self
                    .layout
                    .as_ref()
                    .and_then(|layout| layout.column_at(button.position.x));
                return TableResponse::HeaderMenu(HeaderMenu {
                    column,
                    position: button.position,
                });
            }
            Input::PointerButton(button) if button.button == PointerButton::Left => {
                self.pointer = Some(button.position);
                match button.pressed {
                    true => {
                        if let Some(response) = self.press_header() {
                            cx.request_redraw();
                            return response;
                        }
                    }
                    false => {
                        if let Some(resize) = self.resizing.take() {
                            return TableResponse::ColumnResized(resize.column);
                        }
                    }
                }
            }
            Input::PointerLeft => self.pointer = None,
            Input::PointerMoved(point) => {
                self.pointer = Some(*point);
                if self.drag_border_to(point.x) {
                    cx.request_redraw();
                    return TableResponse::Handled;
                }
                self.show_pointer_shape(cx, *point);
            }
            Input::Wheel(wheel) if self.is_pointer_inside() => {
                let shift = wheel.mods.contains(Mods::SHIFT);
                let scale_factor = self
                    .layout
                    .as_ref()
                    .map_or(1.0, |layout| layout.scale_factor);
                if self.scroll_sideways(sideways_wheel(wheel.delta, shift, scale_factor)) {
                    cx.request_redraw();
                }
                if shift {
                    return TableResponse::Handled;
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
            | Input::Wheel(_) => {}
        }
        table_response(self.list.handle_event(input, cx))
    }

    /// A left press at the pointer, when it is over the header: grabs a
    /// column border, or asks to sort by the column clicked. `None` when
    /// the pointer is not over the header.
    fn press_header(&mut self) -> Option<TableResponse> {
        let point = self.pointer?;
        let layout = self.layout.as_ref()?;
        if !layout.header.contains(point) {
            return None;
        }
        if let Some(column) = layout.border_at(point.x) {
            let start_width = match self.columns[column].is_fill {
                true => layout.drawn_width(column),
                false => self.columns[column].width,
            };
            self.resizing = Some(Resize {
                column,
                start_x: point.x,
                start_width,
            });
            return Some(TableResponse::Handled);
        }
        let column = layout.column_at(point.x)?;
        if !self.columns[column].sortable {
            return Some(TableResponse::Handled);
        }
        let sort = next_sort(self.sort, column);
        Some(TableResponse::Sorted(sort))
    }

    /// Moves the border being dragged to `x`, in physical pixels; whether
    /// one is being dragged.
    fn drag_border_to(&mut self, x: f32) -> bool {
        let (Some(resize), Some(layout)) = (self.resizing, &self.layout) else {
            return false;
        };
        let column = &mut self.columns[resize.column];
        let moved = (x - resize.start_x) / layout.scale_factor as f32;
        column.width = (resize.start_width + moved).max(column.min_width);
        true
    }

    /// Scrolls the columns `delta` logical pixels sideways (left when
    /// negative), no further than they reach; whether they moved.
    fn scroll_sideways(&mut self, delta: f32) -> bool {
        let Some(layout) = &self.layout else {
            return false;
        };
        let max = layout.max_scroll / layout.scale_factor as f32;
        let scroll = (self.scroll_x + delta).clamp(0.0, max);
        let moved = scroll != self.scroll_x;
        self.scroll_x = scroll;
        moved
    }

    /// Whether the pointer is over the table as last drawn.
    fn is_pointer_inside(&self) -> bool {
        match (self.pointer, &self.layout) {
            (Some(point), Some(layout)) => layout.area.contains(point),
            (None, _) | (_, None) => false,
        }
    }

    /// Shows a column resize pointer over a column border, the default
    /// pointer once it leaves one.
    fn show_pointer_shape(&mut self, cx: &mut EventCx<'_>, point: Point) {
        let over = self.layout.as_ref().is_some_and(|layout| {
            layout.header.contains(point) && layout.border_at(point.x).is_some()
        });
        if over != self.edge_hovered {
            self.edge_hovered = over;
            let icon = match over {
                true => CursorIcon::ColResize,
                false => CursorIcon::Default,
            };
            cx.set_cursor(icon);
        }
    }

    // --------------------------------------------------------------------
    // Drawing
    // --------------------------------------------------------------------

    /// The table lent `rows`, as a [`Widget`] to hand inputs and draw.
    pub fn lend<'a, S: TableSource + ?Sized>(&'a mut self, rows: &'a S) -> Lent<'a, Self, &'a S> {
        Lent::new(self, rows)
    }

    /// Draws the header and the rows of `source` that fit in `area`, in
    /// `theme`'s colors: the header on the surface color with the sorted
    /// column's arrow, rows as a [`List`] draws them, each cell's text cut
    /// to its column.
    fn draw_rows(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        source: &(impl TableSource + ?Sized),
        theme: &Theme,
    ) {
        let metrics = frame.cell_metrics();
        let scale_factor = frame.scale_factor();
        let stroke = logical(STROKE, scale_factor);
        let header = Rect::new(area.x, area.y, area.width, metrics.height.min(area.height));
        let widths = drawn_widths(&self.columns, area.width, scale_factor);
        let max_scroll = (widths.iter().sum::<f32>() - area.width).max(0.0);
        let scroll = (self.scroll_x * scale_factor as f32)
            .clamp(0.0, max_scroll)
            .round();
        self.scroll_x = scroll / scale_factor as f32;
        let edges = widths
            .iter()
            .scan(area.x - scroll, |x, width| {
                *x += width;
                Some(*x)
            })
            .collect();
        let layout = HeaderLayout {
            area,
            edges,
            header,
            max_scroll,
            scale_factor,
        };
        self.draw_header(frame, &layout, stroke, theme);
        let rows_top = header.y + header.height + stroke;
        let rows = RowsToDraw {
            area: Rect::new(
                area.x,
                rows_top,
                area.width,
                (area.y + area.height - rows_top).max(0.0),
            ),
            count: source.row_count(),
            now: Instant::now(),
        };
        let columns = &self.columns;
        let spans = layout.spans().collect::<Vec<_>>();
        let padding = (PADDING_CELLS * metrics.width).round();
        let draw_row = |frame: &mut Frame, row: usize, rect: Rect| {
            for (index, (column, (left, right))) in columns.iter().zip(&spans).enumerate() {
                if column.hidden {
                    continue;
                }
                let cell = Rect::new(
                    left + padding,
                    rect.y,
                    right - left - 2.0 * padding,
                    rect.height,
                );
                let content = source.cell_content(row, index);
                draw_content(frame, &content, cell, column.align, theme);
            }
        };
        self.list
            .draw_rows(frame, rows, theme, draw_row, |row| source.cell_text(row, 0));
        self.layout = Some(layout);
    }

    /// Draws the header row of `layout`: titles, the sort arrow, the
    /// borders between columns, and a line under it all.
    fn draw_header(&self, frame: &mut Frame, layout: &HeaderLayout, stroke: f32, theme: &Theme) {
        let header = layout.header;
        let metrics = frame.cell_metrics();
        let padding = (PADDING_CELLS * metrics.width).round();
        frame.fill_rect(header, theme.surface);
        frame.clipped(header, |frame| {
            for (index, (left, right)) in layout.spans().enumerate() {
                let column = &self.columns[index];
                if column.hidden {
                    continue;
                }
                let mark = self.sort.filter(|sort| sort.column == index).map(|sort| {
                    match sort.direction {
                        SortDirection::Ascending => ASCENDING_MARK,
                        SortDirection::Descending => DESCENDING_MARK,
                    }
                });
                let mut title_right = right - padding;
                if let Some(mark) = mark {
                    let mark_x = title_right - metrics.width;
                    let text = StyledText::plain(mark, MARK_COLOR);
                    let clip = Rect::new(mark_x, header.y, metrics.width, header.height);
                    frame.draw_text(&text, Point::new(mark_x, header.y), clip, theme);
                    title_right = mark_x;
                }
                let cell = Rect::new(
                    left + padding,
                    header.y,
                    title_right - left - padding,
                    header.height,
                );
                draw_cell(
                    frame,
                    column.title(),
                    column.align,
                    cell,
                    TITLE_COLOR,
                    theme,
                );
                frame.fill_rect_over_text(
                    Rect::new(right - stroke, header.y, stroke, header.height),
                    theme.border,
                );
            }
        });
        let line = Rect::new(header.x, header.y + header.height, header.width, stroke);
        frame.fill_rect_over_text(line, theme.border);
    }

    /// Where `row` was drawn in the last frame, in physical pixels, across
    /// the table's width, whether in sight or not; `None` before the first
    /// frame.
    pub fn row_rect(&self, row: usize) -> Option<Rect> {
        self.list.row_rect(row)
    }

    /// Where the cell of `row` in `column` was drawn in the last frame, in
    /// physical pixels, scrolled sideways as the columns were: to place an
    /// inline rename field over it. `None` before the first frame or for a
    /// column past the last.
    pub fn cell_rect(&self, row: usize, column: usize) -> Option<Rect> {
        let rect = self.list.row_rect(row)?;
        let (left, right) = self.layout.as_ref()?.spans().nth(column)?;
        Some(Rect::new(left, rect.y, right - left, rect.height))
    }

    /// The rows drawn in the last frame, wholly or in part. Empty before
    /// the first frame.
    pub fn visible_range(&self) -> Range<usize> {
        self.list.visible_range()
    }

    /// Describes the table as last drawn to screen readers as `id`: a table
    /// of `source`'s rows and columns holding a header row (column headers
    /// named by title, the sorted one with its direction) and the rows in
    /// sight (row `index` is the `index`th child of `id`, its cells its
    /// children by column), selected rows marked; the cursor's row has the
    /// focus while the table does. Nothing before the first frame.
    pub fn access(&self, frame: &mut Frame, id: FocusId, source: &(impl TableSource + ?Sized)) {
        let Some(layout) = &self.layout else {
            return;
        };
        let area = Rect::new(
            layout.header.x,
            layout.header.y,
            layout.header.width,
            self.list
                .row_rect(self.state().count())
                .map_or(layout.header.height, |end| end.y - layout.header.y),
        );
        let mut table = Node::new(Role::Table);
        table.set_row_count(source.row_count());
        table.set_column_count(self.columns.iter().filter(|column| !column.hidden).count());
        frame.enter_access(id, area, table);
        let header_id = id.child(HEADER_ROW);
        frame.enter_access(header_id, layout.header, Node::new(Role::Row));
        for (index, (left, right)) in layout.spans().enumerate() {
            let column = &self.columns[index];
            if column.hidden {
                continue;
            }
            let mut node = Node::new(Role::ColumnHeader);
            node.set_label(column.title());
            node.set_column_index(index);
            if let Some(sort) = self.sort.filter(|sort| sort.column == index) {
                node.set_sort_direction(match sort.direction {
                    SortDirection::Ascending => AccessSortDirection::Ascending,
                    SortDirection::Descending => AccessSortDirection::Descending,
                });
            }
            let rect = Rect::new(left, layout.header.y, right - left, layout.header.height);
            frame.access(header_id.child(index as u64), rect, node);
        }
        frame.leave_access();
        let state = self.state();
        for row in state.visible_rows() {
            let Some(rect) = self.list.row_rect(row) else {
                continue;
            };
            let row_id = id.child(row as u64);
            let mut node = Node::new(Role::Row);
            node.set_row_index(row);
            node.set_selected(state.is_selected(row));
            frame.enter_access(row_id, rect, node);
            for (index, (left, right)) in layout.spans().enumerate() {
                if self.columns[index].hidden {
                    continue;
                }
                let mut cell = Node::new(Role::Cell);
                cell.set_label(source.cell_text(row, index).into_owned());
                cell.set_column_index(index);
                let cell_rect = Rect::new(left, rect.y, right - left, rect.height);
                frame.access(row_id.child(index as u64), cell_rect, cell);
            }
            frame.leave_access();
        }
        frame.leave_access();
        if state.visible_rows().contains(&state.cursor()) {
            frame
                .access
                .set_inner_focus(id, id.child(state.cursor() as u64));
        }
    }
}

impl<S: TableSource + ?Sized> Widget for Lent<'_, Table, &S> {
    type Output = ();
    type Response = TableResponse;

    /// Reacts to `input`: keys as a list's, with Left and Right (`h`,
    /// `l`), `0`, and `$` scrolling columns wider than the table sideways
    /// (hand keys only while the table has the keyboard); the pointer
    /// resizes columns by their borders, asks to sort by a title clicked,
    /// and does with the rows what a list does. Asks for a redraw whenever
    /// what is drawn changed.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> TableResponse {
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
        if !matches!(response, TableResponse::Ignored) {
            cx.request_redraw();
        }
        response
    }

    /// Draws the header and the rows that fit in `area`, in `theme`'s
    /// colors.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        self.widget.draw_rows(frame, area, self.model, theme);
    }

    /// Asks for the next frame of a glide still under way, returning when
    /// it is due; `None` at rest.
    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        self.widget.list.about_to_wait(cx)
    }
}

impl Outcome for TableResponse {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

// ========================================================================
// HeaderLayout Implementations
// ========================================================================

impl HeaderLayout {
    /// Each column's left and right edge, left to right.
    fn spans(&self) -> impl Iterator<Item = (f32, f32)> + '_ {
        let lefts = iter::once(self.header.x).chain(self.edges.iter().copied());
        lefts.zip(self.edges.iter().copied())
    }

    /// The column whose right border is within grabbing distance of `x`,
    /// the nearest when two are.
    fn border_at(&self, x: f32) -> Option<usize> {
        let margin = GRAB_MARGIN * self.scale_factor as f32;
        let widths = self.spans().map(|(left, right)| right - left);
        self.edges
            .iter()
            .zip(widths)
            .enumerate()
            .filter(|(_, (edge, width))| *width > 0.0 && (x - **edge).abs() <= margin)
            .map(|(column, (edge, _))| (column, edge))
            .min_by(|(_, a), (_, b)| (x - **a).abs().total_cmp(&(x - **b).abs()))
            .map(|(column, _)| column)
    }

    /// How wide `column` was drawn, in logical pixels.
    fn drawn_width(&self, column: usize) -> f32 {
        self.spans().nth(column).map_or(0.0, |(left, right)| {
            (right - left) / self.scale_factor as f32
        })
    }

    /// The column at `x`, or `None` past the last one.
    fn column_at(&self, x: f32) -> Option<usize> {
        self.spans()
            .position(|(left, right)| x >= left && x < right)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The sort a click on `column` asks for after `current`: the other
/// direction on the column already sorted, else ascending.
fn next_sort(current: Option<SortKey>, column: usize) -> SortKey {
    let direction = match current {
        Some(sort) if sort.column == column => sort.direction.flipped(),
        Some(_) | None => SortDirection::Ascending,
    };
    SortKey { column, direction }
}

/// Each of `columns`' width as drawn in a table `width` physical pixels
/// wide on a display scaled by `scale_factor`, in whole physical pixels:
/// its own width (none when hidden), the first shown fill column taking
/// what the others leave too.
fn drawn_widths(columns: &[TableColumn], width: f32, scale_factor: f64) -> Vec<f32> {
    let mut widths = columns
        .iter()
        .map(|column| match column.hidden {
            true => 0.0,
            false => (column.width * scale_factor as f32).round(),
        })
        .collect::<Vec<_>>();
    let spare = (width - widths.iter().sum::<f32>()).max(0.0).floor();
    if let Some(fill) = columns
        .iter()
        .position(|column| column.is_fill && !column.hidden)
    {
        widths[fill] += spare;
    }
    widths
}

/// How far the wheel's `delta` scrolls the columns sideways, in logical
/// pixels on a display scaled by `scale_factor`: its sideways part, and,
/// with `shift` held, its vertical part too. Content moving right (a
/// positive delta) scrolls toward the first column.
fn sideways_wheel(delta: WheelDelta, shift: bool, scale_factor: f64) -> f32 {
    let along = |x: f32, y: f32| match shift {
        true => x + y,
        false => x,
    };
    match delta {
        WheelDelta::Lines(lines) => -along(lines.x, lines.y) * SIDEWAYS_STEP,
        WheelDelta::Pixels(pixels) => -along(pixels.x, pixels.y) / scale_factor as f32,
    }
}

/// What the rows' `response` means for the table.
fn table_response(response: ListResponse) -> TableResponse {
    match response {
        ListResponse::Activated(row) => TableResponse::Activated(row),
        ListResponse::CursorMoved(row) => TableResponse::CursorMoved(row),
        ListResponse::Dropped(dropped) => TableResponse::Dropped(dropped),
        ListResponse::Handled => TableResponse::Handled,
        ListResponse::Ignored => TableResponse::Ignored,
    }
}

/// Draws `text` in `color` as a chrome label placed in `cell` by `align`,
/// cut to it with an ellipsis.
fn draw_cell(
    frame: &mut Frame,
    text: &str,
    align: TextAlign,
    cell: Rect,
    color: TextColor,
    theme: &Theme,
) {
    if cell.width <= 0.0 {
        return;
    }
    let whole = StyledText::plain(text, color).with_ellipsis();
    let line = fit_label(frame, theme, &whole, cell.width);
    let room = cell.width - styled_label_width(frame, theme, &line);
    let x = match align {
        TextAlign::Start => cell.x,
        TextAlign::Center => cell.x + (room / 2.0).floor(),
        TextAlign::End => cell.x + room,
    };
    draw_styled_label(frame, theme, &line, Point::new(x.round(), cell.y), cell);
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::access::find_node;
    use crate::app::offscreen;
    use crate::base::color::Rgba;
    use crate::base::key::parse_key;

    /// Files as rows: a name and a size.
    struct Files;

    impl TableSource for Files {
        fn row_count(&self) -> usize {
            50
        }

        fn cell_text(&self, row: usize, column: usize) -> Cow<'_, str> {
            match column {
                0 => Cow::Owned(format!("file_{row}.txt")),
                _ => Cow::Owned(format!("{} KB", row * 10)),
            }
        }
    }

    /// A name column 200 pixels wide and a size column 100 wide, the size
    /// not sortable and at the end of its cells.
    fn table() -> Table {
        Table::new(vec![
            TableColumn::new("Name", 200.0).with_min_width(80.0),
            TableColumn::new("Size", 100.0)
                .with_align(TextAlign::End)
                .with_sortable(false),
        ])
    }

    /// The header of [`table`] drawn 20 pixels high at the top left, on a
    /// display scaled by `scale_factor`.
    fn header(scale_factor: f64) -> HeaderLayout {
        let scale = scale_factor as f32;
        HeaderLayout {
            area: Rect::new(0.0, 0.0, 600.0, 400.0),
            edges: vec![200.0 * scale, 300.0 * scale],
            header: Rect::new(0.0, 0.0, 600.0, 20.0),
            max_scroll: 0.0,
            scale_factor,
        }
    }

    /// Columns 150, 100 (filling), and 150 logical pixels wide.
    fn with_fill() -> Vec<TableColumn> {
        vec![
            TableColumn::new("Name", 150.0),
            TableColumn::new("Path", 100.0).with_fill(true),
            TableColumn::new("Size", 150.0),
        ]
    }

    /// `table` drawn offscreen in `width` by 200 physical pixels.
    fn draw_in(table: &mut Table, width: u32) {
        let mut frame = offscreen::plain_frame(width, 200);
        let area = Rect::new(0.0, 0.0, width as f32, 200.0);
        table.lend(&Files).draw(&mut frame, area, &Theme::dark());
    }

    /// The right edges `table` last drew its columns at.
    fn edges(table: &Table) -> Vec<f32> {
        table.layout.as_ref().expect("drawn").edges.clone()
    }

    #[test]
    fn test_a_hidden_column_takes_no_width_its_border_none_and_its_fill_passes_on() {
        let mut columns = with_fill();
        columns[1].hidden = true;
        assert_eq!(
            drawn_widths(&columns, 600.0, 1.0),
            [150.0, 0.0, 150.0],
            "no fill left"
        );
        columns[2].is_fill = true;
        assert_eq!(drawn_widths(&columns, 600.0, 1.0), [150.0, 0.0, 450.0]);
        let mut table = Table::new(with_fill());
        assert!(table.set_column_visible(1, false));
        assert!(!table.set_column_visible(9, false), "no such column");
        draw_in(&mut table, 600);
        let layout = table.layout.as_ref().expect("drawn");
        assert_eq!(
            layout.border_at(150.0),
            Some(0),
            "the shown column's border, not the hidden one's"
        );
        assert_eq!(layout.column_at(151.0), Some(2));
    }

    #[test]
    fn test_a_right_click_on_the_header_asks_for_its_menu() {
        let mut window = crate::testing::TestWindow::new(600, 200);
        let mut table = table();
        draw_in(&mut table, 600);
        let at = Point::new(250.0, 5.0);
        let click = Input::PointerButton(crate::input::event::ButtonInput::press(
            PointerButton::Right,
            at,
        ));
        let response = window.send(&click, |input, cx| table.lend(&Files).handle(input, cx));
        assert_eq!(
            response,
            TableResponse::HeaderMenu(HeaderMenu {
                column: Some(1),
                position: at
            })
        );
        let below = Input::PointerButton(crate::input::event::ButtonInput::press(
            PointerButton::Right,
            Point::new(250.0, 100.0),
        ));
        let response = window.send(&below, |input, cx| table.lend(&Files).handle(input, cx));
        assert!(
            !matches!(response, TableResponse::HeaderMenu(_)),
            "the rows' own"
        );
    }

    #[test]
    fn test_a_fill_column_takes_the_spare_width_and_none_when_there_is_none() {
        let columns = with_fill();
        assert_eq!(drawn_widths(&columns, 600.0, 1.0), [150.0, 300.0, 150.0]);
        assert_eq!(
            drawn_widths(&columns, 600.0, 2.0),
            [300.0, 200.0, 300.0],
            "doubled, nothing spare"
        );
        assert_eq!(drawn_widths(&columns, 300.0, 1.0), [150.0, 100.0, 150.0]);
    }

    #[test]
    fn test_columns_wider_than_the_table_scroll_sideways_with_the_header_and_stop_at_the_end() {
        let mut table = Table::new(with_fill());
        draw_in(&mut table, 300);
        let press = |table: &mut Table, spec| table.handle_key(parse_key(spec).expect("a key"));
        assert_eq!(press(&mut table, "l"), TableResponse::Handled);
        draw_in(&mut table, 300);
        assert_eq!(edges(&table), [102.0, 202.0, 352.0], "scrolled one step");
        for _ in 0..10 {
            press(&mut table, "<right>");
        }
        draw_in(&mut table, 300);
        assert_eq!(
            edges(&table),
            [50.0, 150.0, 300.0],
            "the last column ends at the edge"
        );
        assert_eq!(press(&mut table, "l"), TableResponse::Handled, "at the end");
        draw_in(&mut table, 1000);
        assert_eq!(edges(&table)[0], 150.0, "a wider table needs no scroll");
        assert_eq!(
            press(&mut table, "h"),
            TableResponse::Ignored,
            "the columns fit"
        );
    }

    #[test]
    fn test_a_count_steps_sideways_that_often_and_dollar_and_zero_go_to_the_edges() {
        let mut table = Table::new(with_fill());
        draw_in(&mut table, 300);
        for spec in ["2", "l"] {
            table.handle_key(parse_key(spec).expect("a key"));
        }
        draw_in(&mut table, 300);
        assert_eq!(edges(&table), [54.0, 154.0, 304.0], "two steps");
        table.handle_key(parse_key("$").expect("a key"));
        draw_in(&mut table, 300);
        assert_eq!(edges(&table), [50.0, 150.0, 300.0], "the last edge");
        table.handle_key(parse_key("0").expect("a key"));
        draw_in(&mut table, 300);
        assert_eq!(edges(&table), [150.0, 250.0, 400.0], "the first edge");
    }

    #[test]
    fn test_shift_with_the_wheel_or_a_sideways_swipe_scrolls_toward_later_columns() {
        let down = WheelDelta::Lines(Point::new(0.0, -1.0));
        assert_eq!(sideways_wheel(down, true, 1.0), SIDEWAYS_STEP);
        assert_eq!(sideways_wheel(down, false, 1.0), 0.0, "rows scroll instead");
        let swipe = WheelDelta::Pixels(Point::new(-30.0, 0.0));
        assert_eq!(sideways_wheel(swipe, false, 2.0), 15.0, "logical pixels");
    }

    #[test]
    fn test_dragging_a_fill_column_border_starts_from_its_drawn_width() {
        let mut table = Table::new(with_fill());
        draw_in(&mut table, 600);
        table.pointer = Some(Point::new(450.0, 5.0));
        assert_eq!(table.press_header(), Some(TableResponse::Handled));
        table.drag_border_to(470.0);
        assert_eq!(table.column_widths(), [150.0, 320.0, 150.0]);
        draw_in(&mut table, 600);
        assert_eq!(
            edges(&table),
            [150.0, 470.0, 620.0],
            "now wider than the table"
        );
    }

    #[test]
    fn test_a_title_click_sorts_ascending_and_again_flips_the_direction() {
        let ascending = next_sort(None, 1);
        assert_eq!(ascending.direction, SortDirection::Ascending);
        assert_eq!(
            next_sort(Some(ascending), 1).direction,
            SortDirection::Descending
        );
        let other = next_sort(Some(next_sort(Some(ascending), 1)), 0);
        assert_eq!(
            (other.column, other.direction),
            (0, SortDirection::Ascending)
        );
    }

    #[test]
    fn test_a_border_is_grabbed_near_a_column_edge_and_a_title_elsewhere() {
        let layout = header(2.0);
        assert_eq!(layout.border_at(405.0), Some(0), "within 3 logical pixels");
        assert_eq!(layout.border_at(410.0), None);
        assert_eq!(layout.column_at(410.0), Some(1));
        assert_eq!(layout.column_at(700.0), None, "past the last column");
    }

    #[test]
    fn test_a_border_drag_resizes_in_logical_pixels_never_below_the_minimum() {
        let mut table = table();
        table.layout = Some(header(2.0));
        table.pointer = Some(Point::new(401.0, 10.0));
        assert_eq!(table.press_header(), Some(TableResponse::Handled));
        assert!(table.drag_border_to(461.0));
        assert_eq!(
            table.column_widths(),
            [230.0, 100.0],
            "60 physical is 30 logical"
        );
        table.drag_border_to(0.0);
        assert_eq!(table.column_widths()[0], 80.0, "held to the minimum");
    }

    #[test]
    fn test_a_click_on_a_title_asks_to_sort_unless_the_column_is_not_sortable() {
        let mut table = table();
        table.layout = Some(header(1.0));
        table.pointer = Some(Point::new(50.0, 10.0));
        let sort = SortKey {
            column: 0,
            direction: SortDirection::Ascending,
        };
        assert_eq!(table.press_header(), Some(TableResponse::Sorted(sort)));
        assert_eq!(table.sort(), None, "the app sorts and says so");
        table.pointer = Some(Point::new(250.0, 10.0));
        assert_eq!(table.press_header(), Some(TableResponse::Handled));
        table.pointer = Some(Point::new(250.0, 40.0));
        assert_eq!(table.press_header(), None, "below the header, the rows'");
    }

    #[test]
    fn test_keys_move_the_rows_cursor_and_enter_opens_its_row() {
        let mut table = table();
        table.state_mut().set_count(Files.row_count());
        let mut press = |spec| table.handle_key(parse_key(spec).expect("a key"));
        assert_eq!(press("C-n"), TableResponse::CursorMoved(1));
        assert_eq!(press("<cr>"), TableResponse::Activated(1));
        assert_eq!(press("q"), TableResponse::Ignored);
    }

    #[test]
    fn test_saved_widths_are_held_to_each_minimum() {
        let mut table = table();
        table.set_column_widths(&[10.0, 150.0, 99.0]);
        assert_eq!(table.column_widths(), [80.0, 150.0]);
    }

    #[test]
    fn test_the_header_marks_the_sorted_column_and_sizes_sit_at_the_cell_end() {
        let theme = Theme::dark();
        let mut table = table();
        table.set_sort(Some(SortKey {
            column: 0,
            direction: SortDirection::Descending,
        }));
        let area = Rect::new(0.0, 0.0, 300.0, 200.0);
        let Some(snapshot) = offscreen::render(300, 200, |frame| {
            frame.clear(Rgba::from(theme.background));
            table.lend(&Files).draw(frame, area, &theme);
        }) else {
            return;
        };
        let height = snapshot.metrics().height;
        let cell = snapshot.metrics().width;
        snapshot.assert_pixel(100.0, 1.0, theme.surface, "the header");
        let arrow = Rect::new(200.0 - 2.0 * cell, 0.0, cell * 1.5, height);
        assert!(snapshot.contains_color(arrow, theme.accent), "the arrow");
        // Row 3 reads "30 KB": in the size column's right half only.
        let row = height + 1.0 + 3.0 * height;
        let right = Rect::new(250.0, row, 50.0, height);
        let left = Rect::new(201.0, row, 40.0, height);
        assert!(
            snapshot.contains_color(right, theme.foreground),
            "end aligned"
        );
        assert!(
            !snapshot.contains_color(left, theme.foreground),
            "nothing at the start"
        );
    }

    #[test]
    fn test_the_table_describes_its_header_rows_and_cells() {
        let id = FocusId::new("files");
        let mut frame = offscreen::plain_frame(400, 300);
        let mut table = table();
        table.set_sort(Some(SortKey {
            column: 0,
            direction: SortDirection::Ascending,
        }));
        table.state_mut().set_count(Files.row_count());
        table.state_mut().toggle_selected(2);
        table.lend(&Files).draw(
            &mut frame,
            Rect::new(0.0, 0.0, 400.0, 300.0),
            &Theme::dark(),
        );
        table.access(&mut frame, id, &Files);
        let update = frame.access.tree_update("t");
        let node = |id| find_node(&update, id).expect("described");
        assert_eq!(node(id).role(), Role::Table);
        assert_eq!(
            (node(id).row_count(), node(id).column_count()),
            (Some(50), Some(2))
        );
        let name = node(id.child(HEADER_ROW).child(0));
        assert_eq!(
            (name.role(), name.label(), name.sort_direction()),
            (
                Role::ColumnHeader,
                Some("Name"),
                Some(AccessSortDirection::Ascending)
            )
        );
        let row = node(id.child(2));
        assert_eq!((row.role(), row.is_selected()), (Role::Row, Some(true)));
        let size = node(id.child(2).child(1));
        assert_eq!((size.role(), size.label()), (Role::Cell, Some("20 KB")));
    }

    #[test]
    fn test_a_cell_rect_spans_its_column_on_its_row_scrolled_sideways() {
        let mut table = table();
        draw_in(&mut table, 250);
        let row = table.row_rect(2).expect("drawn");
        let cell = table.cell_rect(2, 1).expect("drawn");
        assert_eq!((cell.x, cell.width), (200.0, 100.0));
        assert_eq!((cell.y, cell.height), (row.y, row.height));
        table.handle_key(parse_key("$").expect("a key"));
        draw_in(&mut table, 250);
        assert_eq!(table.cell_rect(2, 1).map(|cell| cell.x), Some(150.0));
        assert_eq!(table.cell_rect(2, 2), None, "no third column");
    }
}
