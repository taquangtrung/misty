//! [`Thumbnails`]: small images loaded on a pool of worker threads and
//! kept in a bounded cache, for a grid of pictures such as a folder of
//! photos.
//!
//! The app gives a loader that turns one of its keys (a path, say) into
//! an [`Image`]: reading and decoding a file, or drawing one. Asking for a
//! key the cache lacks queues it; a worker runs the loader, shrinks the
//! image to the thumbnail size, and wakes the event loop; the app's
//! [`Thumbnails::poll`] then takes the results in. Requests no frame asked
//! for again are dropped before a worker starts them
//! ([`Thumbnails::begin_frame`]), so scrolling past a thousand pictures
//! loads only the ones that stay in sight, newest first. The cache holds
//! a bounded number of thumbnails and forgets the least recently drawn.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::mem;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;

use super::super::platform::waker::Waker;
use super::super::render::image::Image;

// ========================================================================
// Data Structures
// ========================================================================

/// A cache of thumbnails by key of type `K`, filled by worker threads.
pub struct Thumbnails<K> {
    /// The most thumbnails kept loaded.
    capacity: usize,
    /// Counts lookups, for forgetting the least recently used.
    clock: u64,
    /// What the cache knows of each key asked for.
    entries: HashMap<K, Entry>,
    /// The queue and results shared with the workers.
    shared: Arc<Shared<K>>,
}

/// What a [`Thumbnails`] holds for a key.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Thumbnail<'a> {
    /// Loading it failed; the reason is for people to read.
    Failed(&'a str),
    /// It is being loaded.
    Loading,
    /// It is loaded.
    Ready(&'a Image),
}

/// What the cache knows of one key.
enum Entry {
    /// Loading failed for this reason.
    Failed(String),
    /// Queued or being loaded.
    Loading,
    /// Loaded, and when it was last asked for.
    Ready(ReadyEntry),
}

/// A loaded thumbnail.
struct ReadyEntry {
    /// The thumbnail.
    image: Image,
    /// The clock at the last lookup.
    used: u64,
}

/// What the cache and its workers share.
struct Shared<K> {
    /// The keys to load, the newest last, and the results.
    state: Mutex<PoolState<K>>,
    /// Signalled when a key is queued or the cache is dropped.
    work: Condvar,
}

/// The work of the pool.
struct PoolState<K> {
    /// The keys waiting for a worker, the newest last.
    queue: VecDeque<K>,
    /// Loaded thumbnails or failures, waiting for [`Thumbnails::poll`].
    results: Vec<(K, Result<Image, String>)>,
    /// Whether the cache was dropped and the workers should stop.
    stopped: bool,
}

// ========================================================================
// Thumbnails Implementations
// ========================================================================

impl<K: Clone + Eq + Hash + Send + 'static> Thumbnails<K> {
    /// A cache of at most `capacity` thumbnails no wider or taller than
    /// `max_side` pixels, loaded by `load` on `workers` threads (at least
    /// one), each waking the event loop through `waker` when it is done.
    pub fn new<M: Send + 'static>(
        waker: Waker<M>,
        workers: usize,
        capacity: usize,
        max_side: u32,
        load: impl Fn(&K) -> Result<Image, String> + Send + Sync + 'static,
    ) -> Self {
        Self::with_wake(workers, capacity, max_side, load, move || waker.wake())
    }

    /// A cache as [`Thumbnails::new`] makes, its workers calling `wake`
    /// as each load ends: for an app that wakes its event loop through
    /// its own closure, or a headless test with no event loop to wake.
    pub fn with_wake(
        workers: usize,
        capacity: usize,
        max_side: u32,
        load: impl Fn(&K) -> Result<Image, String> + Send + Sync + 'static,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(PoolState {
                queue: VecDeque::new(),
                results: Vec::new(),
                stopped: false,
            }),
            work: Condvar::new(),
        });
        let job = Arc::new(move |key: &K| load(key).map(|image| image.shrunk_to(max_side)));
        let wake = Arc::new(wake);
        for _ in 0..workers.max(1) {
            let (shared, job, wake) = (Arc::clone(&shared), Arc::clone(&job), Arc::clone(&wake));
            // A pool that cannot start a thread loads nothing; the grid
            // keeps its placeholders.
            let _ = thread::Builder::new()
                .name("misty-thumbnails".into())
                .spawn(move || work(&shared, &*job, &*wake));
        }
        Self {
            capacity: capacity.max(1),
            clock: 0,
            entries: HashMap::new(),
            shared,
        }
    }

    /// Drops the requests no worker has started, so only what the coming
    /// frame asks for again is loaded. Call it before drawing.
    pub fn begin_frame(&mut self) {
        let mut state = lock(&self.shared.state);
        for key in state.queue.drain(..) {
            self.entries.remove(&key);
        }
    }

    /// What the cache holds for `key`, queuing it to load when it holds
    /// nothing; a loaded thumbnail counts as just used.
    pub fn get(&mut self, key: &K) -> Thumbnail<'_> {
        self.clock += 1;
        if !self.entries.contains_key(key) {
            self.entries.insert(key.clone(), Entry::Loading);
            lock(&self.shared.state).queue.push_back(key.clone());
            self.shared.work.notify_one();
        }
        match self.entries.get_mut(key) {
            Some(Entry::Ready(ready)) => {
                ready.used = self.clock;
                Thumbnail::Ready(&ready.image)
            }
            Some(Entry::Failed(reason)) => Thumbnail::Failed(reason),
            Some(Entry::Loading) | None => Thumbnail::Loading,
        }
    }

    /// Takes in what the workers loaded, forgetting the least recently
    /// used thumbnails past the capacity; whether anything arrived, so the
    /// app redraws. Call it from
    /// [`Application::about_to_wait`](crate::Application::about_to_wait).
    pub fn poll(&mut self) -> bool {
        let results = mem::take(&mut lock(&self.shared.state).results);
        if results.is_empty() {
            return false;
        }
        for (key, result) in results {
            let entry = match result {
                Ok(image) => Entry::Ready(ReadyEntry {
                    image,
                    used: self.clock,
                }),
                Err(reason) => Entry::Failed(reason),
            };
            self.entries.insert(key, entry);
        }
        self.evict();
        true
    }

    /// Forgets `key`, to load it again (its file changed).
    pub fn forget(&mut self, key: &K) {
        self.entries.remove(key);
    }

    /// Forgets the least recently used thumbnails past the capacity.
    fn evict(&mut self) {
        let mut ready: Vec<(u64, K)> = self
            .entries
            .iter()
            .filter_map(|(key, entry)| match entry {
                Entry::Ready(ready) => Some((ready.used, key.clone())),
                Entry::Failed(_) | Entry::Loading => None,
            })
            .collect();
        if ready.len() <= self.capacity {
            return;
        }
        ready.sort_unstable_by_key(|(used, _)| *used);
        let excess = ready.len() - self.capacity;
        for (_, key) in ready.into_iter().take(excess) {
            self.entries.remove(&key);
        }
    }
}

impl<K> Drop for Thumbnails<K> {
    fn drop(&mut self) {
        lock(&self.shared.state).stopped = true;
        self.shared.work.notify_all();
    }
}

// ========================================================================
// Functions
// ========================================================================

/// A worker: loads the newest queued key with `job` until the cache is
/// dropped, calling `wake` after each.
fn work<K>(
    shared: &Shared<K>,
    job: &(impl Fn(&K) -> Result<Image, String> + ?Sized),
    wake: &(impl Fn() + ?Sized),
) {
    loop {
        let key = {
            let mut state = lock(&shared.state);
            loop {
                if state.stopped {
                    return;
                }
                if let Some(key) = state.queue.pop_back() {
                    break key;
                }
                state = shared
                    .work
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };
        let result = job(&key);
        lock(&shared.state).results.push((key, result));
        wake();
    }
}

/// The pool's state, even after a worker panicked holding it.
fn lock<K>(state: &Mutex<PoolState<K>>) -> MutexGuard<'_, PoolState<K>> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    /// How long a test waits for the workers.
    const PATIENCE: Duration = Duration::from_secs(10);

    /// A square image `side` pixels wide.
    fn square(side: u32) -> Image {
        let pixels = vec![0x80; (side * side) as usize * 4];
        Image::from_rgba(side, side, pixels).expect("a valid image")
    }

    /// Polls `thumbnails` until `done` holds, failing after [`PATIENCE`].
    fn wait_for(
        thumbnails: &mut Thumbnails<u32>,
        mut done: impl FnMut(&mut Thumbnails<u32>) -> bool,
    ) {
        let start = Instant::now();
        while !done(thumbnails) {
            assert!(start.elapsed() < PATIENCE, "the workers never finished");
            thumbnails.poll();
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn test_a_key_loads_on_a_worker_shrunk_to_the_thumbnail_size() {
        let load = |key: &u32| match key {
            0 => Err("unreadable".to_owned()),
            _ => Ok(square(400)),
        };
        let mut thumbnails = Thumbnails::with_wake(2, 8, 100, load, || {});
        assert_eq!(thumbnails.get(&1), Thumbnail::Loading);
        wait_for(&mut thumbnails, |t| {
            matches!(t.get(&1), Thumbnail::Ready(_))
        });
        let Thumbnail::Ready(image) = thumbnails.get(&1) else {
            unreachable!("waited for it");
        };
        assert_eq!((image.width(), image.height()), (100, 100));
        thumbnails.get(&0);
        wait_for(&mut thumbnails, |t| {
            t.get(&0) == Thumbnail::Failed("unreadable")
        });
    }

    #[test]
    fn test_past_the_capacity_the_least_recently_used_are_forgotten() {
        let mut thumbnails = Thumbnails::with_wake(1, 2, 16, |_: &u32| Ok(square(8)), || {});
        for key in [1, 2] {
            thumbnails.get(&key);
            wait_for(&mut thumbnails, |t| {
                matches!(t.get(&key), Thumbnail::Ready(_))
            });
        }
        thumbnails.get(&1);
        thumbnails.get(&3);
        wait_for(&mut thumbnails, |t| {
            t.entries.len() == 2 && t.entries.contains_key(&3)
        });
        assert!(thumbnails.entries.contains_key(&1), "used last, so kept");
        assert!(
            !thumbnails.entries.contains_key(&2),
            "the least recently used"
        );
    }

    #[test]
    fn test_requests_not_started_are_dropped_at_the_next_frame() {
        let gate = Arc::new(Mutex::new(()));
        let held = gate.lock().expect("the gate");
        let worker_gate = Arc::clone(&gate);
        let load = move |_: &u32| {
            let _open = worker_gate.lock().unwrap_or_else(PoisonError::into_inner);
            Ok(square(8))
        };
        let mut thumbnails = Thumbnails::with_wake(1, 8, 16, load, || {});
        thumbnails.get(&1);
        let start = Instant::now();
        while !lock(&thumbnails.shared.state).queue.is_empty() {
            assert!(start.elapsed() < PATIENCE, "the worker never started");
            thread::yield_now();
        }
        thumbnails.get(&2);
        thumbnails.get(&3);
        thumbnails.begin_frame();
        thumbnails.get(&3);
        drop(held);
        wait_for(&mut thumbnails, |t| {
            matches!(t.get(&3), Thumbnail::Ready(_))
        });
        assert!(
            matches!(thumbnails.get(&1), Thumbnail::Ready(_)),
            "already started"
        );
        assert!(
            !thumbnails.entries.contains_key(&2),
            "dropped, never asked again"
        );
    }
}
