//! [`RubberBand`]: a rectangle dragged from empty space that selects every
//! row or cell it touches, on top of the selection it started with when
//! Ctrl was held, shared by the list widgets that draw one (the list view,
//! the icon grid). Where the band runs, what it touches, and where the
//! selection is kept stay with each widget; this keeps the rows under it
//! and draws it.

use super::super::app::frame::Frame;
use super::super::base::geometry::{Point, Rect};
use super::super::base::theme::Theme;
use super::super::render::icon::{logical, outline};

// ========================================================================
// Constants
// ========================================================================

/// The thickness of a rubber band's outline, in logical pixels.
const BAND_STROKE: f32 = 1.0;

/// The opacity of a rubber band's fill, out of 255.
const BAND_ALPHA: u8 = 48;

// ========================================================================
// Data Structures
// ========================================================================

/// A rubber band being drawn over a list's rows or a grid's cells.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RubberBand {
    /// The rows kept under it: those selected when it started with Ctrl
    /// held, none without.
    base: Vec<usize>,
    /// Where it started, in whatever space the widget keeps it in (down in
    /// content pixels, so it stays on its row as the list scrolls).
    start: Point,
}

// ========================================================================
// RubberBand Implementations
// ========================================================================

impl RubberBand {
    /// A band starting at `start`, selecting on top of the rows `base`
    /// (the selection when Ctrl is held, nothing otherwise).
    pub(crate) fn new(start: Point, base: impl IntoIterator<Item = usize>) -> Self {
        Self {
            base: base.into_iter().collect(),
            start,
        }
    }

    /// Where the band started.
    pub(crate) fn start(&self) -> Point {
        self.start
    }

    /// The selection while the band touches the rows `touched`: those on
    /// top of the rows it started with.
    pub(crate) fn selection(
        &self,
        touched: impl IntoIterator<Item = usize>,
    ) -> impl Iterator<Item = usize> {
        self.base.iter().copied().chain(touched)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The rectangle with corners `a` and `b`, whichever way the band was
/// dragged.
pub(crate) fn band_rect(a: Point, b: Point) -> Rect {
    Rect::new(
        a.x.min(b.x),
        a.y.min(b.y),
        (a.x - b.x).abs(),
        (a.y - b.y).abs(),
    )
}

/// Draws a rubber band over `rect` (on screen): a shade of the accent color
/// inside an accent outline.
pub(crate) fn draw_band(frame: &mut Frame, rect: Rect, theme: &Theme) {
    let accent = theme.accent;
    frame.fill_rect(rect, accent.with_alpha(BAND_ALPHA));
    for edge in outline(rect, logical(BAND_STROKE, frame.scale_factor())) {
        frame.fill_rect_over_text(edge, accent);
    }
}
