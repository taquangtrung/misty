//! Panic survival: an app callback that panics is caught and reported, and
//! the app carries on, unless panics come so fast that carrying on would
//! only spin on them.

use std::any::Any;
use std::collections::VecDeque;
use std::panic::{self, AssertUnwindSafe};
use std::time::{Duration, Instant};

// ========================================================================
// Constants
// ========================================================================

/// How many panics the app carries on from within [`PANIC_WINDOW`]; the
/// next one inside the window is let through.
const MAX_PANICS: usize = 5;

/// How far back panics count toward [`MAX_PANICS`].
const PANIC_WINDOW: Duration = Duration::from_secs(10);

/// What a panic is described as when its payload is not text.
const NO_MESSAGE: &str = "a panic with no message";

// ========================================================================
// Data Structures
// ========================================================================

/// When the recent panics happened, oldest first.
#[derive(Debug, Default)]
pub(crate) struct PanicRate {
    /// The panics inside the window, oldest first.
    recent: VecDeque<Instant>,
}

// ========================================================================
// PanicRate Implementations
// ========================================================================

impl PanicRate {
    /// Counts a panic at `now`, forgetting those older than the window, and
    /// says whether there have now been too many to carry on from.
    pub(crate) fn is_over_limit_after(&mut self, now: Instant) -> bool {
        while let Some(oldest) = self.recent.front() {
            if now.saturating_duration_since(*oldest) < PANIC_WINDOW {
                break;
            }
            self.recent.pop_front();
        }
        self.recent.push_back(now);
        self.recent.len() > MAX_PANICS
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Runs `call`, catching a panic in it. A caught panic is counted in `rate`
/// and returned as `Err` with what it said; when it is one too many, it is
/// raised again instead, ending the app.
pub(crate) fn survive<R>(rate: &mut PanicRate, call: impl FnOnce() -> R) -> Result<R, String> {
    // Unwind safety is asserted, not proven: after a panic the app's state
    // may be half updated, which the app hears about through `on_panic` and
    // is judged better than losing everything it holds.
    let payload = match panic::catch_unwind(AssertUnwindSafe(call)) {
        Ok(value) => return Ok(value),
        Err(payload) => payload,
    };
    if rate.is_over_limit_after(Instant::now()) {
        panic::resume_unwind(payload);
    }
    Err(describe_panic(payload.as_ref()))
}

/// What a panic said, from the payload it unwound with.
fn describe_panic(payload: &(dyn Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        return (*text).to_owned();
    }
    match payload.downcast_ref::<String>() {
        Some(text) => text.clone(),
        None => NO_MESSAGE.to_owned(),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_the_sixth_panic_inside_ten_seconds_is_over_the_limit() {
        let mut rate = PanicRate::default();
        let start = Instant::now();
        for second in 0..5 {
            let at = start + Duration::from_secs(second);
            assert!(
                !rate.is_over_limit_after(at),
                "panic {second} is carried on from"
            );
        }
        assert!(rate.is_over_limit_after(start + Duration::from_secs(9)));
    }

    #[test]
    fn test_panics_older_than_the_window_stop_counting() {
        let mut rate = PanicRate::default();
        let start = Instant::now();
        for second in 0..5 {
            rate.is_over_limit_after(start + Duration::from_secs(second));
        }
        // Ten seconds after the first, it has left the window.
        assert!(!rate.is_over_limit_after(start + Duration::from_secs(10)));
        assert!(rate.is_over_limit_after(start + Duration::from_secs(10)));
    }

    #[test]
    fn test_survive_returns_the_value_or_the_panic_message() {
        let mut rate = PanicRate::default();
        assert_eq!(survive(&mut rate, || 7), Ok(7));
        let formatted = survive(&mut rate, || -> () { panic!("bad {}", 1) });
        assert_eq!(formatted, Err("bad 1".to_owned()));
        let literal = survive(&mut rate, || -> () { panic!("literal") });
        assert_eq!(literal, Err("literal".to_owned()));
        let opaque = survive(&mut rate, || -> () { panic::panic_any(3_u8) });
        assert_eq!(opaque, Err(NO_MESSAGE.to_owned()));
    }

    #[test]
    fn test_survive_rethrows_once_over_the_limit() {
        let mut rate = PanicRate::default();
        for _ in 0..MAX_PANICS {
            assert!(survive(&mut rate, || -> () { panic!("again") }).is_err());
        }
        let escaped = panic::catch_unwind(AssertUnwindSafe(|| {
            survive(&mut rate, || -> () { panic!("one too many") })
        }));
        assert!(escaped.is_err(), "the sixth panic ends the app");
    }
}
