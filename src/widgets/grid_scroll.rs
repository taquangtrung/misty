//! [`GridScroll`]: the scroll of a grid of cells too large for its area,
//! as a spreadsheet's, with its first rows and columns frozen: they stay
//! in sight while the rest scrolls under them both ways.
//!
//! The app keeps the cells and their sizes. Each frame it lends
//! [`GridScroll::lend`] the width of every column and the height of every
//! row, in physical pixels, and draws the loan; the scroll draws its
//! scrollbars and returns a
//! [`GridLayout`]: the columns and rows in sight with where each is drawn,
//! and the four regions to clip cells to. A cell in a frozen row and a
//! frozen column is cut to [`GridLayout::corner`], one in a frozen row
//! only to [`GridLayout::top`], in a frozen column only to
//! [`GridLayout::left`], and the rest to [`GridLayout::body`];
//! [`GridLayout::clip`] picks it.
//!
//! Input handed to the loan goes to its [`ScrollView`]: the wheel, the
//! scrollbars, and keys while focused. Finding the cells in sight costs a pass over the sizes,
//! about a millisecond for a million rows.

use std::ops::Range;
use std::time::Instant;

use super::super::app::context::EventCx;
use super::super::app::frame::Frame;
use super::super::app::widget::{Lent, Widget};
use super::super::base::clock;
use super::super::base::geometry::{Point, Rect};
use super::super::base::theme::Theme;
use super::super::input::event::Input;
use super::scroll_view::{ScrollView, ScrollViewResponse};

// ========================================================================
// Data Structures
// ========================================================================

/// The scroll of a grid with frozen leading rows and columns.
#[derive(Clone, Debug, Default)]
pub struct GridScroll {
    /// How many leading columns stay in sight.
    frozen_cols: usize,
    /// How many leading rows stay in sight.
    frozen_rows: usize,
    /// The scroll of the body.
    scroll: ScrollView,
}

/// Where a [`GridScroll`] shows a grid this frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GridLayout {
    /// Where the scrolling cells show.
    pub body: Rect,
    /// The columns in sight, frozen ones first, left to right.
    pub columns: Vec<GridLine>,
    /// Where the cells in both a frozen row and a frozen column show.
    pub corner: Rect,
    /// Where the frozen columns' scrolling cells show.
    pub left: Rect,
    /// The rows in sight, frozen ones first, top to bottom.
    pub rows: Vec<GridLine>,
    /// Where the frozen rows' scrolling cells show.
    pub top: Rect,
    /// How many leading columns are frozen.
    frozen_cols: usize,
    /// How many leading rows are frozen.
    frozen_rows: usize,
}

/// A column or row in sight: which, and where it is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridLine {
    /// Which column or row, counting from 0.
    pub index: usize,
    /// Its width or height, in physical pixels.
    pub size: f32,
    /// Its left or top edge, in physical pixels in the window.
    pub start: f32,
}

// ========================================================================
// GridScroll Implementations
// ========================================================================

impl GridScroll {
    /// A scroll of a grid with its first `frozen_rows` rows and
    /// `frozen_cols` columns frozen, at the top left.
    pub fn new(frozen_rows: usize, frozen_cols: usize) -> Self {
        Self {
            frozen_cols,
            frozen_rows,
            scroll: ScrollView::new(),
        }
    }

    /// How far the body is scrolled, in physical pixels.
    pub fn offset(&self) -> Point {
        self.scroll.offset()
    }

    /// Puts the body at `offset` at once.
    pub fn scroll_to(&mut self, offset: Point) {
        self.scroll.scroll_to(offset);
    }

    /// The scroll lent `widths`, the width of every column, and
    /// `heights`, the height of every row, in physical pixels, as a
    /// [`Widget`] to hand inputs and draw.
    pub fn lend<'a>(
        &'a mut self,
        widths: &'a [f32],
        heights: &'a [f32],
    ) -> Lent<'a, Self, (&'a [f32], &'a [f32])> {
        Lent::new(self, (widths, heights))
    }

    /// Draws the scrollbars over `area` for a grid of columns `widths`
    /// wide and rows `heights` tall, and returns where its cells in sight
    /// go.
    fn draw_grid(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        theme: &Theme,
        (widths, heights): (&[f32], &[f32]),
    ) -> GridLayout {
        let frozen_cols = self.frozen_cols.min(widths.len());
        let frozen_rows = self.frozen_rows.min(heights.len());
        let frozen_width: f32 = widths[..frozen_cols].iter().sum();
        let frozen_height: f32 = heights[..frozen_rows].iter().sum();
        let rest_width: f32 = widths[frozen_cols..].iter().sum();
        let rest_height: f32 = heights[frozen_rows..].iter().sum();
        self.scroll.set_content_size(rest_width, rest_height);
        let scrolled = Rect::new(
            area.x + frozen_width,
            area.y + frozen_height,
            (area.width - frozen_width).max(0.0),
            (area.height - frozen_height).max(0.0),
        );
        let body = self.scroll.draw_at(frame, scrolled, theme, clock::now());
        let offset = self.scroll.offset();
        let columns = lines_in_sight(
            widths,
            frozen_cols,
            area.x,
            body.x - offset.x,
            body.x..body.x + body.width,
        );
        let rows = lines_in_sight(
            heights,
            frozen_rows,
            area.y,
            body.y - offset.y,
            body.y..body.y + body.height,
        );
        GridLayout {
            body,
            columns,
            corner: Rect::new(
                area.x,
                area.y,
                frozen_width.min(area.width),
                frozen_height.min(area.height),
            ),
            frozen_cols,
            frozen_rows,
            left: Rect::new(area.x, body.y, frozen_width.min(area.width), body.height),
            rows,
            top: Rect::new(body.x, area.y, body.width, frozen_height.min(area.height)),
        }
    }
}

// ========================================================================
// Widget Implementations
// ========================================================================

impl Widget for Lent<'_, GridScroll, (&[f32], &[f32])> {
    type Output = GridLayout;
    type Response = ScrollViewResponse;

    /// Reacts to `input` as its [`ScrollView`] does.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> ScrollViewResponse {
        self.widget.scroll.handle(input, cx)
    }

    /// Draws the scrollbars over `area` for the grid lent, and returns
    /// where its cells in sight go.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) -> GridLayout {
        self.widget.draw_grid(frame, area, theme, self.model)
    }

    /// Tells it whether it has the keyboard; it takes keys only while it
    /// does.
    fn set_focused(&mut self, focused: bool) {
        self.widget.scroll.set_focused(focused);
    }

    /// Whether the app last said it has the keyboard.
    fn is_focused(&self) -> bool {
        self.widget.scroll.is_focused()
    }

    /// Asks for the next frame of a scroll glide under way.
    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        self.widget.scroll.about_to_wait(cx)
    }
}

// ========================================================================
// GridLayout Implementations
// ========================================================================

impl GridLayout {
    /// The region the cell at `row` and `col` is cut to.
    pub fn clip(&self, row: usize, col: usize) -> Rect {
        match (row < self.frozen_rows, col < self.frozen_cols) {
            (true, true) => self.corner,
            (true, false) => self.top,
            (false, true) => self.left,
            (false, false) => self.body,
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The lines of `sizes` in sight: the first `frozen` from `frozen_start`,
/// then the rest from `scrolled_start` that touch `sight`.
fn lines_in_sight(
    sizes: &[f32],
    frozen: usize,
    frozen_start: f32,
    scrolled_start: f32,
    sight: Range<f32>,
) -> Vec<GridLine> {
    let mut lines = Vec::new();
    let mut start = frozen_start;
    for (index, &size) in sizes[..frozen].iter().enumerate() {
        lines.push(GridLine { index, size, start });
        start += size;
    }
    let mut start = scrolled_start;
    for (index, &size) in sizes.iter().enumerate().skip(frozen) {
        if start >= sight.end {
            break;
        }
        if start + size > sight.start {
            lines.push(GridLine { index, size, start });
        }
        start += size;
    }
    lines
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestWindow;

    #[test]
    fn test_frozen_lines_stay_and_only_the_scrolled_lines_in_sight_come() {
        let window = TestWindow::new(400, 300);
        let mut grid = GridScroll::new(1, 1);
        let widths = vec![50.0; 100];
        let heights = vec![20.0; 10_000];
        let area = Rect::new(0.0, 0.0, 300.0, 200.0);
        let mut frame = window.frame();
        grid.lend(&widths, &heights)
            .draw(&mut frame, area, &Theme::dark());
        grid.scroll_to(Point::new(125.0, 2000.0));
        let layout = grid
            .lend(&widths, &heights)
            .draw(&mut frame, area, &Theme::dark());
        assert_eq!(
            (layout.columns[0].index, layout.columns[0].start),
            (0, 0.0),
            "the frozen column"
        );
        assert_eq!(
            (layout.rows[0].index, layout.rows[0].start),
            (0, 0.0),
            "the frozen row"
        );
        let first_col = layout.columns[1];
        assert_eq!(
            first_col.index, 3,
            "125 pixels scrolled past columns 1 and 2, into 3"
        );
        assert!(
            first_col.start < layout.body.x && first_col.start + first_col.size > layout.body.x
        );
        let first_row = layout.rows[1];
        assert_eq!(first_row.index, 101, "2000 pixels down from row 1");
        assert!(layout.rows.len() < 20, "{} rows", layout.rows.len());
        assert_eq!(layout.clip(0, 0), layout.corner);
        assert_eq!(layout.clip(0, 5), layout.top);
        assert_eq!(layout.clip(7, 0), layout.left);
        assert_eq!(layout.clip(7, 5), layout.body);
    }
}
