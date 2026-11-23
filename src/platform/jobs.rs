//! [`Jobs`]: a small pool of worker threads that runs the app's closures
//! off the UI thread (reading a file, parsing a document, searching) and
//! hands each result to [`Application::message`](crate::Application::message)
//! through a [`Waker`], waking the event loop. No async runtime is behind
//! it: a job is a plain closure, run once on one of the pool's threads.
//!
//! [`Jobs::spawn`] returns a [`JobHandle`] that cancels the job when it is
//! dropped: a job still queued never starts, and the result of one already
//! running is thrown away when it ends, so a result that went stale while
//! it was computed (the user opened another document meanwhile) never
//! reaches the app. Keep the handle while the result is wanted, replace it
//! with a newer job's to cancel the older, or [`JobHandle::detach`] it to
//! let the job finish whatever happens.

use std::collections::VecDeque;
use std::fmt::{self, Debug, Formatter};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;

use super::waker::Waker;

// ========================================================================
// Data Structures
// ========================================================================

/// A pool of worker threads running jobs that each end in a message of
/// type `M` for the app. Dropping the pool stops its threads once their
/// running jobs end; queued jobs never start.
pub struct Jobs<M: 'static> {
    /// The queue shared with the workers.
    shared: Arc<Shared<M>>,
}

/// A job queued or running on a [`Jobs`] pool. Dropping it cancels the
/// job: it never starts if it is still queued, and its result is thrown
/// away if it is running.
#[derive(Debug)]
#[must_use = "dropping a job's handle cancels the job; keep it, or detach it"]
pub struct JobHandle {
    /// Set when the job is cancelled, read by the worker before it starts
    /// the job and before it delivers the result.
    cancelled: Arc<AtomicBool>,
    /// Whether dropping the handle leaves the job be.
    detached: bool,
}

/// What a pool and its workers share.
struct Shared<M: 'static> {
    /// The jobs waiting for a worker, and whether the pool was dropped.
    state: Mutex<PoolState>,
    /// Where results go.
    waker: Waker<M>,
    /// Signalled when a job is queued or the pool is dropped.
    work: Condvar,
}

/// The work of a pool.
struct PoolState {
    /// The jobs waiting for a worker, the oldest first.
    queue: VecDeque<QueuedJob>,
    /// Whether the pool was dropped and the workers should stop.
    stopped: bool,
}

/// A job waiting for a worker: what to run, which delivers its own result,
/// and whether it was cancelled.
struct QueuedJob {
    /// Set when the job's handle was dropped.
    cancelled: Arc<AtomicBool>,
    /// Runs the job and hands back what delivers its result.
    run: Box<dyn FnOnce() -> Box<dyn FnOnce() + Send> + Send>,
}

// ========================================================================
// Jobs Implementations
// ========================================================================

impl<M: Send + 'static> Jobs<M> {
    /// A pool of `workers` threads (at least one), each handing the result
    /// of a job it finished to the app through `waker` (from
    /// [`AppContext::waker`](crate::AppContext::waker), or a headless
    /// test window's). A pool that cannot start a thread runs nothing on
    /// it; with no thread at all, jobs wait in the queue.
    pub fn new(waker: Waker<M>, workers: usize) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(PoolState {
                queue: VecDeque::new(),
                stopped: false,
            }),
            waker,
            work: Condvar::new(),
        });
        for _ in 0..workers.max(1) {
            let shared = Arc::clone(&shared);
            // A thread that cannot start leaves its jobs to the others.
            let _ = thread::Builder::new()
                .name("misty-jobs".into())
                .spawn(move || work(&shared));
        }
        Self { shared }
    }

    /// Queues `job` to run on a worker; its result reaches
    /// [`Application::message`](crate::Application::message) unless the
    /// returned handle was dropped by the time it ends. Jobs start in the
    /// order queued. A job that panics delivers nothing and leaves its
    /// worker running.
    pub fn spawn(&self, job: impl FnOnce() -> M + Send + 'static) -> JobHandle {
        let cancelled = Arc::new(AtomicBool::new(false));
        let waker = self.shared.waker.clone();
        let run = Box::new(move || {
            let message = job();
            Box::new(move || {
                // The app has quit; nothing is left to tell.
                let _ = waker.send(message);
            }) as Box<dyn FnOnce() + Send>
        });
        let queued = QueuedJob {
            cancelled: Arc::clone(&cancelled),
            run,
        };
        lock(&self.shared.state).queue.push_back(queued);
        self.shared.work.notify_one();
        JobHandle {
            cancelled,
            detached: false,
        }
    }
}

impl<M: 'static> Drop for Jobs<M> {
    fn drop(&mut self) {
        let mut state = lock(&self.shared.state);
        state.stopped = true;
        state.queue.clear();
        drop(state);
        self.shared.work.notify_all();
    }
}

impl<M: 'static> Debug for Jobs<M> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let queued = lock(&self.shared.state).queue.len();
        f.debug_struct("Jobs")
            .field("queued", &queued)
            .finish_non_exhaustive()
    }
}

// ========================================================================
// JobHandle Implementations
// ========================================================================

impl JobHandle {
    /// Lets the job run and deliver its result whatever becomes of this
    /// handle.
    pub fn detach(mut self) {
        self.detached = true;
    }
}

impl Drop for JobHandle {
    fn drop(&mut self) {
        if !self.detached {
            self.cancelled.store(true, Ordering::Release);
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// A worker's loop: takes the oldest job not cancelled, runs it, and
/// delivers its result unless it was cancelled meanwhile, until the pool
/// is dropped.
fn work<M>(shared: &Shared<M>) {
    loop {
        let job = {
            let mut state = lock(&shared.state);
            loop {
                if state.stopped {
                    return;
                }
                match state.queue.pop_front() {
                    Some(job) if job.cancelled.load(Ordering::Acquire) => {}
                    Some(job) => break job,
                    None => {
                        state = shared
                            .work
                            .wait(state)
                            .unwrap_or_else(PoisonError::into_inner);
                    }
                }
            }
        };
        let QueuedJob { cancelled, run } = job;
        // A panicking job delivers nothing; the worker goes on.
        let Ok(deliver) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)) else {
            continue;
        };
        if !cancelled.load(Ordering::Acquire) {
            deliver();
        }
    }
}

/// `mutex` locked, even when a thread panicked while holding it: the queue
/// stays usable.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{self, Receiver};
    use std::time::Duration;

    use super::super::waker::UserEvent;
    use super::*;

    /// How long a test waits for a message that should come.
    const PATIENCE: Duration = Duration::from_secs(10);

    /// The next message `receiver` gets, or `None` when none comes in
    /// `wait`.
    fn next(receiver: &Receiver<UserEvent<u32>>, wait: Duration) -> Option<u32> {
        match receiver.recv_timeout(wait) {
            Ok(UserEvent::Message(message)) => Some(message),
            Ok(UserEvent::Access(_) | UserEvent::Appearance(_) | UserEvent::Wake) | Err(_) => None,
        }
    }

    #[test]
    fn test_each_job_s_result_reaches_the_app() {
        let (waker, receiver) = Waker::channel();
        let jobs = Jobs::new(waker, 3);
        let handles: Vec<JobHandle> = (1..=5).map(|n| jobs.spawn(move || n * 10)).collect();
        let mut got: Vec<u32> = (0..5).filter_map(|_| next(&receiver, PATIENCE)).collect();
        got.sort_unstable();
        assert_eq!(got, [10, 20, 30, 40, 50]);
        drop(handles);
    }

    #[test]
    fn test_a_dropped_handle_stops_a_queued_job_and_drops_a_running_one_s_result() {
        let (waker, receiver) = Waker::channel();
        let jobs = Jobs::new(waker, 1);
        let (started_tx, started) = mpsc::channel();
        let (release_tx, release) = mpsc::channel::<()>();
        let running = jobs.spawn(move || {
            started_tx.send(()).expect("the test waits");
            release.recv().expect("the test releases");
            1
        });
        let ran = Arc::new(AtomicBool::new(false));
        let queued = jobs.spawn({
            let ran = Arc::clone(&ran);
            move || {
                ran.store(true, Ordering::SeqCst);
                2
            }
        });
        started
            .recv_timeout(PATIENCE)
            .expect("the first job started");
        drop(queued);
        drop(running);
        release_tx.send(()).expect("the job waits");
        jobs.spawn(|| 3).detach();
        assert_eq!(
            next(&receiver, PATIENCE),
            Some(3),
            "1 was stale, 2 never ran"
        );
        assert!(
            !ran.load(Ordering::SeqCst),
            "the cancelled job never started"
        );
        assert_eq!(next(&receiver, Duration::from_millis(50)), None);
    }

    #[test]
    fn test_a_panicking_job_delivers_nothing_and_the_worker_goes_on() {
        let (waker, receiver) = Waker::channel();
        let jobs = Jobs::new(waker, 1);
        let failing = jobs.spawn(|| panic!("a job that fails"));
        let after = jobs.spawn(|| 7);
        assert_eq!(next(&receiver, PATIENCE), Some(7));
        drop((failing, after));
    }
}
