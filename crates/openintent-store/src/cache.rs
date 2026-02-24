//! Hot cache layer using [`moka`].
//!
//! Wraps common database queries with a lock-free, concurrent LRU cache.
//! Supports caching any `Serialize + DeserializeOwned` type via a generic
//! [`CacheLayer`] struct.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use moka::future::Cache;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tracing::debug;

use crate::error::{StoreError, StoreResult};

// ── cache stats ──────────────────────────────────────────────────────

/// Counters tracking cache effectiveness.
#[derive(Debug)]
pub struct CacheStats {
    hits: AtomicU64,
    misses: AtomicU64,
}

impl CacheStats {
    fn new() -> Self {
        Self {
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    fn record_hit(&self) {
        self.hits.fetch_add(1, Ordering::Relaxed);
    }

    fn record_miss(&self) {
        self.misses.fetch_add(1, Ordering::Relaxed);
    }

    /// Total cache hits since creation.
    pub fn hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }

    /// Total cache misses since creation.
    pub fn misses(&self) -> u64 {
        self.misses.load(Ordering::Relaxed)
    }

    /// Total lookups (hits + misses).
    pub fn total(&self) -> u64 {
        self.hits() + self.misses()
    }

    /// Hit rate as a value between 0.0 and 1.0 (returns 0.0 if no lookups).
    pub fn hit_rate(&self) -> f64 {
        let total = self.total();
        if total == 0 {
            return 0.0;
        }
        self.hits() as f64 / total as f64
    }

    /// Reset all counters to zero.
    pub fn reset(&self) {
        self.hits.store(0, Ordering::Relaxed);
        self.misses.store(0, Ordering::Relaxed);
    }
}

impl fmt::Display for CacheStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "hits={} misses={} total={} rate={:.2}%",
            self.hits(),
            self.misses(),
            self.total(),
            self.hit_rate() * 100.0,
        )
    }
}

// ── cache layer ──────────────────────────────────────────────────────

/// A generic, async-aware cache backed by `moka::future::Cache`.
///
/// `T` must be `Serialize + DeserializeOwned + Clone + Send + Sync`.
/// Values are stored as JSON strings internally so any serializable
/// type can share the same cache infrastructure.
///
/// # Example
///
/// ```ignore
/// use openintent_store::cache::CacheLayer;
///
/// let cache: CacheLayer<MyRecord> = CacheLayer::builder("my_records")
///     .max_capacity(1000)
///     .ttl_seconds(300)
///     .build();
///
/// // Try the cache first, fall back to DB.
/// let record = match cache.get("record:123").await {
///     Some(r) => r,
///     None => {
///         let r = fetch_from_db(123).await?;
///         cache.insert("record:123", &r).await?;
///         r
///     }
/// };
/// ```
pub struct CacheLayer<T> {
    name: &'static str,
    inner: Cache<String, String>,
    stats: Arc<CacheStats>,
    _marker: std::marker::PhantomData<T>,
}

impl<T> Clone for CacheLayer<T> {
    fn clone(&self) -> Self {
        Self {
            name: self.name,
            inner: self.inner.clone(),
            stats: Arc::clone(&self.stats),
            _marker: std::marker::PhantomData,
        }
    }
}

impl<T> CacheLayer<T>
where
    T: Serialize + DeserializeOwned + Clone + Send + Sync + 'static,
{
    /// Start building a new cache layer.
    pub fn builder(name: &'static str) -> CacheLayerBuilder<T> {
        CacheLayerBuilder {
            name,
            max_capacity: 10_000,
            ttl_seconds: 300,
            _marker: std::marker::PhantomData,
        }
    }

    /// Look up a cached value by key. Returns `None` on miss.
    pub async fn get(&self, key: &str) -> Option<T> {
        match self.inner.get(key).await {
            Some(json_str) => match serde_json::from_str::<T>(&json_str) {
                Ok(val) => {
                    self.stats.record_hit();
                    debug!(cache = self.name, key = key, "cache hit");
                    Some(val)
                }
                Err(err) => {
                    // Corrupted entry — evict and treat as miss.
                    tracing::warn!(
                        cache = self.name,
                        key = key,
                        %err,
                        "cache entry deserialization failed, evicting"
                    );
                    self.inner.invalidate(key).await;
                    self.stats.record_miss();
                    None
                }
            },
            None => {
                self.stats.record_miss();
                debug!(cache = self.name, key = key, "cache miss");
                None
            }
        }
    }

    /// Insert a value into the cache.
    pub async fn insert(&self, key: &str, value: &T) -> StoreResult<()> {
        let json_str =
            serde_json::to_string(value).map_err(|e| StoreError::Cache(e.to_string()))?;
        self.inner.insert(key.to_string(), json_str).await;
        debug!(cache = self.name, key = key, "cache insert");
        Ok(())
    }

    /// Remove a specific entry from the cache.
    pub async fn invalidate(&self, key: &str) {
        self.inner.invalidate(key).await;
        debug!(cache = self.name, key = key, "cache invalidate");
    }

    /// Remove all entries from the cache.
    pub async fn invalidate_all(&self) {
        self.inner.invalidate_all();
        debug!(cache = self.name, "cache invalidate_all");
    }

    /// Get a reference to the cache statistics.
    pub fn stats(&self) -> &CacheStats {
        &self.stats
    }

    /// Return the current number of entries in the cache.
    pub fn entry_count(&self) -> u64 {
        self.inner.entry_count()
    }

    /// Get or insert: try the cache, and if missing, call the async loader,
    /// cache the result, and return it.
    ///
    /// This is the preferred way to use the cache — single call that handles
    /// the hit/miss/insert flow automatically.
    pub async fn get_or_insert_with<F, Fut>(
        &self,
        key: &str,
        loader: F,
    ) -> StoreResult<T>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = StoreResult<T>>,
    {
        if let Some(cached) = self.get(key).await {
            return Ok(cached);
        }

        let value = loader().await?;
        self.insert(key, &value).await?;
        Ok(value)
    }
}

// ── builder ──────────────────────────────────────────────────────────

/// Builder for [`CacheLayer`].
pub struct CacheLayerBuilder<T> {
    name: &'static str,
    max_capacity: u64,
    ttl_seconds: u64,
    _marker: std::marker::PhantomData<T>,
}

impl<T> CacheLayerBuilder<T>
where
    T: Serialize + DeserializeOwned + Clone + Send + Sync + 'static,
{
    /// Maximum number of entries the cache will hold.
    pub fn max_capacity(mut self, cap: u64) -> Self {
        self.max_capacity = cap;
        self
    }

    /// Time-to-live in seconds for each entry.
    pub fn ttl_seconds(mut self, secs: u64) -> Self {
        self.ttl_seconds = secs;
        self
    }

    /// Build the cache layer.
    pub fn build(self) -> CacheLayer<T> {
        let inner = Cache::builder()
            .max_capacity(self.max_capacity)
            .time_to_live(Duration::from_secs(self.ttl_seconds))
            .build();

        debug!(
            name = self.name,
            max_capacity = self.max_capacity,
            ttl_seconds = self.ttl_seconds,
            "cache layer created"
        );

        CacheLayer {
            name: self.name,
            inner,
            stats: Arc::new(CacheStats::new()),
            _marker: std::marker::PhantomData,
        }
    }
}

// ── tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct TestRecord {
        id: u64,
        name: String,
    }

    fn make_cache() -> CacheLayer<TestRecord> {
        CacheLayer::builder("test")
            .max_capacity(100)
            .ttl_seconds(60)
            .build()
    }

    #[tokio::test]
    async fn insert_and_get() {
        let cache = make_cache();
        let record = TestRecord {
            id: 1,
            name: "Alice".to_string(),
        };

        cache.insert("rec:1", &record).await.unwrap();
        let cached = cache.get("rec:1").await;
        assert_eq!(cached, Some(record));
    }

    #[tokio::test]
    async fn miss_returns_none() {
        let cache = make_cache();
        let cached = cache.get("nonexistent").await;
        assert_eq!(cached, None);
    }

    #[tokio::test]
    async fn invalidate_removes_entry() {
        let cache = make_cache();
        let record = TestRecord {
            id: 2,
            name: "Bob".to_string(),
        };

        cache.insert("rec:2", &record).await.unwrap();
        cache.invalidate("rec:2").await;
        let cached = cache.get("rec:2").await;
        assert_eq!(cached, None);
    }

    #[tokio::test]
    async fn stats_tracking() {
        let cache = make_cache();
        let record = TestRecord {
            id: 3,
            name: "Carol".to_string(),
        };

        cache.insert("rec:3", &record).await.unwrap();

        // 1 hit
        let _ = cache.get("rec:3").await;
        // 2 misses
        let _ = cache.get("nope1").await;
        let _ = cache.get("nope2").await;

        assert_eq!(cache.stats().hits(), 1);
        assert_eq!(cache.stats().misses(), 2);
        assert_eq!(cache.stats().total(), 3);
        assert!((cache.stats().hit_rate() - (1.0 / 3.0)).abs() < 0.01);
    }

    #[tokio::test]
    async fn get_or_insert_with_caches() {
        let cache = make_cache();

        // First call: loader is invoked.
        let val = cache
            .get_or_insert_with("rec:4", || async {
                Ok(TestRecord {
                    id: 4,
                    name: "Dave".to_string(),
                })
            })
            .await
            .unwrap();
        assert_eq!(val.name, "Dave");

        // Second call: should come from cache (we can't easily track
        // the loader call with the current API, but stats prove it).
        let val2 = cache
            .get_or_insert_with("rec:4", || async {
                // This should NOT be called.
                Ok(TestRecord {
                    id: 999,
                    name: "Should not appear".to_string(),
                })
            })
            .await
            .unwrap();
        assert_eq!(val2.name, "Dave");
        // One miss (first call), one hit (second call).
        assert_eq!(cache.stats().hits(), 1);
        assert_eq!(cache.stats().misses(), 1);
    }

    #[tokio::test]
    async fn invalidate_all_clears_cache() {
        let cache = make_cache();
        cache
            .insert(
                "a",
                &TestRecord {
                    id: 1,
                    name: "a".into(),
                },
            )
            .await
            .unwrap();
        cache
            .insert(
                "b",
                &TestRecord {
                    id: 2,
                    name: "b".into(),
                },
            )
            .await
            .unwrap();

        cache.invalidate_all().await;

        // moka's invalidate_all is lazy; run_pending_tasks may be needed.
        // But get() should still return None for invalidated entries.
        assert!(cache.get("a").await.is_none());
        assert!(cache.get("b").await.is_none());
    }

    #[test]
    fn stats_display() {
        let stats = CacheStats::new();
        stats.hits.store(50, Ordering::Relaxed);
        stats.misses.store(50, Ordering::Relaxed);
        let display = format!("{stats}");
        assert!(display.contains("hits=50"));
        assert!(display.contains("misses=50"));
        assert!(display.contains("50.00%"));
    }

    #[test]
    fn stats_reset() {
        let stats = CacheStats::new();
        stats.hits.store(10, Ordering::Relaxed);
        stats.misses.store(5, Ordering::Relaxed);
        stats.reset();
        assert_eq!(stats.hits(), 0);
        assert_eq!(stats.misses(), 0);
    }

    #[test]
    fn hit_rate_no_lookups() {
        let stats = CacheStats::new();
        assert_eq!(stats.hit_rate(), 0.0);
    }
}
