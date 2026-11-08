//! One grid cell, the cursor that sits on one, and the highlights laid over
//! runs of them.

use std::ops::Range;

use super::super::color::Rgba;
use super::style::CellStyle;

// ========================================================================
// Constants
// ========================================================================

/// What an empty cell holds.
const BLANK: char = ' ';

// ========================================================================
// Data Structures
// ========================================================================

/// One grid cell: the grapheme drawn in it, its style, and its part in
/// double-width layout. The default cell is a blank in the default style.
///
/// A grapheme is stored as its first character in `ch` plus, in the rare case
/// it has more (an emoji ZWJ sequence, a flag, a variation selector), the
/// rest in `tail`, so the common cell carries no heap allocation.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Cell {
    /// The grapheme's first character; `' '` for a blank cell. Ignored in a
    /// [`CellWidth::Continuation`] cell.
    pub ch: char,
    /// How the cell is drawn.
    pub style: CellStyle,
    /// The grapheme's characters after `ch`, or `None` when `ch` is the whole
    /// grapheme.
    pub tail: Option<Box<str>>,
    /// The cell's part in double-width layout.
    pub width: CellWidth,
}

/// A cell's part in double-width layout. A wide grapheme (most CJK
/// characters and emoji) spans two columns: a [`CellWidth::Wide`] cell holding
/// it, then a [`CellWidth::Continuation`] cell to its right holding nothing.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum CellWidth {
    /// The right half of a wide grapheme: drawn by the cell to its left, so a
    /// renderer skips it.
    Continuation,
    /// A grapheme one column wide.
    #[default]
    Single,
    /// The left half of a wide grapheme, which is drawn across this column
    /// and the next.
    Wide,
}

/// Where the cursor is and how it is drawn.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Cursor {
    /// The column, from 0 at the left.
    pub col: usize,
    /// The row, from 0 at the top.
    pub row: usize,
    /// How the cursor is drawn.
    pub shape: CursorShape,
}

/// How the cursor is drawn. A hidden cursor is no cursor at all: see
/// [`CellSource::cursor`](super::CellSource::cursor).
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum CursorShape {
    /// A thin vertical line at the cell's left edge, the usual insert cursor.
    Bar,
    /// The whole cell filled.
    #[default]
    Block,
    /// The cell's outline, the usual cursor of a window or pane without the
    /// keyboard: the glyph inside keeps its own colors.
    Hollow,
    /// A line along the cell's bottom edge.
    Underline,
}

/// A tint laid over the backgrounds of a run of cells in one row: a
/// selection, a search match, or a band across a line. Its alpha says how
/// much of it covers the background (255 replaces it), and the glyphs, their
/// lines, and a block cursor stay on top.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CellHighlight {
    /// The tint, its alpha the share of the background it covers.
    pub color: Rgba,
    /// The columns covered, from 0 at the left; a wide cell is tinted when
    /// either of its columns is.
    pub cols: Range<usize>,
    /// The row, from 0 at the top.
    pub row: usize,
}

// ========================================================================
// Cell Implementations
// ========================================================================

impl Cell {
    /// A single-width cell drawing `ch` in `style`.
    pub fn new(ch: char, style: CellStyle) -> Self {
        Self {
            ch,
            style,
            tail: None,
            width: CellWidth::Single,
        }
    }
}

impl Default for Cell {
    fn default() -> Self {
        Self::new(BLANK, CellStyle::default())
    }
}

// ========================================================================
// Cursor Implementations
// ========================================================================

impl Cursor {
    /// A cursor at `row` and `col`, drawn as `shape`.
    pub fn new(row: usize, col: usize, shape: CursorShape) -> Self {
        Self { col, row, shape }
    }
}

// ========================================================================
// CellHighlight Implementations
// ========================================================================

impl CellHighlight {
    /// A highlight of the columns `cols` of `row`, tinted `color`.
    pub fn new(row: usize, cols: Range<usize>, color: Rgba) -> Self {
        Self { color, cols, row }
    }

    /// Whether the highlight covers any of the columns `cols`.
    pub(crate) fn overlaps(&self, cols: &Range<usize>) -> bool {
        self.cols.start < cols.end && cols.start < self.cols.end
    }
}
