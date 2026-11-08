//! Row glides: [`RowGlide`] eases a view of whole rows (a list, the quick
//! pick's results, the settings page's rows) from where it was drawn to
//! its new scroll, so a key that scrolls by rows glides there instead of
//! jumping.

use std::ops::Range;
use std::time::Instant;

use super::super::app::context::EventCx;
use super::super::base::scroll::SmoothScroll;

// ========================================================================
// Data Structures
// ========================================================================

/// The scroll of a view of rows as drawn, easing toward the view's scroll.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct RowGlide {
    /// The row at the top edge of the last frame, with the share of it
    /// scrolled out of sight; `None` before the first frame or after the
    /// rows changed, so the next frame jumps.
    drawn: Option<f32>,
    /// The easing from the drawn scroll to the view's.
    motion: SmoothScroll,
}

// ========================================================================
// RowGlide Implementations
// ========================================================================

impl RowGlide {
    /// Makes scrolls jump (`true`) rather than glide, for users who asked
    /// their system for reduced motion.
    pub(crate) fn set_reduced_motion(&mut self, reduced_motion: bool) {
        self.motion.set_reduced_motion(reduced_motion);
    }

    /// Makes the next frame jump to its scroll, for rows that are no
    /// longer the ones drawn (a new filter, another section).
    pub(crate) fn jump_next_frame(&mut self) {
        self.drawn = None;
    }

    /// The scroll the last frame was drawn at; `None` before the first.
    pub(crate) fn drawn(&self) -> Option<f32> {
        self.drawn
    }

    /// Starts gliding toward the row `target` when it changed, `rows`
    /// being how many rows show, and returns (and keeps) the scroll to
    /// draw at `now`. A glide never covers more than a screen: a longer
    /// jump starts a screen short of its target. The first frame, and the
    /// first after [`RowGlide::jump_next_frame`], does not glide.
    pub(crate) fn follow(&mut self, target: usize, rows: usize, now: Instant) -> f32 {
        let target = target as f32;
        if self.drawn.is_none() {
            self.motion.jump_to(target);
        }
        if target != self.motion.target() {
            let screen = rows.max(1) as f32;
            let distance = target - self.motion.position(now);
            if distance.abs() > screen {
                self.motion.jump_to(target - screen * distance.signum());
            }
            self.motion.scroll_to(target, now);
        }
        let offset = self.motion.position(now);
        self.drawn = Some(offset);
        offset
    }

    /// Asks for the next frame of a glide still under way, returning when
    /// it is due; `None` at rest.
    pub(crate) fn about_to_wait(&self, cx: &mut EventCx<'_>) -> Option<Instant> {
        // The last frame drawn may have been a step short of the end.
        if self.drawn? == self.motion.target() {
            return None;
        }
        let now = Instant::now();
        let next = self.motion.next_frame(now).unwrap_or(now);
        cx.request_redraw_at(next);
        Some(next)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The rows of `count` that show, wholly or in part, when `rows` rows fit
/// and the top edge is at `offset`.
pub(crate) fn rows_in_sight(offset: f32, rows: usize, count: usize) -> Range<usize> {
    let first = offset.floor().max(0.0) as usize;
    let end = ((offset + rows as f32).ceil().max(0.0) as usize).min(count);
    first..end.max(first)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn test_a_scroll_glides_at_most_a_screen_and_not_on_the_first_frame() {
        let mut glide = RowGlide::default();
        let start = Instant::now();
        assert_eq!(
            glide.follow(5, 10, start),
            5.0,
            "no glide before the first frame"
        );
        let now = glide.follow(505, 10, start);
        assert_eq!(now, 495.0, "a long jump starts a screen short");
        let later = glide.follow(505, 10, start + Duration::from_millis(60));
        assert!(later > 495.0 && later < 505.0, "{later}");
        glide.set_reduced_motion(true);
        assert_eq!(glide.follow(505, 10, start), 505.0);
    }

    #[test]
    fn test_new_rows_jump_instead_of_gliding() {
        let mut glide = RowGlide::default();
        let start = Instant::now();
        glide.follow(40, 10, start);
        glide.jump_next_frame();
        assert_eq!(glide.drawn(), None);
        assert_eq!(glide.follow(0, 10, start), 0.0, "no glide from row 40");
    }

    #[test]
    fn test_rows_in_sight_take_a_part_row_at_each_edge() {
        assert_eq!(rows_in_sight(0.0, 3, 10), 0..3);
        assert_eq!(rows_in_sight(2.5, 3, 10), 2..6, "half of 2, all of 3 to 5");
        assert_eq!(rows_in_sight(8.5, 3, 10), 8..10, "cut at the count");
        assert_eq!(rows_in_sight(0.0, 3, 0), 0..0);
    }
}
