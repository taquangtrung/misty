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
//!
//! A spreadsheet viewer asks for more, each off until asked for, so a
//! table that asks for none draws as it always did. The source styles a
//! cell ([`TableSource::cell_style`]: a fill across the whole cell,
//! borders on its edges, its own alignment), merges a range of cells into
//! one ([`TableSource::merged_range`]), and draws over the cells, a chart
//! anchored to a range ([`TableSource::draw_over`]). The table freezes
//! leading rows and columns, which stay in place while the others scroll
//! under them ([`Table::with_frozen`]), zooms the cells, their text, rows,
//! and widths ([`Table::with_zoom`]), and keeps a cell cursor
//! ([`Table::set_cell_cursor`]): an outlined cell on the cursor's row that
//! Left and Right (`h`, `l`), `0`, `$`, and a click move between columns,
//! the columns scrolling sideways to keep it in sight.

use std::borrow::Cow;
use std::collections::HashSet;
use std::iter;
use std::mem;
use std::ops::Range;
use std::time::Instant;

use accesskit::{Node, Role, SortDirection as AccessSortDirection};

use super::super::app::context::EventCx;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::{Lent, Outcome, Widget};
use super::super::base::clock;
use super::super::base::cursor::CursorIcon;
use super::super::base::geometry::{Point, Rect};
#[cfg(test)]
use super::super::base::key::KeyScheme;
use super::super::base::key::{Key, Mods};
use super::super::base::list::ListState;
use super::super::base::styled::{StyledText, TextColor};
use super::super::base::theme::{Theme, ThemeToken};
use super::super::input::event::{Input, PointerButton, WheelDelta};
use super::super::render::icon::{logical, outline};
use super::super::render::paragraph::TextAlign;
use super::drag::Drag;
use super::label::{draw_styled_label, fit_label, styled_label_width};
use super::list_keys::{CountedKey, ListKey, ListKeys, Read};
use super::list_view::{List, ListResponse, RowDrop, RowsToDraw};
use super::row_content::{RowContent, RowFont, draw_content_in};

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

/// The thickness of the cell cursor's outline, in logical pixels.
const CELL_CURSOR: f32 = 2.0;

/// The least a table zooms its cells.
const MIN_ZOOM: f32 = 0.25;

/// The most a table zooms its cells.
const MAX_ZOOM: f32 = 4.0;

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

/// The parts of a table with frozen rows and columns, the part that
/// scrolls both ways first.
const REGIONS: [Region; 4] = [
    Region {
        column_frozen: false,
        row_frozen: false,
    },
    Region {
        column_frozen: true,
        row_frozen: false,
    },
    Region {
        column_frozen: false,
        row_frozen: true,
    },
    Region {
        column_frozen: true,
        row_frozen: true,
    },
];

// ========================================================================
// Data Structures
// ========================================================================

/// A table of rows in columns with a cursor and a selection. It keeps the
/// columns, the sort it shows, the row state, and the layout of the last
/// frame it drew.
#[derive(Clone, Debug, Default)]
pub struct Table {
    /// The column of the cell cursor on the cursor's row, or `None` for
    /// no cell cursor.
    cell_cursor: Option<usize>,
    /// The columns, left to right.
    columns: Vec<TableColumn>,
    /// Whether the pointer is over a column border, as last told the
    /// window.
    edge_hovered: bool,
    /// The font the cells' text is set in.
    font: RowFont,
    /// How many leading columns stay in place while the others scroll
    /// sideways under them.
    frozen_columns: usize,
    /// How many leading rows stay in place while the others scroll under
    /// them.
    frozen_rows: usize,
    /// Where the header was drawn last, or `None` before the first frame.
    layout: Option<HeaderLayout>,
    /// The rows: cursor, selection, scroll, glide, clicks, and drags.
    list: List,
    /// Where the pointer is, or `None` when it is outside the window.
    pointer: Option<Point>,
    /// The column being resized, while its border is dragged.
    resizing: Option<Resize>,
    /// The row and column of the cell cursor the columns last scrolled
    /// sideways to show, so they scroll only when it moves.
    revealed: Option<(usize, usize)>,
    /// How far the columns are scrolled sideways, in logical pixels.
    scroll_x: f32,
    /// The column the rows are sorted by, as the app said.
    sort: Option<SortKey>,
    /// How many times their size the cells are drawn, or `None` for once.
    zoom: Option<f32>,
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

/// How a [`Table`] draws one cell beyond its content, as
/// [`TableSource::cell_style`] says: a fill across the whole cell, lines
/// along its edges, and where its text sits. The default draws nothing
/// more than the content, placed as its column says.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TableCellStyle {
    /// Where the text sits in the cell, or `None` for its column's
    /// alignment.
    pub align: Option<TextAlign>,
    /// The lines along the cell's edges.
    pub borders: CellBorders,
    /// What fills the whole cell under its text, or `None` for nothing.
    pub fill: Option<TextColor>,
    /// Whether its text runs on over the empty cells beside it in its row,
    /// as a spreadsheet's does: right of text at the start, left of text
    /// at the end, both ways from centered text. A cell is empty when its
    /// [`TableSource::cell_text`] is, and it is not part of a merged range.
    pub spill: bool,
}

/// The lines along the edges of a [`Table`]'s cell, each `None` for no
/// line. A line runs centered on the grid line, so the right edge of one
/// cell and the left edge of the next draw as one line.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CellBorders {
    /// The bottom edge.
    pub bottom: Option<CellBorder>,
    /// The left edge.
    pub left: Option<CellBorder>,
    /// The right edge.
    pub right: Option<CellBorder>,
    /// The top edge.
    pub top: Option<CellBorder>,
}

/// One line along a [`Table`] cell's edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellBorder {
    /// Its color.
    pub color: TextColor,
    /// Its thickness, in logical pixels (at least a physical pixel).
    pub width: f32,
}

/// A range of a [`Table`]'s cells, merged into one by
/// [`TableSource::merged_range`]: the rows and columns it spans, by index,
/// hidden columns included.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CellRange {
    /// The columns it spans.
    pub columns: Range<usize>,
    /// The rows it spans.
    pub rows: Range<usize>,
}

/// Where a [`Table`] drew its cells this frame, handed to
/// [`TableSource::draw_over`] to place what is drawn over them.
#[derive(Clone, Copy, Debug)]
pub struct TableCells<'a> {
    /// The table, laid out.
    table: &'a Table,
}

/// A cell drawn after the rows: a merged range, or a cell with borders,
/// kept in the frozen or scrolling part of the table it belongs to.
#[derive(Clone, Debug)]
struct LateCell {
    /// What the cell shows, for a merged range; `None` for a cell whose
    /// rows drew its content.
    content: Option<(RowContent, TextAlign)>,
    /// The cell, or the whole merged range, in physical pixels.
    rect: Rect,
    /// The part of the table it is drawn in.
    region: Region,
    /// Its fill and borders.
    style: TableCellStyle,
}

/// What draws a table's rows, a cell at a time, and keeps what is drawn
/// after them.
struct RowPainter<'a, S: ?Sized> {
    /// The frozen columns' part of the rows, and the part that scrolls.
    clips: (Rect, Rect),
    /// The columns.
    columns: &'a [TableColumn],
    /// The font the cells' text is set in.
    font: RowFont,
    /// How many leading columns stay in place.
    frozen: usize,
    /// The merged ranges and bordered cells, drawn after the rows.
    late: Vec<LateCell>,
    /// The height of a line of the cells' text.
    line_height: f32,
    /// The room at each side of a cell's text.
    padding: f32,
    /// How many leading rows stay in place.
    pinned: usize,
    /// The height of a row.
    row_height: f32,
    /// The room above a row's text.
    row_padding: f32,
    /// The merged ranges already kept, by their top left cell and
    /// whether they are on the frozen rows.
    seen: HashSet<(usize, usize, bool)>,
    /// The cells.
    source: &'a S,
    /// Each column's left and right edge.
    spans: Vec<(f32, f32)>,
    /// The style of each cell of the part of a row being drawn, `None`
    /// for one not drawn with the row; kept to reuse its room.
    styles: Vec<Option<TableCellStyle>>,
    /// The theme.
    theme: &'a Theme,
}

/// Which part of a table with frozen rows and columns a cell is drawn in.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct Region {
    /// Whether it is in a frozen column.
    column_frozen: bool,
    /// Whether it is on a frozen row.
    row_frozen: bool,
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
    /// A click or a key moved the cell cursor to this column
    /// ([`Table::set_cell_cursor`]).
    CellCursorMoved(usize),
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
    /// How many leading columns stay in place.
    frozen: usize,
    /// Where the columns that scroll start, in physical pixels across:
    /// the right edge of the frozen ones, the table's left without any.
    frozen_right: f32,
    /// The header row.
    header: Rect,
    /// The furthest the columns scroll sideways, in physical pixels; 0
    /// when they fit.
    max_scroll: f32,
    /// Physical pixels per logical pixel.
    scale_factor: f64,
    /// Physical pixels per logical pixel of a column's width: the scale
    /// factor times the zoom.
    width_scale: f64,
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

    /// How the cell of `row` in `column` is drawn beyond its content: its
    /// fill, borders, and alignment. Asked only for cells on screen, each
    /// frame. By default nothing more than the content, placed as its
    /// column says.
    fn cell_style(&self, _row: usize, _column: usize) -> TableCellStyle {
        TableCellStyle::default()
    }

    /// The range of merged cells the cell of `row` in `column` is part of,
    /// or `None` when it stands alone (the default). A merged range is
    /// drawn as one cell with the content and style of its top left cell;
    /// every cell in it answers with the same range. Asked only for cells
    /// on screen, each frame.
    fn merged_range(&self, _row: usize, _column: usize) -> Option<CellRange> {
        None
    }

    /// Draws over the cells, after them, in the part of the table that
    /// scrolls both ways (below the header and the frozen rows, right of
    /// the frozen columns): a chart anchored to a range of cells, placed by
    /// [`TableCells::range_rect`]. By default nothing.
    fn draw_over(&self, _frame: &mut Frame, _cells: &TableCells<'_>, _theme: &Theme) {}
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

    /// This column left out of the table (`true`), as a column chooser
    /// hides one, or shown. [`Table::set_column_visible`] does the same to
    /// a table's column; an app keeping its own columns for the table
    /// element of `misty::view` hides one with this.
    pub fn with_hidden(self, hidden: bool) -> Self {
        Self { hidden, ..self }
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
// TableCellStyle Implementations
// ========================================================================

impl TableCellStyle {
    /// This style filling the whole cell with `fill` under its text.
    pub fn with_fill(self, fill: impl Into<TextColor>) -> Self {
        Self {
            fill: Some(fill.into()),
            ..self
        }
    }

    /// This style placing the cell's text by `align`, in place of its
    /// column's alignment.
    pub fn with_align(self, align: TextAlign) -> Self {
        Self {
            align: Some(align),
            ..self
        }
    }

    /// This style drawing `borders` along the cell's edges.
    pub fn with_borders(self, borders: CellBorders) -> Self {
        Self { borders, ..self }
    }

    /// This style letting the cell's text run on over the empty cells
    /// beside it, as [`TableCellStyle::spill`] says.
    pub fn with_spill(self, spill: bool) -> Self {
        Self { spill, ..self }
    }

    /// Whether it draws nothing beyond the content.
    fn is_plain(&self) -> bool {
        self.fill.is_none() && self.borders == CellBorders::default()
    }
}

// ========================================================================
// CellBorders Implementations
// ========================================================================

impl CellBorders {
    /// `border` along all four edges.
    pub fn all(border: CellBorder) -> Self {
        Self {
            bottom: Some(border),
            left: Some(border),
            right: Some(border),
            top: Some(border),
        }
    }

    /// The lines to fill for these borders around `rect`, each centered
    /// on its edge, on a display scaled by `scale_factor`, with their
    /// colors.
    fn edges(&self, rect: Rect, scale_factor: f64) -> impl Iterator<Item = (Rect, TextColor)> {
        let thickness = |border: &CellBorder| logical(border.width, scale_factor);
        let across = |x: f32, width: f32| ((x - width / 2.0).round(), width);
        let right = rect.x + rect.width;
        let bottom = rect.y + rect.height;
        let lines = [
            self.top.map(|border| {
                let (y, height) = across(rect.y, thickness(&border));
                (Rect::new(rect.x, y, rect.width, height), border.color)
            }),
            self.bottom.map(|border| {
                let (y, height) = across(bottom, thickness(&border));
                (Rect::new(rect.x, y, rect.width, height), border.color)
            }),
            self.left.map(|border| {
                let (x, width) = across(rect.x, thickness(&border));
                (Rect::new(x, rect.y, width, rect.height), border.color)
            }),
            self.right.map(|border| {
                let (x, width) = across(right, thickness(&border));
                (Rect::new(x, rect.y, width, rect.height), border.color)
            }),
        ];
        lines.into_iter().flatten()
    }
}

// ========================================================================
// CellBorder Implementations
// ========================================================================

impl CellBorder {
    /// A line in `color`, `width` logical pixels thick.
    pub fn new(color: impl Into<TextColor>, width: f32) -> Self {
        Self {
            color: color.into(),
            width,
        }
    }
}

// ========================================================================
// CellRange Implementations
// ========================================================================

impl CellRange {
    /// The cells of `rows` in `columns`.
    pub fn new(rows: Range<usize>, columns: Range<usize>) -> Self {
        Self { columns, rows }
    }
}

// ========================================================================
// TableCells Implementations
// ========================================================================

impl TableCells<'_> {
    /// Where the cell of `row` in `column` is drawn, in physical pixels,
    /// whether in sight or not, scrolled as the rows and columns are; as
    /// [`Table::cell_rect`] says.
    pub fn cell_rect(&self, row: usize, column: usize) -> Option<Rect> {
        self.table.cell_rect(row, column)
    }

    /// Where `range` is drawn, from the top left of its first cell to the
    /// bottom right of its last, in physical pixels; `None` for an empty
    /// range or one past the last column.
    pub fn range_rect(&self, range: &CellRange) -> Option<Rect> {
        let last_row = range.rows.end.checked_sub(1)?;
        let last_column = range.columns.end.checked_sub(1)?;
        let first = self.cell_rect(range.rows.start, range.columns.start)?;
        let last = self.cell_rect(last_row, last_column)?;
        Some(Rect::new(
            first.x,
            first.y,
            last.x + last.width - first.x,
            last.y + last.height - first.y,
        ))
    }
}

// ========================================================================
// Table Implementations
// ========================================================================

impl Table {
    /// This widget taking the Vim and Emacs keys, for misty's tests of
    /// them.
    #[cfg(test)]
    pub(crate) fn with_vim_and_emacs(mut self) -> Self {
        self.list.set_key_scheme(KeyScheme::VIM_AND_EMACS);
        self
    }

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

    /// This table with its cells' text laid out in the proportional font,
    /// each character as wide as its shape, as a desktop file manager sets
    /// its rows; the header follows the theme's label font either way.
    pub fn with_proportional_font(self) -> Self {
        Self {
            font: RowFont::Proportional,
            ..self
        }
    }

    /// This table keeping its first `rows` rows and `columns` columns in
    /// place while the others scroll under them, as a spreadsheet freezes
    /// its panes; `0, 0` (the default) for none.
    pub fn with_frozen(mut self, rows: usize, columns: usize) -> Self {
        self.set_frozen(rows, columns);
        self
    }

    /// Keeps the first `rows` rows and `columns` columns (counted with
    /// hidden ones) in place while the others scroll under them; `0, 0`
    /// for none. Rows past the height of the table are not kept.
    pub fn set_frozen(&mut self, rows: usize, columns: usize) {
        self.frozen_rows = rows;
        self.frozen_columns = columns;
    }

    /// How many leading rows and columns stay in place.
    pub fn frozen(&self) -> (usize, usize) {
        (self.frozen_rows, self.frozen_columns)
    }

    /// This table drawing its cells `zoom` times their size: their text,
    /// the rows' height, and the columns' widths; the header keeps the
    /// label font's size. 1 (the default) for their own size.
    pub fn with_zoom(mut self, zoom: f32) -> Self {
        self.set_zoom(zoom);
        self
    }

    /// Draws the cells `zoom` times their size, held to 0.25 to 4; a zoom
    /// that is not a number is taken as 1. The widths the columns are
    /// given, saved, and dragged to stay their own size, unzoomed.
    pub fn set_zoom(&mut self, zoom: f32) {
        let zoom = match zoom.is_finite() {
            true => zoom.clamp(MIN_ZOOM, MAX_ZOOM),
            false => 1.0,
        };
        self.zoom = (zoom != 1.0).then_some(zoom);
    }

    /// How many times their size the cells are drawn.
    pub fn zoom(&self) -> f32 {
        self.zoom.unwrap_or(1.0)
    }

    /// Puts the cell cursor on `column` of the cursor's row, or takes it
    /// away with `None` (the default). While there is one, it is outlined
    /// in the accent color, Left and Right (`h`, `l`), `0`, and `$` move it
    /// between the shown columns ([`TableResponse::CellCursorMoved`]) in
    /// place of scrolling sideways, a click on a cell moves it there too,
    /// and the columns scroll sideways to show it whenever it or the row
    /// cursor moves.
    pub fn set_cell_cursor(&mut self, column: Option<usize>) {
        self.cell_cursor = column;
    }

    /// The column of the cell cursor, or `None` without one.
    pub fn cell_cursor(&self) -> Option<usize> {
        self.cell_cursor
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

    /// Takes the app's `columns` in place of those held, where they
    /// changed from `last`, the app's columns as last taken: every column
    /// as the app now says, but for the width the user dragged a column
    /// to, kept unless the app changed that column's width itself.
    pub(crate) fn take_columns(&mut self, columns: &[TableColumn], last: &[TableColumn]) {
        let held = mem::take(&mut self.columns);
        self.columns = columns
            .iter()
            .enumerate()
            .map(|(index, column)| {
                let was = last.get(index).map(|last| last.width);
                match (held.get(index), was == Some(column.width)) {
                    (Some(held), true) => TableColumn {
                        width: held.width,
                        ..column.clone()
                    },
                    (None, _) | (_, false) => column.clone(),
                }
            })
            .collect();
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
    /// are [`TableResponse::Ignored`] when the columns fit. With a cell
    /// cursor they move it between the shown columns instead.
    pub(crate) fn handle_key(&mut self, key: Key) -> TableResponse {
        let CountedKey { count, key } = match self.list.read_key(key) {
            Read::Consumed => return TableResponse::Handled,
            Read::Ignored => return TableResponse::Ignored,
            Read::Key(counted) => counted,
        };
        if let Some(column) = self.cell_cursor
            && matches!(
                key,
                ListKey::Left | ListKey::LineEnd | ListKey::LineStart | ListKey::Right
            )
        {
            let moved = self.step_cell_cursor(column, key, count.unwrap_or(1));
            self.cell_cursor = Some(moved);
            return match moved == column {
                true => TableResponse::Handled,
                false => TableResponse::CellCursorMoved(moved),
            };
        }
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

    /// The column the cell cursor on `column` goes to for `key`, a Left,
    /// Right, line start, or line end, `count` times: the shown column
    /// that many before or after, or the first or last shown one, stopping
    /// at either end.
    fn step_cell_cursor(&self, column: usize, key: ListKey, count: usize) -> usize {
        let shown = || {
            self.columns
                .iter()
                .enumerate()
                .filter(|(_, column)| !column.hidden)
                .map(|(index, _)| index)
        };
        let found = match key {
            ListKey::LineEnd => shown().next_back(),
            ListKey::LineStart => shown().next(),
            ListKey::Left => shown()
                .rev()
                .filter(|index| *index < column)
                .take(count)
                .last(),
            ListKey::Right => shown().filter(|index| *index > column).take(count).last(),
            ListKey::Activate | ListKey::Extend(_) | ListKey::Move(_) | ListKey::Toggle => None,
        };
        found.unwrap_or(column)
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
                        self.press_cell(button.position);
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

    /// Moves the cell cursor, when there is one, to the column of a press
    /// at `point` on the rows.
    fn press_cell(&mut self, point: Point) {
        let (Some(_), Some(layout)) = (self.cell_cursor, &self.layout) else {
            return;
        };
        let below_header = point.y >= layout.header.y + layout.header.height;
        if layout.area.contains(point)
            && below_header
            && let Some(column) = layout.column_at(point.x)
        {
            self.cell_cursor = Some(column);
        }
    }

    /// Moves the border being dragged to `x`, in physical pixels; whether
    /// one is being dragged.
    fn drag_border_to(&mut self, x: f32) -> bool {
        let (Some(resize), Some(layout)) = (self.resizing, &self.layout) else {
            return false;
        };
        let column = &mut self.columns[resize.column];
        let moved = (x - resize.start_x) / layout.width_scale as f32;
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
    /// to its column; then merged ranges and borders, what the source
    /// draws over the cells, and the cell cursor.
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
        let row_padding = (f32::from(theme.shape.row_padding) * scale_factor as f32).round();
        let header_height = metrics.height + 2.0 * row_padding;
        let header = Rect::new(area.x, area.y, area.width, header_height.min(area.height));
        let width_scale = scale_factor * f64::from(self.zoom());
        let widths = drawn_widths(&self.columns, area.width, width_scale);
        let frozen = self.frozen_columns.min(widths.len());
        let frozen_width = widths[..frozen].iter().sum::<f32>().min(area.width);
        let max_scroll = (widths.iter().sum::<f32>() - area.width).max(0.0);
        self.reveal_cell_cursor(&widths, frozen, area.width - frozen_width, scale_factor);
        let scroll = (self.scroll_x * scale_factor as f32)
            .clamp(0.0, max_scroll)
            .round();
        self.scroll_x = scroll / scale_factor as f32;
        let edges = widths
            .iter()
            .enumerate()
            .scan(0.0, |sum, (index, width)| {
                *sum += width;
                let start = match index < frozen {
                    true => area.x,
                    false => area.x - scroll,
                };
                Some(start + *sum)
            })
            .collect();
        let layout = HeaderLayout {
            area,
            edges,
            frozen,
            frozen_right: area.x + frozen_width,
            header,
            max_scroll,
            scale_factor,
            width_scale,
        };
        self.draw_header(frame, &layout, stroke, row_padding, theme);
        let rows_top = header.y + header.height + stroke;
        let rows = Rect::new(
            area.x,
            rows_top,
            area.width,
            (area.y + area.height - rows_top).max(0.0),
        );
        match self.zoom {
            None => self.draw_cells(frame, layout, rows, source, theme),
            Some(zoom) => frame.with_text_scale(zoom, |frame| {
                self.draw_cells(frame, layout, rows, source, theme);
            }),
        }
    }

    /// Draws the rows of `source` in `rows`, under the header laid out as
    /// `layout`, in the cells' size: the rows as a [`List`] draws them,
    /// each cell's fill and content; then the merged ranges and borders,
    /// what the source draws over the cells, and the cell cursor. Keeps
    /// `layout` as the last frame's.
    fn draw_cells(
        &mut self,
        frame: &mut Frame,
        layout: HeaderLayout,
        rows: Rect,
        source: &(impl TableSource + ?Sized),
        theme: &Theme,
    ) {
        let metrics = frame.cell_metrics();
        let scale_factor = frame.scale_factor();
        let row_padding =
            (f32::from(theme.shape.row_padding) * scale_factor as f32 * self.zoom()).round();
        let row_height = metrics.height + 2.0 * row_padding;
        let count = source.row_count();
        let whole_rows = match row_height > 0.0 {
            true => (rows.height / row_height).floor() as usize,
            false => 0,
        };
        let to_draw = RowsToDraw {
            area: rows,
            count,
            now: clock::now(),
            pinned: self.frozen_rows,
            row_height,
        };
        let mut painter = RowPainter {
            clips: layout.column_clips(rows),
            columns: &self.columns,
            font: self.font,
            frozen: layout.frozen,
            late: Vec::new(),
            line_height: metrics.height,
            padding: (PADDING_CELLS * metrics.width).round(),
            pinned: self.frozen_rows.min(count).min(whole_rows),
            row_height,
            row_padding,
            seen: HashSet::new(),
            source,
            spans: layout.spans().collect(),
            styles: Vec::new(),
            theme,
        };
        self.list.draw_rows(
            frame,
            to_draw,
            theme,
            |frame, row, rect| painter.draw_row(frame, row, rect),
            |row| source.cell_text(row, 0),
        );
        let late = painter.late;
        self.layout = Some(layout);
        self.draw_late(frame, &late, source, theme);
    }

    /// Draws what goes over the rows, each in the part of the table it
    /// belongs to: the merged ranges and the cells' borders in `late`,
    /// what `source` draws over the cells, and the cell cursor.
    fn draw_late(
        &self,
        frame: &mut Frame,
        late: &[LateCell],
        source: &(impl TableSource + ?Sized),
        theme: &Theme,
    ) {
        let (Some(layout), Some((band, _, scrolled))) = (&self.layout, self.list.pinned_split())
        else {
            return;
        };
        let (frozen_clip, scrolled_clip) = layout.column_clips(layout.area);
        let clip_of = |region: Region| {
            let rows = match region.row_frozen {
                true => band,
                false => scrolled,
            };
            let columns = match region.column_frozen {
                true => frozen_clip,
                false => scrolled_clip,
            };
            rows.intersection(&columns)
        };
        let scale_factor = frame.scale_factor();
        if !late.is_empty() {
            for region in REGIONS {
                let Some(clip) = clip_of(region) else {
                    continue;
                };
                let cells = late.iter().filter(|cell| cell.region == region);
                frame.clipped(clip, |frame| {
                    draw_late_cells(frame, cells, self.font, scale_factor, theme);
                });
            }
        }
        let open = Region {
            column_frozen: false,
            row_frozen: false,
        };
        if let Some(clip) = clip_of(open) {
            let cells = TableCells { table: self };
            frame.clipped(clip, |frame| source.draw_over(frame, &cells, theme));
        }
        self.draw_cell_cursor(frame, source, theme, clip_of);
    }

    /// Draws the cell cursor's outline around its cell (the whole range
    /// when the cell is merged), cut by `clip_of` to its part of the
    /// table; nothing without a cell cursor, a row, or a shown column.
    fn draw_cell_cursor(
        &self,
        frame: &mut Frame,
        source: &(impl TableSource + ?Sized),
        theme: &Theme,
        clip_of: impl Fn(Region) -> Option<Rect>,
    ) {
        let Some(column) = self.cell_cursor else {
            return;
        };
        let row = self.state().cursor();
        let is_shown = self
            .columns
            .get(column)
            .is_some_and(|column| !column.hidden);
        if !is_shown || row >= self.state().count() {
            return;
        }
        let cells = TableCells { table: self };
        let rect = match source.merged_range(row, column) {
            Some(range) => cells.range_rect(&range),
            None => cells.cell_rect(row, column),
        };
        let pinned = self.list.pinned_split().map_or(0, |(_, pinned, _)| pinned);
        let region = Region {
            column_frozen: self
                .layout
                .as_ref()
                .is_some_and(|layout| column < layout.frozen),
            row_frozen: row < pinned,
        };
        let (Some(rect), Some(clip)) = (rect, clip_of(region)) else {
            return;
        };
        let thickness = logical(CELL_CURSOR, frame.scale_factor());
        frame.clipped(clip, |frame| {
            for edge in outline(rect, thickness) {
                frame.fill_rect(edge, theme.accent);
            }
        });
    }

    /// Scrolls the columns sideways, when the cell cursor or the row
    /// cursor moved since they last did, to show the cell cursor's column
    /// whole in the `room` the frozen columns leave, its left edge first
    /// when it is wider; `widths` are the columns' drawn widths and
    /// `frozen` how many stay in place.
    fn reveal_cell_cursor(&mut self, widths: &[f32], frozen: usize, room: f32, scale_factor: f64) {
        let Some(column) = self.cell_cursor else {
            return;
        };
        let at = (self.state().cursor(), column);
        if self.revealed == Some(at) {
            return;
        }
        self.revealed = Some(at);
        if column < frozen || column >= widths.len() {
            return;
        }
        let left: f32 = widths[frozen..column].iter().sum();
        let right = left + widths[column];
        let scroll = self.scroll_x * scale_factor as f32;
        let scroll = if left < scroll {
            left
        } else if right > scroll + room {
            (right - room).min(left)
        } else {
            return;
        };
        self.scroll_x = scroll / scale_factor as f32;
    }

    /// Draws the header row of `layout`, its titles `row_padding` below its
    /// top: titles, the sort arrow, the borders between columns, and a line
    /// under it all; the frozen columns' titles stay in place over the
    /// others.
    fn draw_header(
        &self,
        frame: &mut Frame,
        layout: &HeaderLayout,
        stroke: f32,
        row_padding: f32,
        theme: &Theme,
    ) {
        let header = layout.header;
        frame.fill_rect(header, theme.surface);
        let (frozen_clip, scrolled_clip) = layout.column_clips(header);
        let titles = |frame: &mut Frame, columns: Range<usize>| {
            self.draw_titles(frame, layout, columns, (stroke, row_padding), theme);
        };
        frame.clipped(scrolled_clip, |frame| {
            titles(frame, layout.frozen..self.columns.len());
        });
        if layout.frozen > 0 {
            frame.clipped(frozen_clip, |frame| titles(frame, 0..layout.frozen));
        }
        let line = Rect::new(header.x, header.y + header.height, header.width, stroke);
        frame.fill_rect_over_text(line, theme.border);
    }

    /// Draws the titles of `columns` in the header of `layout`, with the
    /// sort arrow and each column's right border, `stroke` thick, the
    /// titles `row_padding` below the header's top.
    fn draw_titles(
        &self,
        frame: &mut Frame,
        layout: &HeaderLayout,
        columns: Range<usize>,
        (stroke, row_padding): (f32, f32),
        theme: &Theme,
    ) {
        let header = layout.header;
        let metrics = frame.cell_metrics();
        let title_y = header.y + row_padding;
        let padding = (PADDING_CELLS * metrics.width).round();
        let spans = layout.spans().enumerate().skip(columns.start);
        for (index, (left, right)) in spans.take(columns.len()) {
            let column = &self.columns[index];
            if column.hidden {
                continue;
            }
            let mark =
                self.sort
                    .filter(|sort| sort.column == index)
                    .map(|sort| match sort.direction {
                        SortDirection::Ascending => ASCENDING_MARK,
                        SortDirection::Descending => DESCENDING_MARK,
                    });
            let mut title_right = right - padding;
            if let Some(mark) = mark {
                let mark_x = title_right - metrics.width;
                let text = StyledText::plain(mark, MARK_COLOR);
                let clip = Rect::new(mark_x, title_y, metrics.width, metrics.height);
                frame.draw_text(&text, Point::new(mark_x, title_y), clip, theme);
                title_right = mark_x;
            }
            let cell = Rect::new(
                left + padding,
                title_y,
                title_right - left - padding,
                metrics.height,
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
        let pinned = self.list.pinned_split().map_or(0, |(_, pinned, _)| pinned);
        let visible = state.visible_rows();
        let under_pinned = visible.start + pinned;
        let rows = (0..pinned).chain(visible.filter(|row| *row >= under_pinned));
        for row in rows {
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
        self.widget.list.set_key_scheme(cx.key_scheme());
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
// RowPainter Implementations
// ========================================================================

impl<S: TableSource + ?Sized> RowPainter<'_, S> {
    /// Draws the cells of `row` in `rect`: the frozen columns' over the
    /// others, each part cut to its side.
    fn draw_row(&mut self, frame: &mut Frame, row: usize, rect: Rect) {
        let count = self.columns.len();
        if self.frozen == 0 {
            self.draw_part(frame, row, rect, 0..count);
            return;
        }
        let (frozen_clip, scrolled_clip) = self.clips;
        let frozen = self.frozen;
        frame.clipped(scrolled_clip, |frame| {
            self.draw_part(frame, row, rect, frozen..count);
        });
        frame.clipped(frozen_clip, |frame| {
            self.draw_part(frame, row, rect, 0..frozen)
        });
    }

    /// Draws the cells of `row` in `columns`, the row in `rect`: every
    /// fill, then every content, so the cells batch; a merged range or a
    /// bordered cell is kept to draw after the rows.
    fn draw_part(&mut self, frame: &mut Frame, row: usize, rect: Rect, columns: Range<usize>) {
        let mut styles = mem::take(&mut self.styles);
        styles.clear();
        let (frozen, row_frozen) = (self.frozen, row < self.pinned);
        let region = |index: usize| Region {
            column_frozen: index < frozen,
            row_frozen,
        };
        for index in columns.clone() {
            if self.columns[index].hidden {
                styles.push(None);
                continue;
            }
            if let Some(range) = self.source.merged_range(row, index) {
                self.keep_merged(range, row, rect, region(index));
                styles.push(None);
                continue;
            }
            let style = self.source.cell_style(row, index);
            if !style.is_plain() {
                let (left, right) = self.spans[index];
                let cell = Rect::new(left, rect.y, right - left, rect.height);
                if let Some(fill) = style.fill {
                    frame.fill_rect(cell, fill.resolve_rgba(self.theme));
                }
                if style.borders != CellBorders::default() {
                    self.late.push(LateCell {
                        content: None,
                        rect: cell,
                        region: region(index),
                        style,
                    });
                }
            }
            styles.push(Some(style));
        }
        for (index, style) in columns.clone().zip(&styles) {
            let Some(style) = style else {
                continue;
            };
            let align = style.align.unwrap_or(self.columns[index].align);
            let (left, right) = match style.spill {
                true => self.spill_span(row, index, align, columns.clone()),
                false => self.spans[index],
            };
            let cell = Rect::new(
                left + self.padding,
                rect.y + self.row_padding,
                right - left - 2.0 * self.padding,
                self.line_height,
            );
            let content = self.source.cell_content(row, index);
            draw_content_in(frame, &content, cell, align, self.theme, self.font);
        }
        self.styles = styles;
    }

    /// The left and right edge the text of the cell at `row` and `index`,
    /// placed by `align`, may take: its own, widened over the empty cells
    /// beside it among `columns` (hidden ones passed over).
    fn spill_span(
        &self,
        row: usize,
        index: usize,
        align: TextAlign,
        columns: Range<usize>,
    ) -> (f32, f32) {
        let free = |column: usize| {
            self.columns[column].hidden
                || (self.source.merged_range(row, column).is_none()
                    && self.source.cell_text(row, column).is_empty())
        };
        let (mut left, mut right) = self.spans[index];
        if align != TextAlign::End {
            let mut column = index + 1;
            while column < columns.end && free(column) {
                if !self.columns[column].hidden {
                    right = self.spans[column].1;
                }
                column += 1;
            }
        }
        if !matches!(align, TextAlign::Start | TextAlign::Justify) {
            let mut column = index;
            while column > columns.start && free(column - 1) {
                column -= 1;
                if !self.columns[column].hidden {
                    left = self.spans[column].0;
                }
            }
        }
        (left, right)
    }

    /// Keeps the merged `range` met on `row`, drawn in `rect`, to draw
    /// after the rows in `region`: once in each of the frozen rows and the
    /// rows that scroll, across all its rows and columns, with its top
    /// left cell's content and style.
    fn keep_merged(&mut self, range: CellRange, row: usize, rect: Rect, region: Region) {
        let first = range.columns.start;
        if !self
            .seen
            .insert((range.rows.start, first, region.row_frozen))
        {
            return;
        }
        let last = range.columns.end.saturating_sub(1).max(first);
        let (Some(&(left, _)), Some(&(_, right))) = (self.spans.get(first), self.spans.get(last))
        else {
            return;
        };
        let above = row.saturating_sub(range.rows.start) as f32 * self.row_height;
        let height = range.rows.len().max(1) as f32 * self.row_height;
        let rect = Rect::new(left, rect.y - above, right - left, height);
        let style = self.source.cell_style(range.rows.start, first);
        let content = self.source.cell_content(range.rows.start, first);
        let align = style.align.unwrap_or(self.columns[first].align);
        self.late.push(LateCell {
            content: Some((content, align)),
            rect,
            region: Region {
                column_frozen: first < self.frozen,
                ..region
            },
            style,
        });
    }
}

// ========================================================================
// HeaderLayout Implementations
// ========================================================================

impl HeaderLayout {
    /// Each column's left and right edge, left to right. The first column
    /// that scrolls starts where the frozen ones end (the table's left
    /// without any) however far it is scrolled, as its cells are drawn.
    fn spans(&self) -> impl Iterator<Item = (f32, f32)> + '_ {
        let lefts = iter::once(self.header.x)
            .chain(self.edges.iter().copied())
            .enumerate()
            .map(|(index, left)| match index == self.frozen {
                true => self.frozen_right,
                false => left,
            });
        lefts.zip(self.edges.iter().copied())
    }

    /// The part of `rows` (a band across the table) the frozen columns
    /// are drawn in, and the part the others scroll in: all of `rows`
    /// without frozen columns.
    fn column_clips(&self, rows: Rect) -> (Rect, Rect) {
        let right = self.area.x + self.area.width;
        (
            Rect::new(
                self.area.x,
                rows.y,
                self.frozen_right - self.area.x,
                rows.height,
            ),
            Rect::new(
                self.frozen_right,
                rows.y,
                right - self.frozen_right,
                rows.height,
            ),
        )
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
            .filter(|(column, (edge, _))| {
                self.frozen == 0 || *column < self.frozen || **edge > self.frozen_right
            })
            .filter(|(_, (edge, width))| *width > 0.0 && (x - **edge).abs() <= margin)
            .map(|(column, (edge, _))| (column, edge))
            .min_by(|(_, a), (_, b)| (x - **a).abs().total_cmp(&(x - **b).abs()))
            .map(|(column, _)| column)
    }

    /// How wide `column` was drawn, in logical pixels.
    fn drawn_width(&self, column: usize) -> f32 {
        self.spans().nth(column).map_or(0.0, |(left, right)| {
            (right - left) / self.width_scale as f32
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

/// Draws `cells`, kept to draw after the rows in `font`: each merged
/// range's fill and content (centered down the range), then every cell's
/// borders.
fn draw_late_cells<'a>(
    frame: &mut Frame,
    cells: impl Iterator<Item = &'a LateCell> + Clone,
    font: RowFont,
    scale_factor: f64,
    theme: &Theme,
) {
    let metrics = frame.cell_metrics();
    let padding = (PADDING_CELLS * metrics.width).round();
    for cell in cells.clone() {
        let Some((content, align)) = &cell.content else {
            continue;
        };
        let rect = cell.rect;
        if let Some(fill) = cell.style.fill {
            frame.fill_rect(rect, fill.resolve_rgba(theme));
        }
        let top = (rect.y + (rect.height - metrics.height) / 2.0).round();
        let line = Rect::new(
            rect.x + padding,
            top,
            rect.width - 2.0 * padding,
            metrics.height,
        );
        frame.clipped(rect, |frame| {
            draw_content_in(frame, content, line, *align, theme, font);
        });
    }
    for cell in cells {
        for (edge, color) in cell.style.borders.edges(cell.rect, scale_factor) {
            frame.fill_rect_over_text(edge, color.resolve_rgba(theme));
        }
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

/// Draws `text` in `color` as a label placed in `cell` by `align`,
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
        TextAlign::Justify | TextAlign::Start => cell.x,
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
    use crate::base::color::{Rgb, Rgba};
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
        .with_vim_and_emacs()
    }

    /// The header of [`table`] drawn 20 pixels high at the top left, on a
    /// display scaled by `scale_factor`.
    fn header(scale_factor: f64) -> HeaderLayout {
        let scale = scale_factor as f32;
        HeaderLayout {
            area: Rect::new(0.0, 0.0, 600.0, 400.0),
            edges: vec![200.0 * scale, 300.0 * scale],
            frozen: 0,
            frozen_right: 0.0,
            header: Rect::new(0.0, 0.0, 600.0, 20.0),
            max_scroll: 0.0,
            scale_factor,
            width_scale: scale_factor,
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
    fn test_taking_the_apps_columns_keeps_a_dragged_width_unless_the_app_changed_it() {
        let last = vec![
            TableColumn::new("Name", 100.0),
            TableColumn::new("Size", 80.0),
        ];
        let mut table = Table::new(last.clone()).with_vim_and_emacs();
        table.set_column_widths(&[150.0, 90.0]);
        let mut columns = last.clone();
        columns[0] = columns[0].clone().with_hidden(true);
        columns[1] = TableColumn::new("Size", 60.0);
        table.take_columns(&columns, &last);
        assert!(table.columns()[0].is_hidden());
        assert_eq!(table.column_widths(), [150.0, 60.0]);
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
        let mut table = Table::new(with_fill()).with_vim_and_emacs();
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
        let mut window = crate::testing::TestWindow::keyboard_first(600, 200);
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
        let mut table = Table::new(with_fill()).with_vim_and_emacs();
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
        let mut table = Table::new(with_fill()).with_vim_and_emacs();
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
        let mut table = Table::new(with_fill()).with_vim_and_emacs();
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

    /// How many text segments `table` draws in the proportional font in a
    /// 400 by 200 frame in `theme`.
    fn shaped_segments(table: &mut Table, theme: &Theme) -> usize {
        let mut frame = offscreen::plain_frame(400, 200);
        let area = Rect::new(0.0, 0.0, 400.0, 200.0);
        table.lend(&Files).draw(&mut frame, area, theme);
        frame
            .draw
            .text()
            .iter()
            .filter(|segment| segment.wrap.is_some())
            .count()
    }

    #[test]
    fn test_a_proportional_table_shapes_its_cells_and_a_plain_one_keeps_them_on_the_grid() {
        let theme = Theme::dark();
        let plain = shaped_segments(&mut table(), &theme);
        let proportional = shaped_segments(&mut table().with_proportional_font(), &theme);
        let rows = table().columns().len() * 3;
        assert!(
            proportional >= plain + rows,
            "{proportional} against {plain}"
        );
    }

    #[test]
    fn test_rows_take_the_shapes_padding_above_and_below_and_none_on_the_grid() {
        let pitch = |theme: &Theme| {
            let mut table = table();
            let mut frame = offscreen::plain_frame(400, 200);
            table
                .lend(&Files)
                .draw(&mut frame, Rect::new(0.0, 0.0, 400.0, 200.0), theme);
            let (first, second) = (table.row_rect(0), table.row_rect(1));
            let height = frame.cell_metrics().height;
            (second.expect("drawn").y - first.expect("drawn").y, height)
        };
        let theme = Theme::dark();
        let (modern, height) = pitch(&theme);
        let padding = f32::from(theme.shape.row_padding);
        assert_eq!(modern, height + 2.0 * padding, "padded");
        let (grid, height) = pitch(&Theme::dark().with_grid_look());
        assert_eq!(grid, height, "a cell tall");
    }
    #[test]
    fn test_a_default_tables_rows_cells_and_visible_range_stay_where_they_were() {
        let theme = Theme::dark();
        let mut table = table();
        let mut frame = offscreen::plain_frame(600, 200);
        table
            .lend(&Files)
            .draw(&mut frame, Rect::new(0.0, 0.0, 600.0, 200.0), &theme);
        let height = frame.cell_metrics().height;
        let row_height = height + 2.0 * f32::from(theme.shape.row_padding);
        let top = row_height + STROKE;
        for row in [0, 1, 4] {
            assert_eq!(
                table.row_rect(row),
                Some(Rect::new(
                    0.0,
                    top + row as f32 * row_height,
                    600.0,
                    row_height
                )),
                "row {row}"
            );
        }
        let cell = |row: usize, column| table.cell_rect(row, column).expect("drawn");
        assert_eq!(
            cell(1, 0),
            Rect::new(0.0, top + row_height, 200.0, row_height)
        );
        assert_eq!(
            cell(1, 1),
            Rect::new(200.0, top + row_height, 100.0, row_height)
        );
        let shown = ((200.0 - top) / row_height).floor() as usize;
        assert_eq!(table.visible_range(), 0..shown, "whole rows from the top");
    }

    #[test]
    fn test_the_wheel_scrolls_a_default_tables_rows_and_drags_the_cursor_along() {
        let mut window = crate::testing::TestWindow::keyboard_first(600, 200);
        let mut table = table();
        draw_in(&mut table, 600);
        let inside = Point::new(100.0, 100.0);
        let send = |table: &mut Table, window: &mut crate::testing::TestWindow, input: Input| {
            window.send(&input, |input, cx| table.lend(&Files).handle(input, cx))
        };
        send(&mut table, &mut window, Input::PointerMoved(inside));
        let notch =
            crate::input::event::WheelInput::new(WheelDelta::Lines(Point::new(0.0, -1.0)), inside);
        send(&mut table, &mut window, Input::Wheel(notch));
        assert_eq!(table.state().scroll(), 3, "three rows a notch");
        assert_eq!(
            table.state().cursor(),
            3 + table.state().margin(),
            "the cursor kept inside the margin"
        );
        draw_in(&mut table, 600);
        assert_eq!(table.state().scroll(), 3, "a redraw keeps the scroll");
    }

    /// The color [`Sheet`] fills cell 1, 1 with.
    const FILL: Rgba = Rgba::new(200, 40, 40, 255);

    /// The color [`Sheet`] borders cell 2, 1 in.
    const LINE: Rgba = Rgba::new(40, 200, 40, 255);

    /// The color [`Sheet`] fills its merged range with.
    const MERGED: Rgba = Rgba::new(40, 40, 200, 255);

    /// The color [`Sheet`] draws over rows 10 to 11.
    const OVER: Rgba = Rgba::new(250, 250, 0, 255);

    /// [`Files`] styled as a sheet: cell 1, 1 filled, cell 2, 1 bordered,
    /// rows 4 and 5 of both columns merged, and a band drawn over rows 10
    /// and 11.
    struct Sheet;

    impl TableSource for Sheet {
        fn row_count(&self) -> usize {
            Files.row_count()
        }

        fn cell_text(&self, row: usize, column: usize) -> Cow<'_, str> {
            match (row, column) {
                (4, 0) => Cow::Borrowed("merged"),
                _ => Files.cell_text(row, column),
            }
        }

        fn cell_style(&self, row: usize, column: usize) -> TableCellStyle {
            match (row, column) {
                (1, 1) => TableCellStyle::default().with_fill(TextColor::Rgba(FILL)),
                (2, 1) => TableCellStyle::default().with_borders(CellBorders::all(
                    CellBorder::new(TextColor::Rgba(LINE), 1.0),
                )),
                (4, 0) => TableCellStyle::default().with_fill(TextColor::Rgba(MERGED)),
                _ => TableCellStyle::default(),
            }
        }

        fn merged_range(&self, row: usize, column: usize) -> Option<CellRange> {
            ((4..6).contains(&row) && column < 2).then(|| CellRange::new(4..6, 0..2))
        }

        fn draw_over(&self, frame: &mut Frame, cells: &TableCells<'_>, _theme: &Theme) {
            if let Some(rect) = cells.range_rect(&CellRange::new(10..12, 0..1)) {
                frame.fill_rect(rect, OVER);
            }
        }
    }

    /// The rectangles of the quads `frame` holds in `color`.
    fn quads_of(frame: &Frame, color: Rgba) -> Vec<Rect> {
        frame
            .draw
            .quads()
            .filter(|quad| quad.color == color)
            .map(|quad| quad.rect)
            .collect()
    }

    #[test]
    fn test_a_cells_fill_covers_it_whole_and_its_borders_run_along_its_edges() {
        let mut table = table();
        let mut frame = offscreen::plain_frame(600, 400);
        let area = Rect::new(0.0, 0.0, 600.0, 400.0);
        table.lend(&Sheet).draw(&mut frame, area, &Theme::dark());
        let filled = table.cell_rect(1, 1).expect("drawn");
        assert_eq!(quads_of(&frame, FILL), [filled], "padding and all");
        let bordered = table.cell_rect(2, 1).expect("drawn");
        let lines = quads_of(&frame, LINE);
        assert_eq!(lines.len(), 4, "{lines:?}");
        assert!(
            lines.contains(&Rect::new(bordered.x, bordered.y, bordered.width, 1.0)),
            "the top edge on the grid line: {lines:?}"
        );
        assert!(
            lines.contains(&Rect::new(
                bordered.x + bordered.width,
                bordered.y,
                1.0,
                bordered.height
            )),
            "the right edge where the next cell starts: {lines:?}"
        );
    }

    #[test]
    fn test_a_merged_range_draws_once_across_its_cells_with_its_first_cells_content() {
        let mut table = table();
        let mut frame = offscreen::plain_frame(600, 400);
        let area = Rect::new(0.0, 0.0, 600.0, 400.0);
        table.lend(&Sheet).draw(&mut frame, area, &Theme::dark());
        let first = table.cell_rect(4, 0).expect("drawn");
        let last = table.cell_rect(5, 1).expect("drawn");
        let whole = Rect::new(
            first.x,
            first.y,
            last.x + last.width - first.x,
            last.y + last.height - first.y,
        );
        assert_eq!(quads_of(&frame, MERGED), [whole]);
        let texts: Vec<String> = frame
            .draw
            .text()
            .iter()
            .map(|segment| segment.runs.iter().map(|run| run.text.as_str()).collect())
            .collect();
        assert_eq!(texts.iter().filter(|text| *text == "merged").count(), 1);
        assert!(
            !texts.iter().any(|text| text == "50 KB" || text == "40 KB"),
            "the covered cells draw nothing: {texts:?}"
        );
        let over = table.cell_rect(10, 0).expect("drawn");
        let band = quads_of(&frame, OVER);
        assert_eq!(band.len(), 1);
        assert_eq!(
            (band[0].x, band[0].y),
            (over.x, over.y),
            "drawn over the cells"
        );
    }

    /// A long title in each row's first cell, spilling, beside an empty
    /// second cell on row 0 and a full one on row 1.
    struct Spill;

    /// The title [`Spill`] puts in its first cells.
    const LONG: &str = "a title far too long for the two hundred pixels of its own cell";

    impl TableSource for Spill {
        fn row_count(&self) -> usize {
            2
        }

        fn cell_text(&self, row: usize, column: usize) -> Cow<'_, str> {
            match (row, column) {
                (_, 0) => Cow::Borrowed(LONG),
                (0, _) => Cow::Borrowed(""),
                _ => Cow::Borrowed("1 KB"),
            }
        }

        fn cell_style(&self, _row: usize, _column: usize) -> TableCellStyle {
            TableCellStyle::default()
                .with_align(TextAlign::Start)
                .with_spill(true)
        }
    }

    #[test]
    fn test_spilling_text_runs_on_over_an_empty_neighbor_only() {
        let mut table = Table::new(vec![
            TableColumn::new("Name", 200.0),
            TableColumn::new("Size", 400.0),
        ])
        .with_vim_and_emacs();
        let mut frame = offscreen::plain_frame(600, 400);
        let area = Rect::new(0.0, 0.0, 600.0, 400.0);
        table.lend(&Spill).draw(&mut frame, area, &Theme::dark());
        let texts: Vec<String> = frame
            .draw
            .text()
            .iter()
            .map(|segment| segment.runs.iter().map(|run| run.text.as_str()).collect())
            .collect();
        assert!(
            texts.iter().any(|text| text == LONG),
            "whole beside the empty cell: {texts:?}"
        );
        assert!(
            texts
                .iter()
                .any(|text| text != LONG && text.starts_with("a title")),
            "cut beside the full cell: {texts:?}"
        );
    }

    #[test]
    fn test_frozen_rows_and_columns_stay_in_place_while_the_others_scroll() {
        let mut table = Table::new(with_fill())
            .with_vim_and_emacs()
            .with_frozen(2, 1);
        table.state_mut().set_count(Files.row_count());
        draw_in(&mut table, 250);
        let top = table.row_rect(0).expect("drawn");
        table.handle_key(parse_key("$").expect("a key"));
        table.state_mut().set_cursor(40);
        draw_in(&mut table, 250);
        assert_eq!(edges(&table)[0], 150.0, "the frozen column stays");
        assert_eq!(table.cell_rect(0, 0).map(|cell| cell.x), Some(0.0));
        assert_eq!(table.row_rect(0), Some(top), "the frozen row stays");
        let second = table.row_rect(1).expect("drawn");
        let cursor = table.row_rect(40).expect("drawn");
        assert!(
            cursor.y >= second.y + second.height,
            "the cursor's row below the frozen ones: {cursor:?}"
        );
        let layout = table.layout.as_ref().expect("drawn");
        assert_eq!(layout.column_at(10.0), Some(0), "the frozen column on top");
        assert_eq!(
            layout.column_at(160.0),
            Some(2),
            "column 1 scrolled under it"
        );
        let under = edges(&table)[1];
        assert!(under < 150.0, "column 1 scrolled under: {under}");
        assert_eq!(
            layout.border_at(under),
            None,
            "a border under the frozen column is not grabbed"
        );
    }

    #[test]
    fn test_the_cell_cursor_moves_between_columns_and_scrolls_its_column_into_sight() {
        let mut table = table();
        table.set_cell_cursor(Some(0));
        draw_in(&mut table, 250);
        let press = |table: &mut Table, spec| table.handle_key(parse_key(spec).expect("a key"));
        assert_eq!(press(&mut table, "l"), TableResponse::CellCursorMoved(1));
        draw_in(&mut table, 250);
        assert_eq!(
            edges(&table),
            [150.0, 250.0],
            "column 1 scrolled into sight"
        );
        assert_eq!(
            press(&mut table, "l"),
            TableResponse::Handled,
            "at the last"
        );
        assert_eq!(press(&mut table, "0"), TableResponse::CellCursorMoved(0));
        draw_in(&mut table, 250);
        assert_eq!(edges(&table), [200.0, 300.0], "back to the first");
        let mut window = crate::testing::TestWindow::keyboard_first(250, 200);
        let press_at = Input::PointerButton(crate::input::event::ButtonInput::press(
            PointerButton::Left,
            Point::new(220.0, 100.0),
        ));
        window.send(
            &Input::PointerMoved(Point::new(220.0, 100.0)),
            |input, cx| table.lend(&Files).handle(input, cx),
        );
        window.send(&press_at, |input, cx| table.lend(&Files).handle(input, cx));
        assert_eq!(table.cell_cursor(), Some(1), "a click moves it");
    }

    #[test]
    fn test_a_zoomed_table_scales_its_rows_and_widths_and_resizes_in_unzoomed_pixels() {
        let theme = Theme::dark();
        let mut table = table().with_zoom(2.0);
        let mut frame = offscreen::plain_frame(800, 400);
        let area = Rect::new(0.0, 0.0, 800.0, 400.0);
        table.lend(&Files).draw(&mut frame, area, &theme);
        let height = frame.cell_metrics().height;
        let padding = (f32::from(theme.shape.row_padding) * 2.0).round();
        let row = table.row_rect(0).expect("drawn");
        assert_eq!(row.height, (height * 2.0).round() + 2.0 * padding);
        assert_eq!(edges(&table), [400.0, 600.0]);
        table.pointer = Some(Point::new(400.0, 5.0));
        assert_eq!(table.press_header(), Some(TableResponse::Handled));
        table.drag_border_to(420.0);
        assert_eq!(table.column_widths(), [210.0, 100.0], "20 drawn is 10 own");
        assert_eq!(table.clone().with_zoom(f32::NAN).zoom(), 1.0);
    }

    #[test]
    fn test_a_fill_paints_the_cells_corners_and_the_cell_cursor_is_outlined_in_the_accent() {
        let theme = Theme::dark();
        let mut table = table();
        table.set_cell_cursor(Some(0));
        table.state_mut().set_count(Sheet.row_count());
        table.state_mut().set_cursor(2);
        let area = Rect::new(0.0, 0.0, 400.0, 300.0);
        let mut drawn = table.clone();
        let Some(snapshot) = offscreen::render(400, 300, |frame| {
            frame.clear(Rgba::from(theme.background));
            drawn.lend(&Sheet).draw(frame, area, &theme);
        }) else {
            return;
        };
        let filled = drawn.cell_rect(1, 1).expect("drawn");
        let corner = |x: f32, y: f32| snapshot.pixel(x, y);
        let fill = Rgb::new(FILL.r, FILL.g, FILL.b);
        assert_eq!(corner(filled.x + 1.0, filled.y + 1.0), fill, "top left");
        let (right, bottom) = (filled.x + filled.width, filled.y + filled.height);
        assert_eq!(corner(right - 2.0, bottom - 1.0), fill, "bottom right");
        let cursor = drawn.cell_rect(2, 0).expect("drawn");
        snapshot.assert_pixel(
            cursor.x + cursor.width / 2.0,
            cursor.y + 0.5,
            theme.accent,
            "the outline",
        );
    }
}
