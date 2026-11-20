//! The grid a renderer reads, behind [`CellSource`], and [`CellBuffer`], a
//! plain grid of cells for apps that are not terminals.

use std::mem;

use super::value::{Cell, CellHighlight, CellWidth, Cursor};

// ========================================================================
// Data Structures
// ========================================================================

/// A grid of `cols` by `rows` cells you write directly, with an optional
/// cursor. Rows and columns count from 0 at the top left; every cell starts as
/// [`Cell::default`].
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CellBuffer {
    /// The cells, row after row.
    cells: Vec<Cell>,
    /// The width in cells.
    cols: usize,
    /// The cursor, or `None` when hidden.
    cursor: Option<Cursor>,
    /// The height in cells.
    rows: usize,
}

// ========================================================================
// Traits
// ========================================================================

/// A grid of cells as a renderer sees it: its size, each cell, and the
/// cursor. A terminal implements it over its screen, so the renderer draws
/// from the terminal's own storage without copying it each frame.
pub trait CellSource {
    /// The width in cells.
    fn cols(&self) -> usize;

    /// The height in cells.
    fn rows(&self) -> usize;

    /// The cell at `row` and `col`, counted from 0 at the top left, or `None`
    /// when either is out of range.
    fn cell(&self, row: usize, col: usize) -> Option<&Cell>;

    /// The cursor, or `None` when no cursor is drawn.
    fn cursor(&self) -> Option<Cursor>;

    /// Whether glyphs with [`Attributes::BLINK`](super::Attributes::BLINK)
    /// show now: the phase of the source's own blink timer, which the app
    /// turns and redraws on. The default always shows them.
    fn blink_visible(&self) -> bool {
        true
    }

    /// The [`CellHighlight`]s over the cells now, each tinting the backgrounds
    /// as left by the ones before it. The default has none.
    fn highlights(&self) -> &[CellHighlight] {
        &[]
    }
}

// ========================================================================
// CellBuffer Implementations
// ========================================================================

impl CellBuffer {
    /// A grid `cols` cells wide and `rows` high, all blank, with no cursor.
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            cells: vec![Cell::default(); cols * rows],
            cols,
            cursor: None,
            rows,
        }
    }

    /// Writes `cell` at `row` and `col`, returning the cell it replaced. Out
    /// of range, nothing is written and the result is `None`.
    ///
    /// The buffer does not keep wide pairs together: writing a
    /// [`CellWidth::Wide`] cell, write the [`CellWidth::Continuation`] to its
    /// right as well.
    pub fn set(&mut self, row: usize, col: usize, cell: Cell) -> Option<Cell> {
        let index = self.index_of(row, col)?;
        Some(mem::replace(&mut self.cells[index], cell))
    }

    /// Shows the cursor at `cursor`, or hides it with `None`. A cursor outside
    /// the grid is drawn nowhere, but kept.
    pub fn set_cursor(&mut self, cursor: Option<Cursor>) {
        self.cursor = cursor;
    }

    /// Makes every cell blank. The cursor stays where it is.
    pub fn clear(&mut self) {
        self.cells.fill(Cell::default());
    }

    /// Changes the size to `cols` by `rows`, anchored at the top left: cells
    /// inside both the old and the new size keep their content, new cells are
    /// blank. A wide cell left in the last column without its continuation
    /// becomes blank, and the cursor moves to stay inside the grid (or is
    /// hidden when the grid is empty).
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let mut cells = Vec::with_capacity(cols * rows);
        for row in 0..rows {
            for col in 0..cols {
                let kept = self
                    .index_of(row, col)
                    .map(|index| mem::take(&mut self.cells[index]));
                cells.push(kept.unwrap_or_default());
            }
            if let Some(last) = cells.last_mut().filter(|_| cols > 0)
                && last.width == CellWidth::Wide
            {
                *last = Cell::default();
            }
        }
        self.cells = cells;
        self.cols = cols;
        self.rows = rows;
        self.cursor = self.cursor.and_then(|cursor| {
            let col = cursor.col.min(cols.checked_sub(1)?);
            let row = cursor.row.min(rows.checked_sub(1)?);
            Some(Cursor::new(row, col, cursor.shape))
        });
    }

    /// The position of `row` and `col` in `cells`, or `None` out of range.
    fn index_of(&self, row: usize, col: usize) -> Option<usize> {
        (row < self.rows && col < self.cols).then(|| row * self.cols + col)
    }
}

impl CellSource for CellBuffer {
    fn cols(&self) -> usize {
        self.cols
    }

    fn rows(&self) -> usize {
        self.rows
    }

    fn cell(&self, row: usize, col: usize) -> Option<&Cell> {
        self.index_of(row, col).map(|index| &self.cells[index])
    }

    fn cursor(&self) -> Option<Cursor> {
        self.cursor
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::super::style::CellStyle;
    use super::super::value::CursorShape;
    use super::*;

    fn cell(ch: char) -> Cell {
        Cell::new(ch, CellStyle::default())
    }

    fn wide(ch: char) -> Cell {
        Cell {
            width: CellWidth::Wide,
            ..cell(ch)
        }
    }

    fn continuation() -> Cell {
        Cell {
            width: CellWidth::Continuation,
            ..Cell::default()
        }
    }

    /// A buffer whose cell at (row, col) holds the letter `a` + row * cols + col.
    fn lettered(cols: usize, rows: usize) -> CellBuffer {
        let mut buffer = CellBuffer::new(cols, rows);
        for (index, letter) in ('a'..).take(cols * rows).enumerate() {
            buffer.set(index / cols, index % cols, cell(letter));
        }
        buffer
    }

    fn chars(buffer: &CellBuffer) -> Vec<String> {
        (0..buffer.rows())
            .map(|row| {
                (0..buffer.cols())
                    .map(|col| buffer.cell(row, col).map_or('?', |cell| cell.ch))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn test_growing_keeps_content_at_the_top_left_and_fills_with_blanks() {
        let mut buffer = lettered(2, 2);
        buffer.resize(3, 3);
        assert_eq!(chars(&buffer), ["ab ", "cd ", "   "]);
        assert_eq!(buffer.cell(2, 2), Some(&Cell::default()));
    }

    #[test]
    fn test_shrinking_keeps_the_overlap_row_by_row() {
        let mut buffer = lettered(3, 3);
        buffer.resize(2, 2);
        assert_eq!(chars(&buffer), ["ab", "de"]);
        buffer.resize(4, 1);
        assert_eq!(chars(&buffer), ["ab  "]);
    }

    #[test]
    fn test_out_of_range_reads_none_and_writes_nothing() {
        let mut buffer = lettered(2, 2);
        assert_eq!(buffer.cell(0, 2), None);
        assert_eq!(buffer.cell(2, 0), None);
        assert_eq!(buffer.set(0, 2, cell('x')), None);
        assert_eq!(buffer.set(5, 0, cell('x')), None);
        assert_eq!(chars(&buffer), ["ab", "cd"]);
        assert_eq!(CellBuffer::new(0, 3).cell(0, 0), None);
    }

    #[test]
    fn test_set_returns_the_replaced_cell() {
        let mut buffer = lettered(2, 1);
        assert_eq!(buffer.set(0, 1, cell('x')), Some(cell('b')));
        assert_eq!(buffer.cell(0, 1), Some(&cell('x')));
    }

    #[test]
    fn test_shrinking_through_a_wide_pair_blanks_its_left_half() {
        let mut buffer = CellBuffer::new(4, 1);
        buffer.set(0, 0, cell('a'));
        buffer.set(0, 1, wide('世'));
        buffer.set(0, 2, continuation());
        buffer.resize(2, 1);
        assert_eq!(buffer.cell(0, 1), Some(&Cell::default()));
        assert_eq!(buffer.cell(0, 0), Some(&cell('a')));
    }

    #[test]
    fn test_shrinking_past_a_whole_wide_pair_keeps_it() {
        let mut buffer = CellBuffer::new(4, 1);
        buffer.set(0, 0, wide('世'));
        buffer.set(0, 1, continuation());
        buffer.resize(2, 1);
        assert_eq!(buffer.cell(0, 0), Some(&wide('世')));
        assert_eq!(buffer.cell(0, 1), Some(&continuation()));
    }

    #[test]
    fn test_resize_moves_the_cursor_inside_and_hides_it_when_empty() {
        let mut buffer = CellBuffer::new(5, 5);
        buffer.set_cursor(Some(Cursor::new(4, 3, CursorShape::Bar)));
        buffer.resize(2, 3);
        assert_eq!(buffer.cursor(), Some(Cursor::new(2, 1, CursorShape::Bar)));
        buffer.resize(0, 3);
        assert_eq!(buffer.cursor(), None);
    }

    #[test]
    fn test_clear_blanks_every_cell_but_keeps_size_and_cursor() {
        let mut buffer = lettered(2, 2);
        let cursor = Some(Cursor::new(1, 1, CursorShape::Block));
        buffer.set_cursor(cursor);
        buffer.clear();
        assert_eq!(chars(&buffer), ["  ", "  "]);
        assert_eq!(buffer.cursor(), cursor);
    }
}
