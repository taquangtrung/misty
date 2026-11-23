//! Telling a double click from two single ones: [`is_double_click`], and
//! counting presses in a row: [`ClickCounter`].

use std::time::{Duration, Instant};

use super::super::base::geometry::Point;
use super::event::PointerButton;

// ========================================================================
// Constants
// ========================================================================

/// The longest gap between two presses that still makes a double click.
const DOUBLE_CLICK_TIME: Duration = Duration::from_millis(500);

/// How far apart, in logical pixels, two presses of a double click may be.
const DOUBLE_CLICK_SLOP: f32 = 4.0;

// ========================================================================
// Data Structures
// ========================================================================

/// Counts the presses of one button in a row, each soon enough after and
/// close enough to the one before it to make a double click with it: 1
/// for a single press, 2 for a double click, 3 for a triple, and so on.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ClickCounter {
    /// The presses in a row so far, 0 before the first.
    count: u8,
    /// The button, time, and place of the last press.
    last: Option<(PointerButton, Instant, Point)>,
}

// ========================================================================
// ClickCounter Implementations
// ========================================================================

impl ClickCounter {
    /// Counts a press of `button` at `point` at `now`, on a display scaled
    /// by `scale_factor`, returning how many presses in a row it makes.
    pub(crate) fn press(
        &mut self,
        button: PointerButton,
        now: Instant,
        point: Point,
        scale_factor: f64,
    ) -> u8 {
        let previous = self
            .last
            .filter(|(last_button, ..)| *last_button == button)
            .map(|(_, at, place)| (at, place));
        self.count = match is_double_click(previous, now, point, scale_factor) {
            true => self.count.saturating_add(1),
            false => 1,
        };
        self.last = Some((button, now, point));
        self.count
    }

    /// The presses in a row the last press made, 0 before the first.
    pub(crate) fn count(&self) -> u8 {
        self.count
    }
}

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

    #[test]
    fn test_presses_in_a_row_count_up_and_start_over_when_slow_far_or_another_button() {
        let mut counter = ClickCounter::default();
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        let here = Point::new(50.0, 50.0);
        let left = PointerButton::Left;
        assert_eq!(counter.count(), 0);
        assert_eq!(counter.press(left, at(0), here, 1.0), 1);
        assert_eq!(counter.press(left, at(200), here, 1.0), 2);
        assert_eq!(counter.press(left, at(400), Point::new(52.0, 51.0), 1.0), 3);
        assert_eq!(counter.count(), 3);
        assert_eq!(counter.press(left, at(1200), here, 1.0), 1, "too slow");
        assert_eq!(
            counter.press(left, at(1300), Point::new(70.0, 50.0), 1.0),
            1
        );
        assert_eq!(
            counter.press(PointerButton::Right, at(1400), Point::new(70.0, 50.0), 1.0),
            1
        );
        assert_eq!(
            counter.press(left, at(1500), Point::new(70.0, 50.0), 1.0),
            1
        );
    }
}
