//! Process-local budgets for calls to Silicon Accounts.
//!
//! Silicon Accounts allows each app 600 account lookups a minute. Briefcase
//! keeps well inside that with a token bucket and answers from its own cache
//! first; a lookup the bucket cannot afford fails with a precise error instead
//! of being sent and refused upstream.

use std::{
    collections::HashMap,
    hash::Hash,
    sync::Mutex,
    time::{Duration, Instant},
};

/// Silicon Accounts' per-app lookup allowance.
pub const LOOKUPS_PER_MINUTE: u32 = 600;

/// A token bucket refilled continuously at `capacity` tokens a minute.
#[derive(Debug)]
pub struct TokenBucket {
    capacity: f64,
    per_second: f64,
    state: Mutex<(f64, Instant)>,
}

impl TokenBucket {
    /// A full bucket of `capacity` tokens refilled over one minute.
    #[must_use]
    pub fn per_minute(capacity: u32) -> Self {
        let capacity = f64::from(capacity.max(1));
        Self {
            capacity,
            per_second: capacity / 60.0,
            state: Mutex::new((capacity, Instant::now())),
        }
    }

    /// Takes one token, or returns how long until one is available.
    ///
    /// # Errors
    ///
    /// Returns the wait until the next token when the bucket is empty.
    pub fn try_take(&self) -> Result<(), Duration> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (tokens, refilled_at) = &mut *state;
        let now = Instant::now();
        *tokens = now
            .duration_since(*refilled_at)
            .as_secs_f64()
            .mul_add(self.per_second, *tokens)
            .min(self.capacity);
        *refilled_at = now;
        if *tokens >= 1.0 {
            *tokens -= 1.0;
            Ok(())
        } else {
            Err(Duration::from_secs_f64((1.0 - *tokens) / self.per_second))
        }
    }
}

/// A small time-bounded memo keyed by `K`, bounded to `capacity` entries.
#[derive(Debug)]
pub struct ExpiringMap<K, V> {
    ttl: Duration,
    capacity: usize,
    entries: Mutex<HashMap<K, (V, Instant)>>,
}

impl<K: Eq + Hash + Clone, V: Clone> ExpiringMap<K, V> {
    /// An empty map whose entries live `ttl`.
    #[must_use]
    pub fn new(ttl: Duration, capacity: usize) -> Self {
        Self {
            ttl,
            capacity: capacity.max(1),
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Returns a live entry.
    pub fn get(&self, key: &K) -> Option<V> {
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries
            .get(key)
            .filter(|(_, stored_at)| stored_at.elapsed() < self.ttl)
            .map(|(value, _)| value.clone())
    }

    /// Stores an entry, first dropping expired ones and, when still full, the
    /// oldest one.
    pub fn insert(&self, key: K, value: V) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if entries.len() >= self.capacity {
            let ttl = self.ttl;
            entries.retain(|_, (_, stored_at)| stored_at.elapsed() < ttl);
        }
        if entries.len() >= self.capacity
            && let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, (_, stored_at))| *stored_at)
                .map(|(key, _)| key.clone())
        {
            entries.remove(&oldest);
        }
        entries.insert(key, (value, Instant::now()));
    }

    /// Forgets every entry matching `predicate`.
    pub fn remove_where(&self, mut predicate: impl FnMut(&K, &V) -> bool) {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|key, (value, _)| !predicate(key, value));
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{ExpiringMap, TokenBucket};

    #[test]
    fn a_bucket_spends_its_capacity_then_names_the_wait() {
        let bucket = TokenBucket::per_minute(3);
        assert!(bucket.try_take().is_ok());
        assert!(bucket.try_take().is_ok());
        assert!(bucket.try_take().is_ok());
        let wait = bucket.try_take().err();
        assert!(
            wait.is_some_and(
                |wait| wait > Duration::from_secs(10) && wait <= Duration::from_secs(20)
            )
        );
    }

    #[test]
    fn expiring_entries_vanish_and_capacity_evicts_the_oldest() {
        let map = ExpiringMap::new(Duration::from_secs(60), 2);
        map.insert("a", 1);
        map.insert("b", 2);
        map.insert("c", 3);
        assert_eq!(map.get(&"a"), None);
        assert_eq!(map.get(&"b"), Some(2));
        assert_eq!(map.get(&"c"), Some(3));
        map.remove_where(|key, _| *key == "b");
        assert_eq!(map.get(&"b"), None);

        let short = ExpiringMap::new(Duration::ZERO, 2);
        short.insert("a", 1);
        assert_eq!(short.get(&"a"), None);
    }
}
