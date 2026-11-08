//! Lays a [`CellSource`] out as a monospace grid: a fill per run of equal
//! backgrounds (highlights tinted in), text segments placed on cell
//! boundaries, and quads for underlines, strikethroughs, braille dots, and
//! the cursor.
//!
//! Plain ASCII glyphs of the window font advance exactly one cell, so a run
//! of them is shaped as one segment. Anything else (wide cells, non-ASCII
//! characters that may come from a fallback font, multi-character
//! graphemes) gets a segment of its own, centered in its cells (and shrunk
//! to them when wider), so a glyph that is wider or narrower than its cells
//! never shifts or overlaps the ones after it.
//! Braille patterns are drawn as dots on each cell's own dot matrix, not
//! from a font, so a braille graph lines up whatever font supplies them. A
//! curly underline is a smooth antialiased wave, one path per run of cells
//! it underlines in one color, a period per cell.

use std::f32::consts::TAU;
use std::ops::{Range, RangeInclusive};

use super::super::base::cell::{
    Attributes, Cell, CellColor, CellHighlight, CellSource, CellStyle, CellWidth, Cursor,
    CursorShape, UnderlineStyle,
};
use super::super::base::color::{Rgb, Rgba};
use super::super::base::geometry::{Point, Rect};
use super::super::base::motion::Lerp;
use super::super::base::paint::{LineJoin, Paint, Stroke};
use super::super::base::path::VectorPath;
use super::super::base::theme::Theme;
use super::super::base::transform::Transform;
use super::draw::{DrawList, Face, PathDraw, Quad, RunStyle, SegmentAlign, TextRun, TextSegment};
use super::font::CellMetrics;
use super::tessellate;

// ========================================================================
// Constants
// ========================================================================

/// What a blank cell holds.
const BLANK: char = ' ';

/// How far a dim glyph is blended toward its background, from 0 (not at
/// all) to 1 (invisible).
const DIM_BLEND: f32 = 0.5;

/// How many line thicknesses wide a bar cursor is.
const BAR_THICKNESSES: f32 = 2.0;

/// The share of a cell a dash of a dashed underline covers.
const DASH_SHARE: f32 = 0.5;

/// Unicode's Braille Patterns block: the blank pattern, then one character
/// per set of the eight dots, dot `n` as bit `n - 1`.
const BRAILLE: RangeInclusive<char> = '\u{2800}'..='\u{28FF}';

/// The columns and rows of a braille cell's dot matrix.
const BRAILLE_COLS: f32 = 2.0;
const BRAILLE_ROWS: f32 = 4.0;

/// Each braille dot's column and row in the matrix, by bit: dots 1, 2, 3
/// down the left, 4, 5, 6 down the right, then 7 and 8 under them.
const BRAILLE_DOTS: [(u8, u8); 8] = [
    (0, 0),
    (0, 1),
    (0, 2),
    (1, 0),
    (1, 1),
    (1, 2),
    (0, 3),
    (1, 3),
];

/// A braille dot's diameter, as a share of the shorter side of its slot.
const BRAILLE_DOT_SHARE: f32 = 0.9;

/// How far a curly underline swings either way, as a share of the cell's
/// height, and the least it swings, in pixels, so it reads as a wave at
/// any size.
const CURL_AMPLITUDE_SHARE: f32 = 0.07;
const CURL_MIN_AMPLITUDE: f32 = 1.25;

/// A curly underline's stroke, as a share of the cell's height, and the
/// thinnest it is drawn, in pixels: an antialiased curve as thin as a
/// straight line would read fainter than one.
const CURL_THICKNESS_SHARE: f32 = 0.06;
const CURL_MIN_THICKNESS: f32 = 1.2;

/// The most pixels between the points a curly underline is drawn through,
/// so it reads as a curve and not a run of straight pieces.
const CURL_STEP: f32 = 1.0;

// ========================================================================
// Data Structures
// ========================================================================

/// The colors one cell is drawn in, after its attributes and the cursor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CellColors {
    /// Behind the glyph.
    background: Rgb,
    /// Whether the background is the theme's default, drawn as opaque as
    /// [`Theme::background_alpha`] says.
    default_background: bool,
    /// The glyph and its lines.
    foreground: Rgb,
}

/// The grid being laid out and where its output goes.
struct GridLayout<'a> {
    /// Whether blinking glyphs show now.
    blink_visible: bool,
    /// The family and size every run is drawn in, or the default for the
    /// window's font.
    run_style: RunStyle,
    /// The cursor, if it is drawn.
    cursor: Option<Cursor>,
    /// Where the output goes.
    draw: &'a mut DrawList,
    /// The source's highlights, in the order they are painted.
    highlights: &'a [CellHighlight],
    /// The size of a cell.
    metrics: CellMetrics,
    /// The top left corner of cell 0, 0.
    origin: Point,
    /// What the cells' colors and the cursor's resolve against.
    theme: &'a Theme,
    /// The curly underlines of the rows laid out, painted over every
    /// cell's background once all are laid out.
    waves: Vec<PathDraw>,
}

/// The row being laid out: the fill, the curly underline, and the ASCII
/// segment still growing.
#[derive(Default)]
struct RowState {
    /// The current curly underline: its color, left edge, and right edge.
    curl: Option<(Rgb, f32, f32)>,
    /// The current fill: its color, left edge, and right edge.
    fill: Option<(Rgba, f32, f32)>,
    /// The segment of ASCII glyphs being extended, and the column it
    /// continues at.
    segment: Option<(TextSegment, usize)>,
}

// ========================================================================
// CellColors Implementations
// ========================================================================

impl CellColors {
    /// The colors `style` draws in against `theme`: reverse video swaps
    /// them, and dim blends the foreground toward the background.
    fn of(style: &CellStyle, theme: &Theme) -> Self {
        let mut foreground = theme.foreground_of(style.foreground);
        let mut background = theme.background_of(style.background);
        if style.attributes.contains(Attributes::REVERSE) {
            (foreground, background) = (background, foreground);
        }
        if style.attributes.contains(Attributes::DIM) {
            foreground = foreground.lerp(background, DIM_BLEND);
        }
        let reversed = style.attributes.contains(Attributes::REVERSE);
        Self {
            background,
            default_background: style.background == CellColor::Default && !reversed,
            foreground,
        }
    }

    /// The colors under a block cursor of color `cursor`: it behind, and as the
    /// glyph color whichever of the cell's own background and foreground
    /// stands out more against it, so the glyph stays legible on a cell
    /// whose background is as light as the cursor (a search match).
    fn under_block_cursor(self, cursor: Rgb) -> Self {
        let foreground = match cursor.contrast(self.background) >= cursor.contrast(self.foreground)
        {
            true => self.background,
            false => self.foreground,
        };
        Self {
            background: cursor,
            default_background: false,
            foreground,
        }
    }
}

// ========================================================================
// GridLayout Implementations
// ========================================================================

impl GridLayout<'_> {
    /// Lays out row `row` of `source`.
    fn lay_out_row(&mut self, source: &(impl CellSource + ?Sized), row: usize) {
        let cols = source.cols();
        let top = self.origin.y + row as f32 * self.metrics.height;
        let mut state = RowState::default();
        let mut after_wide = false;
        let highlights: Vec<&CellHighlight> = self
            .highlights
            .iter()
            .filter(|highlight| highlight.row == row)
            .collect();
        for col in 0..cols {
            let Some(cell) = source.cell(row, col) else {
                self.flush_fill(&mut state, top);
                self.flush_curl(&mut state, top);
                self.flush_segment(&mut state);
                after_wide = false;
                continue;
            };
            let span = match cell.width {
                CellWidth::Continuation if after_wide => {
                    after_wide = false;
                    continue;
                }
                CellWidth::Continuation | CellWidth::Single => 1,
                CellWidth::Wide if col + 1 < cols => 2,
                CellWidth::Wide => 1,
            };
            after_wide = cell.width == CellWidth::Wide;
            let left = self.origin.x + col as f32 * self.metrics.width;
            let width = span as f32 * self.metrics.width;
            let mut colors = CellColors::of(&cell.style, self.theme);
            let tint = tinted(colors.background, &highlights, &(col..col + span));
            colors.default_background &= tint == colors.background;
            colors.background = tint;
            let cursor = self
                .cursor
                .filter(|cursor| (cursor.row, cursor.col) == (row, col));
            if let Some(Cursor {
                shape: CursorShape::Block,
                ..
            }) = cursor
            {
                colors = colors.under_block_cursor(self.theme.cursor);
            }
            let alpha = match colors.default_background {
                true => self.theme.background_alpha,
                false => u8::MAX,
            };
            let fill = colors.background.with_alpha(alpha);
            self.extend_fill(&mut state, fill, left, width, top);
            self.place_glyph(&mut state, cell, col, span, colors.foreground, top);
            let cell_rect = Rect::new(left, top, width, self.metrics.height);
            self.push_lines(cell, cell_rect, colors.foreground);
            match cell.style.underline {
                UnderlineStyle::Curly => {
                    let color = self.underline_color(cell, colors.foreground);
                    self.extend_curl(&mut state, color, left, width, top);
                }
                UnderlineStyle::Dashed
                | UnderlineStyle::Dotted
                | UnderlineStyle::Double
                | UnderlineStyle::None
                | UnderlineStyle::Single => self.flush_curl(&mut state, top),
            }
        }
        self.flush_fill(&mut state, top);
        self.flush_curl(&mut state, top);
        self.flush_segment(&mut state);
    }

    /// Grows the row's curly underline by the cell at `left`, or starts a
    /// new one when its color differs or a cell between went without.
    fn extend_curl(&mut self, state: &mut RowState, color: Rgb, left: f32, width: f32, top: f32) {
        match &mut state.curl {
            Some((curl_color, _, right)) if *curl_color == color && *right == left => {
                *right = left + width;
            }
            Some(_) | None => {
                self.flush_curl(state, top);
                state.curl = Some((color, left, left + width));
            }
        }
    }

    /// Keeps the row's pending curly underline, for painting once every
    /// background is laid out: a wave through points at most
    /// [`CURL_STEP`] apart, a period per cell, level at each cell's edges,
    /// kept inside the cells.
    fn flush_curl(&mut self, state: &mut RowState, top: f32) {
        let Some((color, left, right)) = state.curl.take() else {
            return;
        };
        let metrics = self.metrics;
        let thickness = (metrics.height * CURL_THICKNESS_SHARE).max(CURL_MIN_THICKNESS);
        let amplitude = (metrics.height * CURL_AMPLITUDE_SHARE).max(CURL_MIN_AMPLITUDE);
        let lowest = metrics.height - amplitude - thickness / 2.0;
        let centre = top + (metrics.underline_top() + thickness / 2.0).min(lowest);
        let samples = ((right - left) / CURL_STEP).ceil().max(1.0) as usize;
        let points: Vec<Point> = (0..=samples)
            .map(|index| {
                let x = left + (right - left) * index as f32 / samples as f32;
                let phase = (x - left) / metrics.width * TAU;
                Point::new(x, centre + amplitude * phase.sin())
            })
            .collect();
        let stroke = Stroke::new(thickness).with_join(LineJoin::Round);
        let wave = tessellate::stroke(
            &VectorPath::polyline(&points),
            &stroke,
            Paint::Solid(Rgba::from(color)),
            Transform::IDENTITY,
        );
        self.waves.extend(wave);
    }

    /// Paints the kept curly underlines over every fill laid out.
    fn push_waves(&mut self) {
        let fills_before = self.draw.fills.len();
        let waves = self.waves.drain(..).map(|wave| PathDraw {
            fills_before,
            ..wave
        });
        self.draw.paths.extend(waves);
    }

    /// Grows the row's fill by the cell at `left`, or starts a new one when
    /// its color differs.
    fn extend_fill(&mut self, state: &mut RowState, color: Rgba, left: f32, width: f32, top: f32) {
        match &mut state.fill {
            Some((fill_color, _, right)) if *fill_color == color && *right == left => {
                *right = left + width;
            }
            Some(_) | None => {
                self.flush_fill(state, top);
                state.fill = Some((color, left, left + width));
            }
        }
    }

    /// Emits the row's pending fill.
    fn flush_fill(&mut self, state: &mut RowState, top: f32) {
        if let Some((color, left, right)) = state.fill.take() {
            self.draw.fills.push(Quad::solid(
                Rect::new(left, top, right - left, self.metrics.height),
                color,
            ));
        }
    }

    /// Adds `cell`'s glyph at `col`, spanning `span` cells, to the text.
    fn place_glyph(
        &mut self,
        state: &mut RowState,
        cell: &Cell,
        col: usize,
        span: usize,
        color: Rgb,
        top: f32,
    ) {
        let blank = cell.ch == BLANK && cell.tail.is_none();
        let attributes = cell.style.attributes;
        let hidden = attributes.contains(Attributes::CONCEAL)
            || (attributes.contains(Attributes::BLINK) && !self.blink_visible);
        if blank || hidden || cell.width == CellWidth::Continuation {
            self.flush_segment(state);
            return;
        }
        let face = Face {
            bold: cell.style.attributes.contains(Attributes::BOLD),
            italic: cell.style.attributes.contains(Attributes::ITALIC),
        };
        let mut text = String::from(cell.ch);
        text.extend(cell.tail.as_deref());
        let origin = Point::new(self.origin.x + col as f32 * self.metrics.width, top);
        if BRAILLE.contains(&cell.ch) && cell.tail.is_none() {
            self.flush_segment(state);
            let width = span as f32 * self.metrics.width;
            let cell_rect = Rect::new(origin.x, top, width, self.metrics.height);
            self.push_braille(cell.ch, cell_rect, color);
            return;
        }
        if !is_one_cell_ascii(cell) {
            self.flush_segment(state);
            self.draw.text.push(TextSegment {
                align: SegmentAlign::Center(span as f32 * self.metrics.width),
                clip: None,
                origin,
                runs: vec![TextRun {
                    color,
                    face,
                    style: self.run_style.clone(),
                    text,
                }],
                wrap: None,
            });
            return;
        }
        if let Some((segment, next_col)) = &mut state.segment
            && *next_col == col
        {
            *next_col += 1;
            match segment.runs.last_mut() {
                Some(run) if run.color == color && run.face == face => run.text.push_str(&text),
                Some(_) | None => segment.runs.push(TextRun {
                    color,
                    face,
                    style: self.run_style.clone(),
                    text,
                }),
            }
            return;
        }
        self.flush_segment(state);
        let segment = TextSegment {
            align: SegmentAlign::Start,
            clip: None,
            origin,
            runs: vec![TextRun {
                color,
                face,
                style: self.run_style.clone(),
                text,
            }],
            wrap: None,
        };
        state.segment = Some((segment, col + 1));
    }

    /// Emits the row's pending ASCII segment.
    fn flush_segment(&mut self, state: &mut RowState) {
        if let Some((segment, _)) = state.segment.take() {
            self.draw.text.push(segment);
        }
    }

    /// Adds `cell`'s underline, in its underline color or else `color`,
    /// and strikethrough, in `color`, over `cell_rect`.
    fn push_lines(&mut self, cell: &Cell, cell_rect: Rect, color: Rgb) {
        let metrics = self.metrics;
        let thickness = metrics.line_thickness();
        let underline_color = self.underline_color(cell, color);
        let strikethrough = cell.style.attributes.contains(Attributes::STRIKETHROUGH);
        let mut push = |x: f32, y: f32, width: f32, color: Rgb| {
            self.draw.decorations.push(Quad::solid(
                Rect::new(x, cell_rect.y + y, width, thickness),
                Rgba::from(color),
            ));
        };
        let mut underline = |x: f32, y: f32, width: f32| push(x, y, width, underline_color);
        let cells = (cell_rect.width / metrics.width).round().max(1.0) as usize;
        let cell_lefts = (0..cells).map(|index| cell_rect.x + index as f32 * metrics.width);
        match cell.style.underline {
            // A wave the row draws across its run of cells.
            UnderlineStyle::Curly => {}
            UnderlineStyle::Dashed => {
                let dash = metrics.width * DASH_SHARE;
                for left in cell_lefts {
                    underline(
                        left + (metrics.width - dash) / 2.0,
                        metrics.underline_top(),
                        dash,
                    );
                }
            }
            UnderlineStyle::Dotted => {
                let right = cell_rect.x + cell_rect.width;
                let mut left = cell_rect.x;
                while left < right {
                    underline(left, metrics.underline_top(), thickness.min(right - left));
                    left += 2.0 * thickness;
                }
            }
            UnderlineStyle::Double => {
                let lower = metrics.height - thickness;
                underline(cell_rect.x, lower, cell_rect.width);
                underline(cell_rect.x, lower - 2.0 * thickness, cell_rect.width);
            }
            UnderlineStyle::None => {}
            UnderlineStyle::Single => {
                underline(cell_rect.x, metrics.underline_top(), cell_rect.width)
            }
        }
        if cell.style.attributes.contains(Attributes::OVERLINE) {
            push(cell_rect.x, 0.0, cell_rect.width, color);
        }
        if strikethrough {
            push(
                cell_rect.x,
                metrics.strikethrough_top(),
                cell_rect.width,
                color,
            );
        }
    }

    /// The color `cell`'s underline is drawn in: its underline color, or
    /// else `color`.
    fn underline_color(&self, cell: &Cell, color: Rgb) -> Rgb {
        match cell.style.underline_color {
            CellColor::Default => color,
            CellColor::Indexed(_) | CellColor::Rgb(_) => {
                self.theme.foreground_of(cell.style.underline_color)
            }
        }
    }

    /// Adds the set dots of the braille pattern `ch` over `cell_rect` in
    /// `color`, each a round dot centered in its slot of the matrix.
    fn push_braille(&mut self, ch: char, cell_rect: Rect, color: Rgb) {
        // In range by construction: `ch` is in the 256 character block.
        let bits = (u32::from(ch) - u32::from(*BRAILLE.start())) as u8;
        let (slot_width, slot_height) = (
            cell_rect.width / BRAILLE_COLS,
            cell_rect.height / BRAILLE_ROWS,
        );
        let diameter = slot_width.min(slot_height) * BRAILLE_DOT_SHARE;
        let set = BRAILLE_DOTS
            .iter()
            .enumerate()
            .filter(|(bit, _)| bits & (1 << bit) != 0);
        for (_, &(dot_col, dot_row)) in set {
            let left = cell_rect.x + (f32::from(dot_col) + 0.5) * slot_width - diameter / 2.0;
            let top = cell_rect.y + (f32::from(dot_row) + 0.5) * slot_height - diameter / 2.0;
            self.draw.decorations.push(Quad {
                radius: diameter / 2.0,
                ..Quad::solid(Rect::new(left, top, diameter, diameter), Rgba::from(color))
            });
        }
    }

    /// Adds the bar, underline, or hollow cursor, over everything else. A
    /// block cursor was drawn as its cell's colors.
    fn push_thin_cursor(&mut self, source: &(impl CellSource + ?Sized)) {
        let Some(cursor) = self.cursor else {
            return;
        };
        if cursor.row >= source.rows() || cursor.col >= source.cols() {
            return;
        }
        let metrics = self.metrics;
        let left = self.origin.x + cursor.col as f32 * metrics.width;
        let top = self.origin.y + cursor.row as f32 * metrics.height;
        let span = match source.cell(cursor.row, cursor.col).map(|cell| cell.width) {
            Some(CellWidth::Wide) if cursor.col + 1 < source.cols() => 2.0,
            Some(CellWidth::Continuation | CellWidth::Single | CellWidth::Wide) | None => 1.0,
        };
        let thickness = metrics.line_thickness();
        let rect = match cursor.shape {
            CursorShape::Bar => Rect::new(left, top, thickness * BAR_THICKNESSES, metrics.height),
            CursorShape::Block => return,
            CursorShape::Hollow => {
                let (width, height) = (span * metrics.width, metrics.height);
                let color = Rgba::from(self.theme.cursor);
                for edge in [
                    Rect::new(left, top, width, thickness),
                    Rect::new(left, top + height - thickness, width, thickness),
                    Rect::new(left, top, thickness, height),
                    Rect::new(left + width - thickness, top, thickness, height),
                ] {
                    self.draw.decorations.push(Quad::solid(edge, color));
                }
                return;
            }
            CursorShape::Underline => {
                let height = thickness * BAR_THICKNESSES;
                Rect::new(
                    left,
                    top + metrics.height - height,
                    span * metrics.width,
                    height,
                )
            }
        };
        self.draw
            .decorations
            .push(Quad::solid(rect, Rgba::from(self.theme.cursor)));
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Adds every cell of `source` to `draw`, cell 0, 0 at `origin`, each cell
/// `metrics` in size, its glyphs in `run_style`'s family and size (the
/// window font's where it sets none), colors resolved against `theme`.
pub(crate) fn lay_out_cells(
    source: &(impl CellSource + ?Sized),
    origin: Point,
    metrics: CellMetrics,
    run_style: RunStyle,
    theme: &Theme,
    draw: &mut DrawList,
) {
    let mut layout = GridLayout {
        blink_visible: source.blink_visible(),
        run_style,
        cursor: source.cursor(),
        draw,
        highlights: source.highlights(),
        metrics,
        origin,
        theme,
        waves: Vec::new(),
    };
    for row in 0..source.rows() {
        layout.lay_out_row(source, row);
    }
    layout.push_waves();
    layout.push_thin_cursor(source);
}

/// Whether `cell` is a single printable ASCII character in one column, which
/// the window font draws exactly one cell wide.
fn is_one_cell_ascii(cell: &Cell) -> bool {
    cell.width == CellWidth::Single && cell.tail.is_none() && cell.ch.is_ascii_graphic()
}

/// `background` under each of `highlights` that covers part of `cols`, in
/// order, each blended over it by its alpha.
fn tinted(background: Rgb, highlights: &[&CellHighlight], cols: &Range<usize>) -> Rgb {
    highlights
        .iter()
        .filter(|highlight| highlight.overlaps(cols))
        .fold(background, |under, highlight| {
            let Rgba { a, b, g, r } = highlight.color;
            under.lerp(Rgb::new(r, g, b), f32::from(a) / f32::from(u8::MAX))
        })
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::offscreen;
    use crate::base::cell::{CellBuffer, CellColor};

    const METRICS: CellMetrics = CellMetrics {
        baseline: 15.0,
        height: 20.0,
        width: 10.0,
    };

    fn styled(ch: char, style: CellStyle) -> Cell {
        Cell::new(ch, style)
    }

    fn plain(ch: char) -> Cell {
        styled(ch, CellStyle::default())
    }

    fn buffer_from(rows: &[&str]) -> CellBuffer {
        let cols = rows
            .iter()
            .map(|row| row.chars().count())
            .max()
            .unwrap_or(0);
        let mut buffer = CellBuffer::new(cols, rows.len());
        for (row, text) in rows.iter().enumerate() {
            for (col, ch) in text.chars().enumerate() {
                buffer.set(row, col, plain(ch));
            }
        }
        buffer
    }

    fn lay_out(source: &impl CellSource, theme: &Theme) -> DrawList {
        let mut draw = DrawList::default();
        lay_out_cells(
            source,
            Point::new(5.0, 7.0),
            METRICS,
            RunStyle::default(),
            theme,
            &mut draw,
        );
        draw
    }

    fn texts(draw: &DrawList) -> Vec<(f32, f32, String)> {
        draw.text
            .iter()
            .map(|segment| {
                let text = segment.runs.iter().map(|run| run.text.as_str()).collect();
                (segment.origin.x, segment.origin.y, text)
            })
            .collect()
    }

    #[test]
    fn test_ascii_runs_share_a_segment_and_blanks_split_them() {
        let draw = lay_out(&buffer_from(&["ab cd", "  x  "]), &Theme::dark());
        assert_eq!(
            texts(&draw),
            [
                (5.0, 7.0, "ab".to_owned()),
                (35.0, 7.0, "cd".to_owned()),
                (25.0, 27.0, "x".to_owned()),
            ]
        );
        assert!(
            draw.text
                .iter()
                .all(|segment| segment.align == SegmentAlign::Start)
        );
    }

    #[test]
    fn test_a_style_change_starts_a_run_but_not_a_segment() {
        let mut buffer = buffer_from(&["abc"]);
        let bold = CellStyle {
            attributes: Attributes::BOLD,
            ..CellStyle::default()
        };
        buffer.set(0, 1, styled('b', bold));
        let draw = lay_out(&buffer, &Theme::dark());
        assert_eq!(draw.text.len(), 1);
        let faces: Vec<bool> = draw.text[0].runs.iter().map(|run| run.face.bold).collect();
        assert_eq!(faces, [false, true, false]);
    }

    #[test]
    fn test_a_wide_cell_is_centered_over_two_columns_and_its_continuation_skipped() {
        let mut buffer = buffer_from(&["a  b"]);
        buffer.set(
            0,
            1,
            Cell {
                width: CellWidth::Wide,
                ..plain('世')
            },
        );
        buffer.set(
            0,
            2,
            Cell {
                width: CellWidth::Continuation,
                ..plain(BLANK)
            },
        );
        let draw = lay_out(&buffer, &Theme::dark());
        assert_eq!(
            texts(&draw),
            [
                (5.0, 7.0, "a".to_owned()),
                (15.0, 7.0, "世".to_owned()),
                (35.0, 7.0, "b".to_owned()),
            ]
        );
        assert_eq!(draw.text[1].align, SegmentAlign::Center(20.0));
        assert_eq!(
            draw.text[2].align,
            SegmentAlign::Start,
            "b is not glued to a"
        );
    }

    #[test]
    fn test_non_ascii_single_cells_get_their_own_segment() {
        let draw = lay_out(&buffer_from(&["a─b"]), &Theme::dark());
        assert_eq!(draw.text.len(), 3);
        assert_eq!(draw.text[1].align, SegmentAlign::Center(10.0));
    }

    #[test]
    fn test_equal_backgrounds_merge_into_one_fill_per_run() {
        let mut buffer = buffer_from(&["abcd"]);
        let red = CellStyle {
            background: CellColor::Indexed(1),
            ..CellStyle::default()
        };
        buffer.set(0, 2, styled('c', red));
        let theme = Theme::dark();
        let draw = lay_out(&buffer, &theme);
        let fills: Vec<(f32, f32, Rgba)> = draw
            .fills
            .iter()
            .map(|quad| (quad.rect.x, quad.rect.width, quad.color))
            .collect();
        assert_eq!(
            fills,
            [
                (5.0, 20.0, Rgba::from(theme.background)),
                (25.0, 10.0, Rgba::from(theme.palette.colors[1])),
                (35.0, 10.0, Rgba::from(theme.background)),
            ]
        );
    }

    #[test]
    fn test_reverse_swaps_and_dim_blends_toward_the_background() {
        let mut theme = Theme::dark();
        theme.background = Rgb::new(0, 0, 0);
        theme.foreground = Rgb::new(200, 100, 50);
        let reverse = CellStyle {
            attributes: Attributes::REVERSE,
            ..CellStyle::default()
        };
        assert_eq!(
            CellColors::of(&reverse, &theme),
            CellColors {
                background: theme.foreground,
                default_background: false,
                foreground: theme.background,
            }
        );
        let dim = CellStyle {
            attributes: Attributes::DIM,
            ..CellStyle::default()
        };
        assert_eq!(
            CellColors::of(&dim, &theme).foreground,
            Rgb::new(100, 50, 25)
        );
    }

    #[test]
    fn test_block_cursor_inverts_its_cell_and_draws_no_quad() {
        let mut buffer = buffer_from(&["ab"]);
        buffer.set_cursor(Some(Cursor::new(0, 1, CursorShape::Block)));
        let theme = Theme::dark();
        let draw = lay_out(&buffer, &theme);
        assert_eq!(draw.fills[1].color, Rgba::from(theme.cursor));
        assert_eq!(draw.text[0].runs[1].color, theme.background);
        assert!(draw.decorations.is_empty());
    }

    #[test]
    fn test_default_cells_and_the_cursor_follow_the_themes_own_colors() {
        let theme = Theme {
            background: Rgb::new(1, 2, 3),
            cursor: Rgb::new(4, 5, 6),
            foreground: Rgb::new(7, 8, 9),
            ..Theme::dark()
        };
        let mut buffer = buffer_from(&["ab"]);
        buffer.set_cursor(Some(Cursor::new(0, 1, CursorShape::Block)));
        let draw = lay_out(&buffer, &theme);
        assert_eq!(draw.fills[0].color, Rgba::from(theme.background));
        assert_eq!(draw.text[0].runs[0].color, theme.foreground);
        assert_eq!(draw.fills[1].color, Rgba::from(theme.cursor));
    }

    #[test]
    fn test_block_cursor_on_a_light_background_draws_the_glyph_in_its_foreground() {
        let theme = Theme::dark();
        let highlight = CellStyle {
            background: CellColor::Rgb(theme.cursor),
            foreground: CellColor::Rgb(theme.background),
            ..CellStyle::default()
        };
        let mut buffer = buffer_from(&["ab"]);
        buffer.set(0, 0, styled('a', highlight));
        buffer.set_cursor(Some(Cursor::new(0, 0, CursorShape::Block)));
        let draw = lay_out(&buffer, &theme);
        assert_eq!(draw.text[0].runs[0].color, theme.background);
    }

    #[test]
    fn test_bar_cursor_is_drawn_last_at_its_cell() {
        let mut buffer = buffer_from(&["ab"]);
        let underlined = CellStyle {
            underline: UnderlineStyle::Single,
            ..CellStyle::default()
        };
        buffer.set(0, 1, styled('b', underlined));
        buffer.set_cursor(Some(Cursor::new(0, 1, CursorShape::Bar)));
        let theme = Theme::dark();
        let draw = lay_out(&buffer, &theme);
        let cursor = draw.decorations.last().copied();
        assert_eq!(
            cursor.map(|quad| (quad.rect.x, quad.rect.height, quad.color)),
            Some((15.0, METRICS.height, Rgba::from(theme.cursor)))
        );
        assert_eq!(draw.decorations.len(), 2, "the underline, then the cursor");
    }

    #[test]
    fn test_a_cursor_outside_the_grid_is_not_drawn() {
        let mut buffer = buffer_from(&["ab"]);
        buffer.set_cursor(Some(Cursor::new(0, 5, CursorShape::Bar)));
        assert!(lay_out(&buffer, &Theme::dark()).decorations.is_empty());
    }

    #[test]
    fn test_underline_styles_stay_inside_their_cells() {
        for underline in [
            UnderlineStyle::Dashed,
            UnderlineStyle::Dotted,
            UnderlineStyle::Double,
            UnderlineStyle::Single,
        ] {
            let mut buffer = CellBuffer::new(2, 1);
            let style = CellStyle {
                underline,
                ..CellStyle::default()
            };
            buffer.set(0, 0, styled('a', style));
            let draw = lay_out(&buffer, &Theme::dark());
            assert!(
                !draw.decorations.is_empty(),
                "{underline:?} draws something"
            );
            for quad in &draw.decorations {
                let rect = quad.rect;
                assert!(
                    rect.x >= 5.0 && rect.x + rect.width <= 15.0,
                    "{underline:?}"
                );
                assert!(
                    rect.y >= 7.0 && rect.y + rect.height <= 27.0,
                    "{underline:?}"
                );
            }
        }
    }

    #[test]
    fn test_a_curly_underline_is_one_wave_per_run_of_one_color_inside_its_row() {
        let theme = Theme::dark();
        let mut buffer = CellBuffer::new(5, 2);
        let curly = |underline_color| CellStyle {
            underline: UnderlineStyle::Curly,
            underline_color,
            ..CellStyle::default()
        };
        for col in 0..3 {
            buffer.set(1, col, styled('a', curly(CellColor::Default)));
        }
        buffer.set(1, 4, styled('a', curly(CellColor::Indexed(1))));
        let draw = lay_out(&buffer, &theme);
        assert!(draw.decorations.is_empty(), "no bars");
        let waves: Vec<(Rect, usize)> = draw
            .paths
            .iter()
            .map(|path| (path.bounds, path.fills_before))
            .collect();
        assert_eq!(waves.len(), 2, "one per run: {waves:?}");
        let fringe = 1.0;
        for (bounds, fills_before) in &waves {
            assert_eq!(*fills_before, draw.fills.len(), "over every background");
            assert!(bounds.y >= 27.0 - fringe && bounds.y + bounds.height <= 47.0 + fringe);
            let flat = CURL_MIN_THICKNESS + 2.0 * fringe;
            assert!(bounds.height > flat, "it swings: {bounds:?}");
        }
        let first = waves[0].0;
        assert!(
            first.x < 6.0 && first.x + first.width > 34.0,
            "three cells: {first:?}"
        );
        assert_eq!(
            draw.paths[1].paint,
            Paint::Solid(Rgba::from(theme.palette.colors[1])),
            "in its underline color"
        );
    }

    #[test]
    fn test_an_underline_color_paints_the_underline_but_not_the_strikethrough() {
        let theme = Theme::dark();
        let mut buffer = CellBuffer::new(1, 1);
        let style = CellStyle {
            attributes: Attributes::STRIKETHROUGH,
            underline: UnderlineStyle::Single,
            underline_color: CellColor::Indexed(1),
            ..CellStyle::default()
        };
        buffer.set(0, 0, styled('a', style));
        let draw = lay_out(&buffer, &theme);
        let colors: Vec<Rgba> = draw.decorations.iter().map(|quad| quad.color).collect();
        assert_eq!(
            colors,
            [
                Rgba::from(theme.palette.colors[1]),
                Rgba::from(theme.foreground)
            ]
        );
    }

    /// A grid whose blinking glyphs are in their hidden phase.
    struct Blinked(CellBuffer);

    impl CellSource for Blinked {
        fn cols(&self) -> usize {
            self.0.cols()
        }
        fn rows(&self) -> usize {
            self.0.rows()
        }
        fn cell(&self, row: usize, col: usize) -> Option<&Cell> {
            self.0.cell(row, col)
        }
        fn cursor(&self) -> Option<Cursor> {
            self.0.cursor()
        }
        fn blink_visible(&self) -> bool {
            false
        }
    }

    #[test]
    fn test_concealed_and_blinked_out_glyphs_leave_their_cells_and_an_overline_tops_its_cell() {
        let theme = Theme::dark();
        let mut buffer = CellBuffer::new(3, 1);
        let with = |attributes| CellStyle {
            attributes,
            ..CellStyle::default()
        };
        buffer.set(0, 0, styled('a', with(Attributes::CONCEAL)));
        buffer.set(0, 1, styled('b', with(Attributes::BLINK)));
        buffer.set(0, 2, styled('c', with(Attributes::OVERLINE)));
        let shown = lay_out(&buffer, &theme);
        assert_eq!(texts(&shown), [(15.0, 7.0, "bc".to_owned())], "blink on");
        let overline = shown.decorations[0].rect;
        assert_eq!((overline.x, overline.y), (25.0, 7.0), "at the top edge");
        let blinked = lay_out(&Blinked(buffer), &theme);
        assert_eq!(texts(&blinked), [(25.0, 7.0, "c".to_owned())], "blink off");
    }

    /// A grid with highlights over it.
    struct Highlighted(CellBuffer, Vec<CellHighlight>);

    impl CellSource for Highlighted {
        fn cols(&self) -> usize {
            self.0.cols()
        }
        fn rows(&self) -> usize {
            self.0.rows()
        }
        fn cell(&self, row: usize, col: usize) -> Option<&Cell> {
            self.0.cell(row, col)
        }
        fn cursor(&self) -> Option<Cursor> {
            self.0.cursor()
        }
        fn highlights(&self) -> &[CellHighlight] {
            &self.1
        }
    }

    #[test]
    fn test_only_the_default_background_takes_the_themes_alpha() {
        let mut theme = Theme::dark();
        theme.background_alpha = 100;
        let mut buffer = buffer_from(&["ab"]);
        let red = CellStyle {
            background: CellColor::Indexed(1),
            ..CellStyle::default()
        };
        buffer.set(0, 1, styled('b', red));
        let draw = lay_out(&buffer, &theme);
        let alphas: Vec<u8> = draw.fills.iter().map(|quad| quad.color.a).collect();
        assert_eq!(
            alphas,
            [100, u8::MAX],
            "a cell of its own color stays solid"
        );
    }

    #[test]
    fn test_highlights_tint_only_their_cells_and_later_ones_paint_over_earlier() {
        let mut theme = Theme::dark();
        theme.background = Rgb::new(0, 0, 0);
        let half_red = Rgba::new(200, 0, 0, 128);
        let blue = Rgba::new(0, 0, 200, u8::MAX);
        let source = Highlighted(
            buffer_from(&["abcd", "efgh"]),
            vec![
                CellHighlight::new(0, 1..3, half_red),
                CellHighlight::new(0, 2..4, blue),
            ],
        );
        let draw = lay_out(&source, &theme);
        let fills: Vec<(f32, f32, Rgba)> = draw
            .fills
            .iter()
            .map(|quad| (quad.rect.x, quad.rect.width, quad.color))
            .collect();
        assert_eq!(
            fills,
            [
                (5.0, 10.0, Rgba::rgb(0, 0, 0)),
                (15.0, 10.0, Rgba::rgb(100, 0, 0)),
                (25.0, 20.0, Rgba::rgb(0, 0, 200)),
                (5.0, 40.0, Rgba::rgb(0, 0, 0)),
            ],
            "half the red over black, the opaque blue over both, row 1 untouched"
        );
    }

    #[test]
    fn test_a_block_cursor_stays_on_top_of_a_highlight_and_a_wide_cell_takes_either_half() {
        let theme = Theme::dark();
        let mut buffer = buffer_from(&["a  "]);
        buffer.set(
            0,
            1,
            Cell {
                width: CellWidth::Wide,
                ..plain('世')
            },
        );
        buffer.set(
            0,
            2,
            Cell {
                width: CellWidth::Continuation,
                ..plain(BLANK)
            },
        );
        buffer.set_cursor(Some(Cursor::new(0, 0, CursorShape::Block)));
        let green = Rgba::rgb(0, 200, 0);
        let source = Highlighted(
            buffer,
            vec![
                CellHighlight::new(0, 0..1, green),
                CellHighlight::new(0, 2..3, green),
            ],
        );
        let draw = lay_out(&source, &theme);
        assert_eq!(draw.fills[0].color, Rgba::from(theme.cursor), "the cursor");
        assert_eq!(
            (draw.fills[1].rect.width, draw.fills[1].color),
            (20.0, green),
            "the whole wide cell, from its right half"
        );
    }

    #[test]
    fn test_braille_draws_round_dots_on_the_cells_matrix_and_no_glyph() {
        let draw = lay_out(&buffer_from(&["a\u{2801}\u{28C0}\u{2800}"]), &Theme::dark());
        assert_eq!(
            texts(&draw),
            [(5.0, 7.0, "a".to_owned())],
            "no braille glyph"
        );
        let dots: Vec<(f32, f32, f32)> = draw
            .decorations
            .iter()
            .map(|quad| (quad.rect.x, quad.rect.y, quad.radius * 2.0))
            .collect();
        // Slots are 5 by 5, so a dot is 4.5 across, a quarter pixel in.
        assert_eq!(
            dots,
            [(15.25, 7.25, 4.5), (25.25, 22.25, 4.5), (30.25, 22.25, 4.5),],
            "dot 1 top left; dots 7 and 8 along the bottom; the blank pattern none"
        );
    }

    #[test]
    fn test_a_hollow_cursor_outlines_its_cell_and_leaves_the_colors_inside() {
        let theme = Theme::dark();
        let mut buffer = buffer_from(&["ab"]);
        buffer.set_cursor(Some(Cursor {
            col: 1,
            row: 0,
            shape: CursorShape::Hollow,
        }));
        let draw = lay_out(&buffer, &theme);
        let edges: Vec<Rect> = draw.decorations.iter().map(|quad| quad.rect).collect();
        assert_eq!(edges.len(), 4, "four edges");
        let thickness = METRICS.line_thickness();
        assert!(
            edges.contains(&Rect::new(15.0, 7.0, 10.0, thickness)),
            "{edges:?}"
        );
        assert!(edges.contains(&Rect::new(25.0 - thickness, 7.0, thickness, 20.0)));
        assert!(
            draw.fills
                .iter()
                .all(|fill| fill.color == Rgba::from(theme.background)),
            "no cell takes the cursor's color"
        );
    }

    #[test]
    fn test_double_underline_draws_two_separate_lines() {
        let mut buffer = CellBuffer::new(1, 1);
        let style = CellStyle {
            underline: UnderlineStyle::Double,
            ..CellStyle::default()
        };
        buffer.set(0, 0, styled('a', style));
        let draw = lay_out(&buffer, &Theme::dark());
        let [upper, lower] = [draw.decorations[1].rect, draw.decorations[0].rect];
        assert!(upper.y + upper.height < lower.y, "a gap between the lines");
    }

    #[test]
    fn test_cells_paint_their_colors_and_the_block_cursor_inverts_its_cell() {
        let theme = Theme::dark();
        let (yellow, blue) = (Rgb::new(250, 220, 40), Rgb::new(20, 40, 160));
        let style = CellStyle {
            background: CellColor::Rgb(blue),
            foreground: CellColor::Rgb(yellow),
            ..CellStyle::default()
        };
        let mut buffer = CellBuffer::new(3, 1);
        buffer.set(0, 0, styled('M', style));
        buffer.set(0, 1, styled('W', style));
        buffer.set_cursor(Some(Cursor::new(0, 1, CursorShape::Block)));
        let Some(snapshot) = offscreen::render(120, 40, |frame| {
            frame.clear(Rgba::from(theme.background));
            frame.draw_cells(&buffer, Point::new(0.0, 0.0), &theme);
        }) else {
            return;
        };
        let metrics = snapshot.metrics();
        let cell = |col: f32| Rect::new(col * metrics.width, 0.0, metrics.width, metrics.height);
        snapshot.assert_pixel(0.0, 0.0, blue, "the styled cell's background");
        assert!(snapshot.contains_color(cell(0.0), yellow), "the glyph");
        let cursor = cell(1.0);
        snapshot.assert_pixel(cursor.x, 0.0, theme.cursor, "the block cursor");
        assert!(
            snapshot.contains_color(cursor, blue),
            "the glyph under the cursor, in its cell's background"
        );
        assert!(
            !snapshot.contains_color(cursor, yellow),
            "no foreground left"
        );
        snapshot.assert_pixel(cell(2.0).x, 0.0, theme.background, "a blank cell");
    }

    #[test]
    fn test_a_bar_cursor_covers_only_the_left_of_its_cell() {
        let theme = Theme::dark();
        let mut buffer = CellBuffer::new(2, 1);
        buffer.set_cursor(Some(Cursor::new(0, 1, CursorShape::Bar)));
        let Some(snapshot) = offscreen::render(80, 40, |frame| {
            frame.clear(Rgba::from(theme.background));
            frame.draw_cells(&buffer, Point::new(0.0, 0.0), &theme);
        }) else {
            return;
        };
        let metrics = snapshot.metrics();
        let (left, middle) = (metrics.width, metrics.height / 2.0);
        snapshot.assert_pixel(left, middle, theme.cursor, "the bar");
        let past = left + metrics.width - 1.0;
        snapshot.assert_pixel(past, middle, theme.background, "the cell's right");
    }
}
