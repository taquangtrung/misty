//! [`Deadlines`]: a keyed set of moments something is due, for the timers
//! an app would otherwise keep by hand: a search that runs once typing
//! settles, a position saved a second after scrolling stops, a tooltip
//! that waits. The app sets a key's deadline, returns
//! [`Deadlines::next`] from
//! [`Application::about_to_wait`](crate::Application::about_to_wait) so
//! the event loop wakes then, and acts on the keys
//! [`Deadlines::take_due`] hands back.

use std::time::Instant;

// ========================================================================
// Data Structures
// ========================================================================

/// The moments keys of type `K` are due, at most one per key.
#[derive(Clone, Debug)]
pub struct Deadlines<K> {
    /// Each key waiting, with when it is due.
    pending: Vec<(K, Instant)>,
}

// ========================================================================
// Deadlines Implementations
// ========================================================================

impl<K: PartialEq> Deadlines<K> {
    /// No deadlines.
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
        }
    }

    /// Makes `key` due at `at`, in place of any deadline it had: setting
    /// it again on every keystroke puts it off until typing settles.
    pub fn set(&mut self, key: K, at: Instant) {
        match self.pending.iter_mut().find(|(pending, _)| *pending == key) {
            Some((_, due)) => *due = at,
            None => self.pending.push((key, at)),
        }
    }

    /// Drops `key`'s deadline, if it has one.
    pub fn clear(&mut self, key: &K) {
        self.pending.retain(|(pending, _)| pending != key);
    }

    /// When `key` is due, or `None` when it has no deadline.
    pub fn get(&self, key: &K) -> Option<Instant> {
        self.pending
            .iter()
            .find(|(pending, _)| pending == key)
            .map(|&(_, at)| at)
    }

    /// The soonest deadline, or `None` when there is none: what
    /// [`Application::about_to_wait`](crate::Application::about_to_wait)
    /// returns so the event loop wakes in time.
    pub fn next(&self) -> Option<Instant> {
        self.pending.iter().map(|&(_, at)| at).min()
    }

    /// Removes and returns the keys due at `now`, the soonest first.
    pub fn take_due(&mut self, now: Instant) -> Vec<K> {
        let (mut due, waiting): (Vec<_>, Vec<_>) =
            self.pending.drain(..).partition(|&(_, at)| at <= now);
        self.pending = waiting;
        due.sort_by_key(|&(_, at)| at);
        due.into_iter().map(|(key, _)| key).collect()
    }
}

impl<K: PartialEq> Default for Deadlines<K> {
    fn default() -> Self {
        Self::new()
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn test_setting_a_key_again_puts_it_off_and_only_due_keys_are_taken_soonest_first() {
        let now = Instant::now();
        let at = |ms| now + Duration::from_millis(ms);
        let mut deadlines = Deadlines::new();
        assert_eq!(deadlines.next(), None);
        deadlines.set("search", at(100));
        deadlines.set("save", at(50));
        deadlines.set("search", at(300));
        deadlines.set("tooltip", at(20));
        assert_eq!(deadlines.get(&"search"), Some(at(300)), "put off, not kept");
        assert_eq!(deadlines.next(), Some(at(20)));
        assert_eq!(deadlines.take_due(at(10)), Vec::<&str>::new());
        assert_eq!(deadlines.take_due(at(120)), ["tooltip", "save"]);
        assert_eq!(deadlines.next(), Some(at(300)));
        deadlines.clear(&"search");
        assert_eq!(deadlines.next(), None);
        assert_eq!(deadlines.take_due(at(1000)), Vec::<&str>::new());
    }
}
