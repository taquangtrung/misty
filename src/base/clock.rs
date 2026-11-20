//! The time misty's widgets and the tree read: the system's clock, unless
//! a headless test holds a clock of its own on its thread
//! (`TreeWindow::advance`), so animations, delays, and timers step
//! without the test sleeping.

use std::cell::Cell;
#[cfg(any(test, feature = "testing"))]
use std::time::Duration;
use std::time::Instant;

thread_local! {
    /// The time a test holds this thread's clock at, or `None` while it
    /// follows the system's.
    static HELD: Cell<Option<Instant>> = const { Cell::new(None) };
}

// ========================================================================
// Functions
// ========================================================================

/// The time now: the held clock's on a thread a test holds it on, the
/// system's otherwise.
pub(crate) fn now() -> Instant {
    HELD.with(Cell::get).unwrap_or_else(Instant::now)
}

/// Holds this thread's clock, at the time now if it was not held, and
/// moves it on by `by`; returns the time it shows.
#[cfg(any(test, feature = "testing"))]
pub(crate) fn advance(by: Duration) -> Instant {
    let at = now() + by;
    HELD.with(|held| held.set(Some(at)));
    at
}

/// Lets this thread's clock follow the system's again.
#[cfg(any(test, feature = "testing"))]
pub(crate) fn release() {
    HELD.with(|held| held.set(None));
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_held_clock_moves_only_when_advanced_and_follows_the_system_once_let_go() {
        let at = advance(Duration::from_secs(60));
        assert_eq!(now(), at, "held still");
        assert_eq!(
            advance(Duration::from_millis(5)),
            at + Duration::from_millis(5)
        );
        release();
        assert!(now() < at, "the system's clock is a minute behind it");
    }
}
