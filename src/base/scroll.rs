//! Smooth scrolling: [`SmoothScroll`] eases a scroll position from where
//! it is to a new target over a short time, so a view that jumps by rows
//! glides there instead, and jumps at once when the user asked for reduced
//! motion. Positions are in whatever unit the view scrolls by (rows,
//! pixels). It is a [`Tween`] that keeps its target: a new target eases on
//! from where the scroll is, along [`Easing::EaseOut`].

use std::time::{Duration, Instant};

use super::motion::{Easing, Tween};

// ========================================================================
// Constants
// ========================================================================

/// How long a scroll takes unless [`SmoothScroll::new`] says otherwise.
pub(crate) const DEFAULT_DURATION: Duration = Duration::from_millis(120);

// ========================================================================
// Data Structures
// ========================================================================

/// A scroll position easing toward its target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SmoothScroll {
    /// The easing from where the running scroll started to its target.
    tween: Tween<f32>,
}

// ========================================================================
// SmoothScroll Implementations
// ========================================================================

impl SmoothScroll {
    /// A scroll at rest at 0 whose scrolls take `duration`.
    pub fn new(duration: Duration) -> Self {
        Self {
            tween: Tween::new(0.0, 0.0, duration).with_easing(Easing::EaseOut),
        }
    }

    /// Makes scrolls jump at once (`true`) or ease (`false`). Turning
    /// reduced motion on ends a running scroll at its target.
    pub fn set_reduced_motion(&mut self, reduced_motion: bool) {
        self.tween.set_reduced_motion(reduced_motion);
    }

    /// Starts easing from where the scroll is at `now` to `target`; at once
    /// with reduced motion or a zero duration. The same target again
    /// changes nothing.
    pub fn scroll_to(&mut self, target: f32, now: Instant) {
        if target != self.tween.target() {
            self.tween.retarget(target, now);
        }
    }

    /// Puts the scroll at `target` at once, ending a running scroll.
    pub fn jump_to(&mut self, target: f32) {
        self.tween.jump_to(target);
    }

    /// Where the scroll is at `now`: exactly the target once it is done.
    pub fn position(&self, now: Instant) -> f32 {
        self.tween.value(now)
    }

    /// Where the scroll ends.
    pub fn target(&self) -> f32 {
        self.tween.target()
    }

    /// Whether the scroll is still moving at `now`.
    pub fn is_animating(&self, now: Instant) -> bool {
        self.tween.is_animating(now)
    }

    /// When the view should next be drawn for a scroll still moving at
    /// `now`, or `None` at rest.
    pub fn next_frame(&self, now: Instant) -> Option<Instant> {
        self.tween.next_frame(now)
    }
}

impl Default for SmoothScroll {
    fn default() -> Self {
        Self::new(DEFAULT_DURATION)
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::motion::FRAME_INTERVAL;

    const DURATION: Duration = Duration::from_millis(100);

    fn at(start: Instant, millis: u64) -> Instant {
        start + Duration::from_millis(millis)
    }

    #[test]
    fn test_a_scroll_eases_to_its_target_and_rests_there_exactly() {
        let start = Instant::now();
        let mut scroll = SmoothScroll::new(DURATION);
        scroll.scroll_to(10.0, start);
        assert_eq!(scroll.position(start), 0.0);
        let halfway = scroll.position(at(start, 50));
        assert!(halfway > 5.0 && halfway < 10.0, "{halfway}");
        assert!(scroll.is_animating(at(start, 99)));
        assert_eq!(scroll.next_frame(start), Some(start + FRAME_INTERVAL));
        assert_eq!(scroll.position(at(start, 100)), 10.0);
        assert!(!scroll.is_animating(at(start, 100)));
        assert_eq!(scroll.next_frame(at(start, 100)), None);
    }

    #[test]
    fn test_a_new_target_mid_scroll_continues_from_where_it_is() {
        let start = Instant::now();
        let mut scroll = SmoothScroll::new(DURATION);
        scroll.scroll_to(10.0, start);
        let middle = at(start, 50);
        let there = scroll.position(middle);
        scroll.scroll_to(0.0, middle);
        assert_eq!(scroll.position(middle), there, "no jump on retargeting");
        assert_eq!(scroll.position(at(middle, 100)), 0.0);
    }

    #[test]
    fn test_reduced_motion_jumps_and_ends_a_running_scroll() {
        let start = Instant::now();
        let mut scroll = SmoothScroll::new(DURATION);
        scroll.scroll_to(10.0, start);
        scroll.set_reduced_motion(true);
        assert_eq!(scroll.position(start), 10.0);
        scroll.scroll_to(3.0, start);
        assert_eq!(scroll.position(start), 3.0);
        assert!(!scroll.is_animating(start));
    }
}
