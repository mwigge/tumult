//! The Star Map response cache: serialised responses kept in process for
//! [`TTL`], so a page that asks again while the user pans, or a second
//! viewer of the same estate, does not read the lake again.
//!
//! The key is built by the handler from the request parameters and the
//! principal's environment scopes; two principals with different scopes
//! never share an entry.
//!
//! Bounds: at most [`MAX_ENTRIES`] responses and [`MAX_BYTES`] of bodies,
//! the oldest dropped first; expired entries go on every read and write.
//! Concurrent misses of one key share a single build
//! ([`StarMapCache::get_or_build`]).
//!
//! Invalidation: every API write that changes what a map shows without
//! new telemetry clears the cache: a risk suppression
//! (`handlers::risks::suppress`), any manual record write (`manual`:
//! create, edit, submit, attach, verify, reject, import), a journal
//! import that stored a run (`import::import_journal`) and a stored
//! performance verdict (`handlers::perf::store`). A build that such a write overtakes is answered
//! to its own caller but never stored (the generation counter), so the
//! write is visible to the next request. Telemetry arriving over OTLP
//! does not clear anything; it shows after at most [`TTL`].
//!
//! Forced refresh: a request carrying `Cache-Control: no-cache` is not
//! allowed to read the lake without limit. It is answered from the cache
//! when the entry is younger than [`REFRESH_FLOOR`], so any number of
//! viewers forcing a refresh cost one lake read per key per floor.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use sha2::{Digest, Sha256};

/// How long a response is served without reading the lake again.
pub(crate) const TTL: Duration = Duration::from_secs(30);
/// The youngest entry a `Cache-Control: no-cache` request may replace.
pub(crate) const REFRESH_FLOOR: Duration = Duration::from_secs(2);
/// Most responses kept at once; the oldest goes first.
const MAX_ENTRIES: usize = 64;
/// Most body bytes kept at once. A response larger than this is served
/// and not kept.
const MAX_BYTES: usize = 64 * 1024 * 1024;
/// Bytes of the body hash the validator carries.
const ETAG_BYTES: usize = 16;

/// One serialised Star Map with its validator.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CachedMap {
    /// A strong `ETag`, quoted: a hash of the body.
    pub(crate) etag: String,
    pub(crate) body: Bytes,
}

impl CachedMap {
    pub(crate) fn new(body: Vec<u8>) -> Self {
        let digest = Sha256::digest(&body);
        let hex: String = digest
            .iter()
            .take(ETAG_BYTES)
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Self {
            etag: format!("\"{hex}\""),
            body: Bytes::from(body),
        }
    }
}

struct Entry {
    stored: Instant,
    map: Arc<CachedMap>,
}

impl Entry {
    fn younger_than(&self, age: Duration, now: Instant) -> bool {
        now.saturating_duration_since(self.stored) < age
    }
}

/// The stored responses with what bounds and invalidates them.
#[derive(Default)]
struct Shelf {
    entries: HashMap<String, Entry>,
    /// Sum of the stored bodies' lengths.
    bytes: usize,
    /// Bumped by every clear; a build started under an older value is
    /// not stored.
    generation: u64,
}

impl Shelf {
    fn remove(&mut self, key: &str) {
        let Some(entry) = self.entries.remove(key) else {
            return;
        };
        self.bytes = self.bytes.saturating_sub(entry.map.body.len());
    }

    fn drop_expired(&mut self, now: Instant) {
        let expired: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, entry)| !entry.younger_than(TTL, now))
            .map(|(key, _)| key.clone())
            .collect();
        for key in expired {
            self.remove(&key);
        }
    }

    /// Drop the oldest entry; `false` when there is none.
    fn drop_oldest(&mut self) -> bool {
        let oldest = self
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.stored)
            .map(|(key, _)| key.clone());
        let Some(oldest) = oldest else {
            return false;
        };
        self.remove(&oldest);
        true
    }
}

/// One build per key at a time: requests for the same key queue on its
/// gate, and all but the first find the response stored.
type Gate = Arc<tokio::sync::Mutex<()>>;

/// A request's place at a key's gate. Dropping it, however the request
/// ends, takes the gate off the table once nobody else waits on it.
struct Flight<'a> {
    cache: &'a StarMapCache,
    key: &'a str,
    gate: Gate,
}

impl Drop for Flight<'_> {
    fn drop(&mut self) {
        let mut flights = self.cache.flights_lock();
        // Gates are only cloned under this lock; two holders are this
        // request and the table.
        if Arc::strong_count(&self.gate) <= 2 {
            flights.remove(self.key);
        }
    }
}

/// Responses by request key. Cheap to share: handlers hold it in an `Arc`.
pub(crate) struct StarMapCache {
    shelf: Mutex<Shelf>,
    flights: Mutex<HashMap<String, Gate>>,
    max_entries: usize,
    max_bytes: usize,
}

impl Default for StarMapCache {
    fn default() -> Self {
        Self::with_limits(MAX_ENTRIES, MAX_BYTES)
    }
}

impl StarMapCache {
    fn with_limits(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            shelf: Mutex::default(),
            flights: Mutex::default(),
            max_entries,
            max_bytes,
        }
    }

    /// The shelf. No change to it can panic half-way, so a poisoned lock
    /// is still good to use.
    fn lock(&self) -> MutexGuard<'_, Shelf> {
        self.shelf.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn flights_lock(&self) -> MutexGuard<'_, HashMap<String, Gate>> {
        self.flights.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The response stored for `key`, while it is younger than `max_age`.
    /// Every read drops the entries older than [`TTL`].
    pub(crate) fn get(&self, key: &str, max_age: Duration, now: Instant) -> Option<Arc<CachedMap>> {
        let mut shelf = self.lock();
        shelf.drop_expired(now);
        let entry = shelf.entries.get(key)?;
        entry
            .younger_than(max_age, now)
            .then(|| Arc::clone(&entry.map))
    }

    /// The value to hand back to [`Self::put`]: read it before the lake.
    pub(crate) fn generation(&self) -> u64 {
        self.lock().generation
    }

    /// Store a response read under `generation`, making room by dropping
    /// what has expired and then the oldest entries. Returns whether it
    /// was stored: not when the cache was cleared since `generation` was
    /// read, and not when the body alone exceeds the byte cap.
    pub(crate) fn put(
        &self,
        key: String,
        map: Arc<CachedMap>,
        now: Instant,
        generation: u64,
    ) -> bool {
        let size = map.body.len();
        if size > self.max_bytes {
            tracing::warn!(
                bytes = size,
                cap = self.max_bytes,
                "star map response is larger than the cache; not kept"
            );
            return false;
        }
        let mut shelf = self.lock();
        if shelf.generation != generation {
            return false;
        }
        shelf.drop_expired(now);
        shelf.remove(&key);
        while shelf.entries.len() >= self.max_entries || shelf.bytes + size > self.max_bytes {
            if !shelf.drop_oldest() {
                break;
            }
        }
        shelf.bytes += size;
        shelf.entries.insert(key, Entry { stored: now, map });
        true
    }

    /// Forget every response: something a map shows was just written.
    /// Builds already reading the lake will not be stored.
    pub(crate) fn clear(&self) {
        let mut shelf = self.lock();
        shelf.entries.clear();
        shelf.bytes = 0;
        shelf.generation = shelf.generation.wrapping_add(1);
    }

    /// Join the queue at `key`'s gate.
    fn flight_for<'a>(&'a self, key: &'a str) -> Flight<'a> {
        let mut flights = self.flights_lock();
        let gate = Arc::clone(flights.entry(key.to_string()).or_default());
        Flight {
            cache: self,
            key,
            gate,
        }
    }

    /// The response for `key`: the stored one while it is younger than
    /// `max_age`, else the one `build` returns, stored unless a clear
    /// overtook it. Concurrent callers for one key run `build` once; the
    /// others wait and take what it stored.
    pub(crate) async fn get_or_build<F, Fut, E>(
        &self,
        key: &str,
        max_age: Duration,
        build: F,
    ) -> Result<Arc<CachedMap>, E>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<CachedMap, E>>,
    {
        if let Some(map) = self.get(key, max_age, Instant::now()) {
            return Ok(map);
        }
        let flight = self.flight_for(key);
        let _turn = flight.gate.lock().await;
        if let Some(map) = self.get(key, max_age, Instant::now()) {
            return Ok(map);
        }
        let generation = self.generation();
        let map = Arc::new(build().await?);
        self.put(
            key.to_string(),
            Arc::clone(&map),
            Instant::now(),
            generation,
        );
        Ok(map)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.lock().entries.len()
    }

    #[cfg(test)]
    fn bytes(&self) -> usize {
        self.lock().bytes
    }

    #[cfg(test)]
    fn flights(&self) -> usize {
        self.flights_lock().len()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn map(body: &str) -> Arc<CachedMap> {
        Arc::new(CachedMap::new(body.as_bytes().to_vec()))
    }

    /// A build that counts how often it ran.
    async fn counted(runs: &AtomicUsize, body: &str) -> Result<CachedMap, String> {
        runs.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(20)).await;
        Ok(CachedMap::new(body.as_bytes().to_vec()))
    }

    /// Sixteen requests for `k` at once; every answer.
    async fn ask_at_once(
        cache: &Arc<StarMapCache>,
        runs: &Arc<AtomicUsize>,
        max_age: Duration,
        body: &'static str,
    ) -> Vec<Arc<CachedMap>> {
        let mut asks = tokio::task::JoinSet::new();
        for _ in 0..16 {
            let (cache, runs) = (Arc::clone(cache), Arc::clone(runs));
            asks.spawn(async move {
                let asked = cache.get_or_build("k", max_age, || counted(&runs, body));
                asked.await.unwrap()
            });
        }
        asks.join_all().await
    }

    #[test]
    fn the_validator_is_a_quoted_hash_of_the_body() {
        let one = CachedMap::new(b"{\"nodes\":[]}".to_vec());
        let same = CachedMap::new(b"{\"nodes\":[]}".to_vec());
        let other = CachedMap::new(b"{\"nodes\":[1]}".to_vec());
        assert_eq!(one.etag, same.etag);
        assert_ne!(one.etag, other.etag);
        assert_eq!(one.etag.len(), 2 * ETAG_BYTES + 2);
        assert!(one.etag.starts_with('"') && one.etag.ends_with('"'));
        assert_eq!(&one.body[..], b"{\"nodes\":[]}");
    }

    #[test]
    fn an_entry_is_served_for_thirty_seconds_and_no_longer() {
        let cache = StarMapCache::default();
        let start = Instant::now();
        cache.put("k".into(), map("a"), start, cache.generation());
        let just_inside = start + TTL - Duration::from_millis(1);
        assert_eq!(cache.get("k", TTL, start), Some(map("a")));
        assert_eq!(cache.get("k", TTL, just_inside), Some(map("a")));
        assert_eq!(cache.get("k", TTL, start + TTL), None);
        assert_eq!(cache.get("other", TTL, start), None);
    }

    #[test]
    fn a_newer_response_replaces_the_stored_one() {
        let cache = StarMapCache::default();
        let start = Instant::now();
        cache.put("k".into(), map("a"), start, cache.generation());
        let later = start + Duration::from_secs(40);
        cache.put("k".into(), map("bb"), later, cache.generation());
        assert_eq!(
            cache.get("k", TTL, start + Duration::from_secs(60)),
            Some(map("bb"))
        );
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.bytes(), 2);
    }

    #[test]
    fn the_cache_is_bounded_and_drops_the_oldest_first() {
        let cache = StarMapCache::default();
        let start = Instant::now();
        for i in 0..MAX_ENTRIES {
            let at = start + Duration::from_millis(i as u64);
            cache.put(format!("k{i}"), map("a"), at, cache.generation());
        }
        let later = start + Duration::from_secs(1);
        cache.put("new".into(), map("b"), later, cache.generation());
        assert_eq!(cache.len(), MAX_ENTRIES);
        assert_eq!(cache.get("k0", TTL, later), None, "the oldest went");
        assert!(cache.get("k1", TTL, later).is_some());
        assert!(cache.get("new", TTL, later).is_some());

        // Expired entries make room before anything live is dropped.
        let expired = start + TTL + Duration::from_secs(2);
        cache.put("fresh".into(), map("c"), expired, cache.generation());
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn the_bodies_held_never_exceed_the_byte_cap() {
        let cache = StarMapCache::with_limits(MAX_ENTRIES, 10);
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        cache.put("a".into(), map("1234"), at(0), cache.generation());
        cache.put("b".into(), map("5678"), at(1), cache.generation());
        assert_eq!(cache.bytes(), 8);

        // Four more bytes do not fit: the oldest goes, not the newest.
        cache.put("c".into(), map("9abc"), at(2), cache.generation());
        assert_eq!(cache.bytes(), 8);
        assert_eq!(cache.get("a", TTL, at(3)), None);
        assert!(cache.get("b", TTL, at(3)).is_some());
        assert!(cache.get("c", TTL, at(3)).is_some());

        // Replacing a key counts the new body, not both.
        cache.put("c".into(), map("9abcde"), at(4), cache.generation());
        assert_eq!(cache.bytes(), 10);
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn a_body_larger_than_the_cap_is_served_but_not_kept() {
        let cache = StarMapCache::with_limits(MAX_ENTRIES, 10);
        let start = Instant::now();
        cache.put("small".into(), map("1234"), start, cache.generation());
        let kept = cache.put("huge".into(), map("0123456789a"), start, cache.generation());
        assert!(!kept);
        assert_eq!(cache.get("huge", TTL, start), None);
        assert!(cache.get("small", TTL, start).is_some(), "nothing evicted");
        assert_eq!(cache.bytes(), 4);
    }

    #[test]
    fn reading_evicts_what_has_expired() {
        let cache = StarMapCache::default();
        let start = Instant::now();
        cache.put("old".into(), map("aaaa"), start, cache.generation());
        let later = start + Duration::from_secs(20);
        cache.put("young".into(), map("bb"), later, cache.generation());
        assert_eq!(cache.len(), 2);

        // A read of any key, hit or miss, drops every expired entry.
        assert_eq!(cache.get("absent", TTL, start + TTL), None);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.bytes(), 2);
        assert!(cache.get("young", TTL, start + TTL).is_some());
    }

    #[test]
    fn a_shorter_age_limit_does_not_evict_what_the_ttl_still_allows() {
        let cache = StarMapCache::default();
        let start = Instant::now();
        cache.put("k".into(), map("a"), start, cache.generation());
        let after_floor = start + REFRESH_FLOOR;
        assert_eq!(cache.get("k", REFRESH_FLOOR, after_floor), None);
        assert_eq!(cache.get("k", TTL, after_floor), Some(map("a")));
        let inside = start + REFRESH_FLOOR - Duration::from_millis(1);
        assert_eq!(cache.get("k", REFRESH_FLOOR, inside), Some(map("a")));
    }

    #[test]
    fn clear_forgets_everything() {
        let cache = StarMapCache::default();
        let start = Instant::now();
        cache.put("k".into(), map("a"), start, cache.generation());
        cache.clear();
        assert_eq!(cache.get("k", TTL, start), None);
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.bytes(), 0);
    }

    #[test]
    fn a_response_read_before_a_clear_is_not_stored_after_it() {
        let cache = StarMapCache::default();
        let start = Instant::now();
        let before = cache.generation();
        cache.clear();
        assert_ne!(cache.generation(), before);
        assert!(!cache.put("k".into(), map("stale"), start, before));
        assert_eq!(cache.get("k", TTL, start), None);
        assert!(cache.put("k".into(), map("new"), start, cache.generation()));
    }

    #[tokio::test]
    async fn a_build_overtaken_by_a_write_is_answered_but_not_cached() {
        let cache = StarMapCache::default();
        // Request A misses and reads the lake; the write commits and
        // clears while A is still assembling; A then finishes.
        let stale = cache
            .get_or_build("k", TTL, || async {
                cache.clear();
                Ok::<_, String>(CachedMap::new(b"before the write".to_vec()))
            })
            .await
            .unwrap();
        assert_eq!(&stale.body[..], b"before the write");
        assert_eq!(cache.get("k", TTL, Instant::now()), None);

        // The next request reads the lake again.
        let runs = AtomicUsize::new(0);
        let next = cache
            .get_or_build("k", TTL, || counted(&runs, "after the write"))
            .await
            .unwrap();
        assert_eq!(&next.body[..], b"after the write");
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn concurrent_misses_of_one_key_build_once() {
        let cache = Arc::new(StarMapCache::default());
        let runs = Arc::new(AtomicUsize::new(0));
        let answers = ask_at_once(&cache, &runs, TTL, "a").await;
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        assert_eq!(answers.len(), 16);
        assert!(answers.iter().all(|answer| answer.body == "a"));
        assert_eq!(cache.flights(), 0, "no flight outlives its requests");

        // Another key is another build, and does not wait for the first.
        let other = cache.get_or_build("other", TTL, || counted(&runs, "b"));
        assert_eq!(other.await.unwrap().body, "b");
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_forced_refresh_is_answered_from_an_entry_younger_than_the_floor() {
        let cache = Arc::new(StarMapCache::default());
        let runs = Arc::new(AtomicUsize::new(0));
        for _ in 0..5 {
            let asked = cache.get_or_build("k", REFRESH_FLOOR, || counted(&runs, "a"));
            asked.await.unwrap();
        }
        assert_eq!(runs.load(Ordering::SeqCst), 1);

        // Sixteen viewers forcing a refresh at once share one lake read.
        cache.clear();
        ask_at_once(&cache, &runs, REFRESH_FLOOR, "b").await;
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_failed_build_stores_nothing_and_frees_the_key() {
        let cache = StarMapCache::default();
        let failed = cache
            .get_or_build("k", TTL, || async {
                Err::<CachedMap, _>("lake gone".to_string())
            })
            .await;
        assert_eq!(failed.unwrap_err(), "lake gone");
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.flights(), 0);

        let runs = AtomicUsize::new(0);
        let retried = cache.get_or_build("k", TTL, || counted(&runs, "a"));
        assert_eq!(retried.await.unwrap().body, "a");
    }

    #[tokio::test]
    async fn an_abandoned_build_frees_the_key() {
        let cache = Arc::new(StarMapCache::default());
        let held = Arc::clone(&cache);
        let abandoned = tokio::spawn(async move {
            held.get_or_build("k", TTL, || async {
                std::future::pending::<Result<CachedMap, String>>().await
            })
            .await
        });
        while cache.flights() == 0 {
            tokio::task::yield_now().await;
        }
        abandoned.abort();
        assert!(abandoned.await.unwrap_err().is_cancelled());
        assert_eq!(cache.flights(), 0);

        let runs = AtomicUsize::new(0);
        let next = cache.get_or_build("k", TTL, || counted(&runs, "a"));
        assert_eq!(next.await.unwrap().body, "a");
    }
}
