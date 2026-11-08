//! A cache of shaped text kept across frames, [`ShapeCache`]: text that
//! looks the same as in a recent frame (same characters, colors, faces,
//! and font) is reused as shaped then instead of being shaped again, so a
//! frame that only moves rows (a scroll) or redraws them unchanged shapes
//! only the new ones. Entries not used for a while are dropped.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

// ========================================================================
// Constants
// ========================================================================

/// How many frames an entry stays without being used, so text scrolled
/// out and back soon after is not shaped again.
pub(crate) const KEEP_FRAMES: u64 = 60;

// ========================================================================
// Data Structures
// ========================================================================

/// Values of type `V` made from keys of type `K`, looked up by a borrowed
/// query that hashes the same way, kept while recently used.
pub(crate) struct ShapeCache<K, V> {
    /// The entries, by the hash of their query.
    buckets: HashMap<u64, Vec<Entry<K, V>>>,
    /// The number of the frame being drawn.
    frame: u64,
}

/// Where an entry is, valid until the next [`ShapeCache::finish_frame`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Slot {
    /// The hash of the entry's query.
    hash: u64,
    /// Its place in the bucket.
    index: usize,
}

/// One cached value.
struct Entry<K, V> {
    /// What it was made from.
    key: K,
    /// The last frame it was used in.
    last_used: u64,
    /// The value.
    value: V,
}

// ========================================================================
// ShapeCache Implementations
// ========================================================================

impl<K, V> ShapeCache<K, V> {
    /// An empty cache.
    pub(crate) fn new() -> Self {
        Self {
            buckets: HashMap::new(),
            frame: 0,
        }
    }

    /// The slot of the entry whose key `is_key` says matches `query`, made
    /// by `make` when there is none, and marked used in this frame.
    pub(crate) fn find_or_make<Q: Hash + ?Sized>(
        &mut self,
        query: &Q,
        is_key: impl Fn(&K, &Q) -> bool,
        make: impl FnOnce(&Q) -> (K, V),
    ) -> Slot {
        let mut hasher = DefaultHasher::new();
        query.hash(&mut hasher);
        let hash = hasher.finish();
        let bucket = self.buckets.entry(hash).or_default();
        let index = match bucket.iter().position(|entry| is_key(&entry.key, query)) {
            Some(index) => index,
            None => {
                let (key, value) = make(query);
                bucket.push(Entry {
                    key,
                    last_used: self.frame,
                    value,
                });
                bucket.len() - 1
            }
        };
        bucket[index].last_used = self.frame;
        Slot { hash, index }
    }

    /// The value at `slot`, found or made in this frame.
    pub(crate) fn value(&self, slot: Slot) -> Option<&V> {
        let entry = self.buckets.get(&slot.hash)?.get(slot.index)?;
        Some(&entry.value)
    }

    /// Ends the frame: drops the entries not used in the last
    /// [`KEEP_FRAMES`] frames. Slots found before are no longer valid.
    pub(crate) fn finish_frame(&mut self) {
        let oldest = self.frame.saturating_sub(KEEP_FRAMES);
        self.buckets.retain(|_, bucket| {
            bucket.retain(|entry| entry.last_used >= oldest);
            !bucket.is_empty()
        });
        self.frame += 1;
    }

    /// How many values are cached.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.buckets.values().map(Vec::len).sum()
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    /// Finds `text` in `cache`, counting each value made in `made`.
    fn find(cache: &mut ShapeCache<String, usize>, text: &str, made: &Cell<usize>) -> usize {
        let slot = cache.find_or_make(
            text,
            |key, query| key == query,
            |query| {
                made.set(made.get() + 1);
                (query.to_owned(), query.len())
            },
        );
        *cache.value(slot).expect("a value just found")
    }

    #[test]
    fn test_the_same_text_is_made_once_and_other_text_again() {
        let mut cache = ShapeCache::new();
        let made = Cell::new(0);
        assert_eq!(find(&mut cache, "row 1", &made), 5);
        cache.finish_frame();
        find(&mut cache, "row 1", &made);
        assert_eq!(made.get(), 1, "the next frame reuses it");
        find(&mut cache, "row 2", &made);
        assert_eq!(made.get(), 2, "a change of text is made anew");
    }

    #[test]
    fn test_entries_unused_for_long_are_dropped_and_used_ones_kept() {
        let mut cache = ShapeCache::new();
        let made = Cell::new(0);
        find(&mut cache, "kept", &made);
        find(&mut cache, "dropped", &made);
        for _ in 0..=KEEP_FRAMES {
            cache.finish_frame();
            find(&mut cache, "kept", &made);
        }
        cache.finish_frame();
        assert_eq!(cache.len(), 1);
        find(&mut cache, "dropped", &made);
        assert_eq!(made.get(), 3, "made again after it was dropped");
    }

    #[test]
    fn test_queries_whose_hashes_collide_still_tell_keys_apart() {
        let mut cache: ShapeCache<&str, &str> = ShapeCache::new();
        // Every query hashes alike: only the key check tells them apart.
        let first = cache.find_or_make(&(), |key, _| *key == "a", |_| ("a", "first"));
        let second = cache.find_or_make(&(), |key, _| *key == "b", |_| ("b", "second"));
        assert_ne!(first, second);
        assert_eq!(cache.value(first), Some(&"first"));
        assert_eq!(cache.value(second), Some(&"second"));
    }
}
