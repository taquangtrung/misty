//! Hang detection: the event loop marks the main thread busy while it runs
//! a callback, and a watchdog thread tells the app's
//! [`HangWatch`](super::super::app::hang::HangWatch) when one runs past
//! its limit. The main thread draws every window, so a callback that never
//! returns freezes them all; the app chooses what to do then (log it,
//! write a report and exit, restart), from the watchdog thread, since the
//! main thread is the one stuck.
//!
//! The watchdog sleeps with no timeout while the main thread is idle, so a
//! quiet app costs nothing; while it is busy, it wakes once at the limit.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use super::super::app::hang::{HangPhase, HangReport, HangWatch};

// ========================================================================
// Constants
// ========================================================================

/// The watchdog thread's name, as a debugger lists it.
const THREAD_NAME: &str = "misty-watchdog";

/// The idle mark in [`Beat::busy_since`].
const IDLE: u64 = 0;

/// The line of `/proc/self/status` giving the memory in use, in kB.
#[cfg(target_os = "linux")]
const RSS_FIELD: &str = "VmRSS:";

// ========================================================================
// Data Structures
// ========================================================================

/// What the main thread and the watchdog share.
#[derive(Debug)]
pub(crate) struct Beat {
    /// When the callback running began, in milliseconds since `start`, or
    /// [`IDLE`].
    busy_since: AtomicU64,
    /// The [`HangPhase`] of the callback running.
    phase: AtomicU8,
    /// Whether the watchdog sleeps with no timeout, to be woken by the
    /// next callback.
    sleeping: AtomicBool,
    /// When the watch began, which `busy_since` counts from.
    start: Instant,
    /// The watchdog thread, to wake.
    watchdog: Thread,
}

/// The main thread busy with one callback while it lives.
pub(crate) struct Busy {
    /// What it marked.
    beat: Option<Arc<Beat>>,
}

// ========================================================================
// Beat Implementations
// ========================================================================

impl Beat {
    /// Marks the main thread busy with a callback of `phase`, waking the
    /// watchdog if it sleeps.
    fn begin(&self, phase: HangPhase) {
        self.phase.store(phase as u8, Ordering::SeqCst);
        // Never `IDLE`, even in the watch's first millisecond.
        let now = self.start.elapsed().as_millis() as u64 + 1;
        self.busy_since.store(now, Ordering::SeqCst);
        if self.sleeping.load(Ordering::SeqCst) {
            self.watchdog.unpark();
        }
    }

    /// Marks the main thread idle.
    fn end(&self) {
        self.busy_since.store(IDLE, Ordering::SeqCst);
    }
}

// ========================================================================
// Busy Implementations
// ========================================================================

impl Busy {
    /// Marks the main thread busy with a callback of `phase` until the
    /// result is dropped; nothing when no watch runs.
    pub(crate) fn begin(beat: Option<&Arc<Beat>>, phase: HangPhase) -> Self {
        if let Some(beat) = beat {
            beat.begin(phase);
        }
        Self {
            beat: beat.cloned(),
        }
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        if let Some(beat) = &self.beat {
            beat.end();
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Starts the watchdog `watch` asks for, returning what the main thread
/// marks its callbacks in; `None` for a watch of nothing, or when no
/// thread could be started.
pub(crate) fn start(watch: HangWatch) -> Option<Arc<Beat>> {
    let on_hang = watch.on_hang?;
    let limit = watch.limit;
    let (sender, receiver) = std::sync::mpsc::channel::<Arc<Beat>>();
    let watchdog = thread::Builder::new()
        .name(THREAD_NAME.to_owned())
        .spawn(move || {
            if let Ok(beat) = receiver.recv() {
                watch_beat(&beat, limit, &*on_hang);
            }
        })
        .ok()?;
    let beat = Arc::new(Beat {
        busy_since: AtomicU64::new(IDLE),
        phase: AtomicU8::new(HangPhase::Input as u8),
        sleeping: AtomicBool::new(false),
        start: Instant::now(),
        watchdog: watchdog.thread().clone(),
    });
    sender.send(Arc::clone(&beat)).ok()?;
    Some(beat)
}

/// The watchdog's loop: sleeps while the main thread is idle, wakes at the
/// limit of each callback, and tells `on_hang` of each that passed it, and
/// again each limit after while it lasts.
fn watch_beat(beat: &Beat, limit: Duration, on_hang: &(dyn Fn(&HangReport) + Send + Sync)) {
    // The callback last told of, and how many times.
    let mut told = (IDLE, 0);
    loop {
        // Say it sleeps before looking, so a callback that begins in
        // between wakes it.
        beat.sleeping.store(true, Ordering::SeqCst);
        let since = beat.busy_since.load(Ordering::SeqCst);
        if since == IDLE {
            thread::park();
            beat.sleeping.store(false, Ordering::SeqCst);
            continue;
        }
        beat.sleeping.store(false, Ordering::SeqCst);
        let busy = beat
            .start
            .elapsed()
            .saturating_sub(Duration::from_millis(since - 1));
        if busy < limit {
            thread::park_timeout(limit - busy);
            continue;
        }
        let repeats = match told.0 == since {
            true => told.1 + 1,
            false => 0,
        };
        told = (since, repeats);
        on_hang(&HangReport {
            memory: memory_in_use(),
            phase: HangPhase::from_u8(beat.phase.load(Ordering::SeqCst)),
            repeats,
            stuck_for: busy,
        });
        thread::park_timeout(limit);
    }
}

/// The memory the process is using, in bytes: its resident set.
#[cfg(target_os = "linux")]
fn memory_in_use() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with(RSS_FIELD))?;
    let kilobytes: u64 = line[RSS_FIELD.len()..]
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    Some(kilobytes * 1024)
}

/// Other systems are not asked.
#[cfg(not(target_os = "linux"))]
fn memory_in_use() -> Option<u64> {
    None
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::mpsc;

    use super::*;

    /// How long a test's callback may run.
    const LIMIT: Duration = Duration::from_millis(40);

    /// A watch at [`LIMIT`] sending each report to the receiver returned.
    fn watched() -> (Arc<Beat>, mpsc::Receiver<HangReport>) {
        let (sender, receiver) = mpsc::channel();
        let sender = Mutex::new(sender);
        let watch = HangWatch::new().limit(LIMIT).on_hang(move |report| {
            let _ = sender.lock().map(|sender| sender.send(report.clone()));
        });
        (start(watch).expect("a watchdog"), receiver)
    }

    #[test]
    fn test_a_callback_past_the_limit_is_told_with_its_phase_and_again_while_it_lasts() {
        let (beat, reports) = watched();
        let busy = Busy::begin(Some(&beat), HangPhase::Drawing);
        let first = reports.recv_timeout(LIMIT * 25).expect("a hang told");
        assert_eq!((first.phase, first.repeats), (HangPhase::Drawing, 0));
        assert!(first.stuck_for >= LIMIT, "{:?}", first.stuck_for);
        let again = reports.recv_timeout(LIMIT * 25).expect("told again");
        assert_eq!(again.repeats, 1);
        assert!(again.stuck_for > first.stuck_for);
        drop(busy);
        // Ended: nothing more, once a report on its way has landed.
        thread::sleep(LIMIT * 2);
        while reports.try_recv().is_ok() {}
        assert!(reports.recv_timeout(LIMIT * 4).is_err(), "ended, not told");
    }

    #[test]
    fn test_callbacks_that_return_in_time_are_never_told() {
        let (beat, reports) = watched();
        for _ in 0..10 {
            let _busy = Busy::begin(Some(&beat), HangPhase::Input);
            thread::sleep(LIMIT / 10);
        }
        assert!(reports.recv_timeout(LIMIT * 3).is_err());
    }

    #[test]
    fn test_a_watch_that_is_off_starts_no_thread() {
        assert!(start(HangWatch::off()).is_none());
    }
}
