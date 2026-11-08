//! Telling a double click from two single ones: [`is_double_click`].

use std::time::{Duration, Instant};

use super::super::base::geometry::Point;

// ========================================================================
// Constants
// ========================================================================

/// The longest gap between two presses that still makes a double click.
const DOUBLE_CLICK_TIME: Duration = Duration::from_millis(500);

/// How far apart, in logical pixels, two presses of a double click may be.
const DOUBLE_CLICK_SLOP: f32 = 4.0;

// ========================================================================
// Functions
// ========================================================================

/// Whether a press at `point` at `now` completes a double click begun by
/// `last`: soon enough after it and close enough to it, on a display scaled
/// by `scale_factor`.
pub(crate) fn is_double_click(
    last: Option<(Instant, Point)>,
    now: Instant,
    point: Point,
    scale_factor: f64,
) -> bool {
    let Some((at, first)) = last else {
        return false;
    };
    let slop = DOUBLE_CLICK_SLOP * scale_factor as f32;
    now.saturating_duration_since(at) <= DOUBLE_CLICK_TIME
        && (point.x - first.x).abs() <= slop
        && (point.y - first.y).abs() <= slop
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_double_click_must_be_quick_and_close() {
        let start = Instant::now();
        let first = Some((start, Point::new(100.0, 10.0)));
        let soon = start + Duration::from_millis(300);
        assert!(is_double_click(first, soon, Point::new(103.0, 12.0), 1.0));
        assert!(!is_double_click(first, soon, Point::new(105.0, 10.0), 1.0));
        assert!(is_double_click(first, soon, Point::new(107.0, 10.0), 2.0));
        let late = start + Duration::from_millis(600);
        assert!(!is_double_click(first, late, Point::new(100.0, 10.0), 1.0));
        assert!(!is_double_click(None, soon, Point::new(100.0, 10.0), 1.0));
    }
}
