use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Limits used to bound OpenConvert resource usage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourcePolicy {
    /// Maximum number of preview frames to keep in memory.
    pub max_cached_frames: usize,
    /// Maximum number of worker threads to use for CPU-bound work.
    pub max_worker_threads: usize,
    /// Maximum temporary storage budget, in bytes.
    pub max_temp_bytes: u64,
}

impl Default for ResourcePolicy {
    fn default() -> Self {
        let parallelism = std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(1);

        Self {
            max_cached_frames: 96,
            max_worker_threads: parallelism.saturating_sub(1).max(1),
            max_temp_bytes: 4 * 1024 * 1024 * 1024,
        }
    }
}

/// Shared cancellation flag visible across cloned handles.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    /// Creates a token in the non-cancelled state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Marks the token as cancelled.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Returns whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

/// Fixed-capacity first-in-first-out cache for frame-like resources.
#[derive(Debug, Clone)]
pub struct FrameCache<K, V> {
    capacity: usize,
    keys: VecDeque<K>,
    values: HashMap<K, V>,
}

impl<K, V> FrameCache<K, V>
where
    K: Clone + Eq + Hash,
{
    /// Creates a cache that stores at most `capacity` entries.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            keys: VecDeque::new(),
            values: HashMap::with_capacity(capacity),
        }
    }

    /// Returns the current number of entries.
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Returns whether the cache contains no entries.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Returns a cached value by key without changing eviction order.
    pub fn get(&self, key: &K) -> Option<&V> {
        self.values.get(key)
    }

    /// Inserts or replaces a value, evicting oldest entries if needed.
    pub fn insert(&mut self, key: K, value: V) {
        if self.capacity == 0 {
            return;
        }

        if let Some(existing) = self.values.get_mut(&key) {
            *existing = value;
            return;
        }

        while self.values.len() >= self.capacity {
            let Some(oldest) = self.keys.pop_front() else {
                break;
            };

            self.values.remove(&oldest);
        }

        self.keys.push_back(key.clone());
        self.values.insert(key, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_cache_evicts_oldest_entry_at_capacity() {
        let mut cache = FrameCache::new(2);

        cache.insert(1, "first");
        cache.insert(2, "second");
        cache.insert(3, "third");

        assert_eq!(cache.len(), 2);
        assert_eq!(cache.get(&1), None);
        assert_eq!(cache.get(&2), Some(&"second"));
        assert_eq!(cache.get(&3), Some(&"third"));
    }

    #[test]
    fn cancelled_token_is_visible_to_clones() {
        let token = CancellationToken::new();
        let clone = token.clone();

        token.cancel();

        assert!(clone.is_cancelled());
    }
}
