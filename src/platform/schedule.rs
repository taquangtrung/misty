//! Redraws asked for at a later time, and how the runtime picks the moment
//! it next has to wake.

use std::time::Instant;

use super::super::platform::window_id::WindowId;

// ========================================================================
// Data Structures
// ========================================================================

/// The redraws asked for at a later time: at most one per window, the
/// earliest asked for.
#[derive(Debug, Default)]
pub(crate) struct RedrawSchedule {
    /// Each window waiting for a redraw, with when it is due.
    pending: Vec<(WindowId, Instant)>,
}

// ========================================================================
// RedrawSchedule Implementations
// ========================================================================

impl RedrawSchedule {
    /// Asks for a redraw of `window` at `at`. A window already waiting keeps
    /// whichever time comes first.
    pub(crate) fn add(&mut self, window: WindowId, at: Instant) {
        match self
            .pending
            .iter_mut()
            .find(|(pending, _)| *pending == window)
        {
            Some((_, due)) => *due = (*due).min(at),
            None => self.pending.push((window, at)),
        }
    }

    /// Drops the redraw asked for `window`, which has closed.
    pub(crate) fn forget(&mut self, window: WindowId) {
        self.pending.retain(|&(pending, _)| pending != window);
    }

    /// Removes and returns the windows whose redraw is due at `now`.
    pub(crate) fn take_due(&mut self, now: Instant) -> Vec<WindowId> {
        let mut due = Vec::new();
        self.pending.retain(|&(window, at)| {
            let is_due = at <= now;
            if is_due {
                due.push(window);
            }
            !is_due
        });
        due
    }

    /// When the next pending redraw is due, or `None` when none is pending.
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        self.pending.iter().map(|&(_, at)| at).min()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The earlier of two optional deadlines: whichever is set when only one
/// is, and `None` when neither is.
pub(crate) fn earliest(first: Option<Instant>, second: Option<Instant>) -> Option<Instant> {
    match (first, second) {
        (Some(first), Some(second)) => Some(first.min(second)),
        (Some(only), None) | (None, Some(only)) => Some(only),
        (None, None) => None,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use winit::window;

    use super::*;

    fn window(id: u64) -> WindowId {
        WindowId(window::WindowId::from(id))
    }

    #[test]
    fn test_earliest_prefers_the_sooner_and_keeps_a_lone_deadline() {
        let now = Instant::now();
        let later = now + Duration::from_millis(5);
        assert_eq!(earliest(Some(later), Some(now)), Some(now));
        assert_eq!(earliest(None, Some(later)), Some(later));
        assert_eq!(earliest(Some(later), None), Some(later));
        assert_eq!(earliest(None, None), None);
    }

    #[test]
    fn test_a_window_keeps_its_soonest_redraw() {
        let now = Instant::now();
        let mut schedule = RedrawSchedule::default();
        schedule.add(window(1), now + Duration::from_millis(50));
        schedule.add(window(1), now + Duration::from_millis(10));
        schedule.add(window(1), now + Duration::from_millis(30));
        assert_eq!(
            schedule.next_deadline(),
            Some(now + Duration::from_millis(10))
        );
        assert_eq!(
            schedule.take_due(now + Duration::from_millis(10)),
            [window(1)]
        );
        assert_eq!(schedule.next_deadline(), None, "one redraw per window");
    }

    #[test]
    fn test_only_due_redraws_are_taken() {
        let now = Instant::now();
        let mut schedule = RedrawSchedule::default();
        schedule.add(window(1), now);
        schedule.add(window(2), now + Duration::from_secs(1));
        assert_eq!(schedule.take_due(now), [window(1)]);
        assert_eq!(schedule.take_due(now), []);
        assert_eq!(schedule.next_deadline(), Some(now + Duration::from_secs(1)));
    }
}
