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
//!
//! One build per key ([`StarMapCache::get_or_build`]): concurrent
//! requests for one key queue, the first reads the lake and the others
//! take what it stored. When that build could not be stored (a write
//! overtook it, or the body is larger than the cache) the requests that
//! waited for it are handed its result all the same, provided the build
//! began no earlier than they arrived in terms of writes (its
//! generation is at least theirs) and its age is one they accept. A
//! request that arrived after a write is never handed a view from
//! before it.
//!
//! Load limits, over all keys and callers: at most [`MAX_BUILDS`] lake
//! reads run at once, at most [`MAX_WAITING`] requests wait for a
//! build, and none waits longer than [`MAX_WAIT`]. A request beyond
//! those limits is refused ([`Miss::Busy`], `503` with `Retry-After`),
//! it does not queue without bound.
//!
//! Invalidation: every API write that changes what a map shows without
//! new telemetry clears the cache: a risk suppression
//! (`handlers::risks::suppress`), any manual record write (`manual`:
//! create, edit, submit, attach, verify, reject, import), a journal
//! import that stored a run (`import::import_journal`) and a stored
//! performance verdict (`handlers::perf::store`). A build that such a
//! write overtakes is answered to its own caller, and to those who
//! asked before the write, but never stored (the generation counter),
//! so the write is visible to the next request. Telemetry arriving over
//! OTLP does not clear anything; it shows after at most [`TTL`].
//!
//! Forced refresh: a request carrying `Cache-Control: no-cache` is not
//! allowed to read the lake without limit. It is answered from the cache
//! when the entry is younger than [`REFRESH_FLOOR`], so any number of
//! viewers forcing a refresh of one map cost one lake read per floor.
//! That floor is per key, and a caller chooses its keys (`focus`,
//! `budget`, ...), so what bounds forced reads of many maps is the
//! limit above: never more than [`MAX_BUILDS`] reads at once, whoever
//! asks and for whatever key.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use sha2::{Digest, Sha256};
use tokio::sync::{Semaphore, SemaphorePermit};

/// How long a response is served without reading the lake again.
pub(crate) const TTL: Duration = Duration::from_secs(30);
/// The youngest entry a `Cache-Control: no-cache` request may replace.
pub(crate) const REFRESH_FLOOR: Duration = Duration::from_secs(2);
/// What a refused request is told to wait before it asks again.
pub(crate) const RETRY_AFTER: Duration = Duration::from_secs(2);
/// Most responses kept at once; the oldest goes first.
const MAX_ENTRIES: usize = 64;
/// Most body bytes kept at once. A response larger than this is served
/// and not kept.
const MAX_BYTES: usize = 64 * 1024 * 1024;
/// Most lake reads for maps at once, over all keys and callers.
const MAX_BUILDS: usize = 4;
/// Most requests waiting for a build at once, over all keys.
const MAX_WAITING: usize = 64;
/// The longest a request waits for its turn to build.
const MAX_WAIT: Duration = Duration::from_secs(10);
/// Bytes of the body hash the validator carries.
const ETAG_BYTES: usize = 16;

/// Where the cache reads the time: the system's, unless a test moves it.
pub(crate) type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

/// What bounds the cache and the load it lets through.
#[derive(Debug, Clone, Copy)]
struct Limits {
    max_entries: usize,
    max_bytes: usize,
    /// Builds running at once.
    builds: usize,
    /// Requests waiting for a build at once.
    max_waiting: usize,
    /// The longest one request waits.
    max_wait: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_entries: MAX_ENTRIES,
            max_bytes: MAX_BYTES,
            builds: MAX_BUILDS,
            max_waiting: MAX_WAITING,
            max_wait: MAX_WAIT,
        }
    }
}

/// Why a request got no map.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Miss<E> {
    /// Too many requests wait for a build, or this one waited too long.
    Busy,
    /// Its build failed.
    Failed(E),
}

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

/// The last build at a key, kept for the requests that waited for it.
struct Handed {
    /// The cache's generation when the build began.
    generation: u64,
    finished: Instant,
    map: Arc<CachedMap>,
}

/// The queue at one key: requests for it build one at a time, and the
/// last build's result is passed down the queue.
#[derive(Default)]
struct Flight {
    gate: tokio::sync::Mutex<()>,
    last: Mutex<Option<Handed>>,
}

impl Flight {
    fn last(&self) -> MutexGuard<'_, Option<Handed>> {
        self.last.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn hand(&self, handed: Handed) {
        *self.last() = Some(handed);
    }

    /// The last build, for a request that arrived under generation
    /// `arrived` and accepts a response younger than `max_age`: only a
    /// build that began under that generation or a later one, so that
    /// no write the request could have seen is missing from it.
    fn handed(&self, arrived: u64, max_age: Duration, now: Instant) -> Option<Arc<CachedMap>> {
        let last = self.last();
        let handed = last.as_ref()?;
        let young = now.saturating_duration_since(handed.finished) < max_age;
        (handed.generation >= arrived && young).then(|| Arc::clone(&handed.map))
    }
}

/// A request's place in a key's queue. Dropping it, however the request
/// ends, takes the queue off the table once nobody else is in it.
struct Seat<'a> {
    cache: &'a StarMapCache,
    key: &'a str,
    flight: Arc<Flight>,
}

impl Drop for Seat<'_> {
    fn drop(&mut self) {
        let mut flights = self.cache.flights_lock();
        // Flights are only cloned under this lock; two holders are this
        // request and the table.
        if Arc::strong_count(&self.flight) <= 2 {
            flights.remove(self.key);
        }
    }
}

/// A request counted as waiting for a build, until this is dropped.
struct Waiting<'a>(&'a AtomicUsize);

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// What a request's turn in the queue came to.
enum Turn<'a> {
    /// Somebody else's build answers it.
    Served(Arc<CachedMap>),
    /// It builds, holding its key's gate and one of the build permits.
    Build(tokio::sync::MutexGuard<'a, ()>, SemaphorePermit<'a>),
    /// The cache takes no more builds.
    Closed,
}

/// Responses by request key. Cheap to share: handlers hold it in an `Arc`.
pub(crate) struct StarMapCache {
    shelf: Mutex<Shelf>,
    flights: Mutex<HashMap<String, Arc<Flight>>>,
    builds: Semaphore,
    waiting: AtomicUsize,
    limits: Limits,
    clock: Clock,
}

impl Default for StarMapCache {
    fn default() -> Self {
        Self::with_clock(Arc::new(Instant::now))
    }
}

impl StarMapCache {
    /// The cache with its production limits, reading the time from `clock`.
    pub(crate) fn with_clock(clock: Clock) -> Self {
        Self::new(Limits::default(), clock)
    }

    fn new(limits: Limits, clock: Clock) -> Self {
        Self {
            shelf: Mutex::default(),
            flights: Mutex::default(),
            builds: Semaphore::new(limits.builds),
            waiting: AtomicUsize::new(0),
            limits,
            clock,
        }
    }

    #[cfg(test)]
    fn with_limits(limits: Limits) -> Self {
        Self::new(limits, Arc::new(Instant::now))
    }

    fn now(&self) -> Instant {
        (self.clock)()
    }

    /// The shelf. No change to it can panic half-way, so a poisoned lock
    /// is still good to use.
    fn lock(&self) -> MutexGuard<'_, Shelf> {
        self.shelf.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn flights_lock(&self) -> MutexGuard<'_, HashMap<String, Arc<Flight>>> {
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
        if size > self.limits.max_bytes {
            tracing::warn!(
                bytes = size,
                cap = self.limits.max_bytes,
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
        while shelf.entries.len() >= self.limits.max_entries
            || shelf.bytes + size > self.limits.max_bytes
        {
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

    /// Join the queue at `key`.
    fn seat<'a>(&'a self, key: &'a str) -> Seat<'a> {
        let mut flights = self.flights_lock();
        let flight = Arc::clone(flights.entry(key.to_string()).or_default());
        Seat {
            cache: self,
            key,
            flight,
        }
    }

    /// Count one more request as waiting; `None` when the line is full.
    fn wait_in_line(&self) -> Option<Waiting<'_>> {
        let waiting = self.waiting.fetch_add(1, Ordering::SeqCst) + 1;
        let line = Waiting(&self.waiting);
        if waiting > self.limits.max_waiting {
            tracing::warn!(
                waiting,
                cap = self.limits.max_waiting,
                "star map request refused: too many wait for a build"
            );
            return None;
        }
        Some(line)
    }

    /// Wait at the key's gate, then take what the build before left for
    /// this request, or else wait for a build permit.
    async fn turn<'a>(&'a self, seat: &'a Seat<'_>, arrived: u64, max_age: Duration) -> Turn<'a> {
        let gate = seat.flight.gate.lock().await;
        let now = self.now();
        if let Some(map) = self.get(seat.key, max_age, now) {
            return Turn::Served(map);
        }
        if let Some(map) = seat.flight.handed(arrived, max_age, now) {
            return Turn::Served(map);
        }
        match self.builds.acquire().await {
            Ok(permit) => Turn::Build(gate, permit),
            Err(_) => Turn::Closed,
        }
    }

    /// The response for `key`: the stored one while it is younger than
    /// `max_age`, else the one `build` returns, stored unless a clear
    /// overtook it. Concurrent callers for one key run `build` once; the
    /// others wait and take what it stored or, when it was not stored,
    /// what it returned (see the module text for when). `build` is
    /// called only once this request's turn has come, so what it reads
    /// is as of then. [`Miss::Busy`] when too many requests wait or this
    /// one waited [`MAX_WAIT`].
    pub(crate) async fn get_or_build<F, Fut, E>(
        &self,
        key: &str,
        max_age: Duration,
        build: F,
    ) -> Result<Arc<CachedMap>, Miss<E>>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<CachedMap, E>>,
    {
        if let Some(map) = self.get(key, max_age, self.now()) {
            return Ok(map);
        }
        let arrived = self.generation();
        let line = self.wait_in_line().ok_or(Miss::Busy)?;
        let seat = self.seat(key);
        let turn = self.turn(&seat, arrived, max_age);
        let Ok(turn) = tokio::time::timeout(self.limits.max_wait, turn).await else {
            tracing::warn!(
                waited_ms = self.limits.max_wait.as_millis(),
                "star map request refused: it waited too long for a build"
            );
            return Err(Miss::Busy);
        };
        let _held = match turn {
            Turn::Served(map) => return Ok(map),
            Turn::Closed => return Err(Miss::Busy),
            Turn::Build(gate, permit) => (gate, permit),
        };
        drop(line);
        let generation = self.generation();
        let map = Arc::new(build().await.map_err(Miss::Failed)?);
        let finished = self.now();
        self.put(key.to_string(), Arc::clone(&map), finished, generation);
        seat.flight.hand(Handed {
            generation,
            finished,
            map: Arc::clone(&map),
        });
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

    #[cfg(test)]
    fn waiting(&self) -> usize {
        self.waiting.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    use tokio::sync::Notify;

    use super::*;

    /// A cache that keeps nothing: every body is larger than its cap.
    fn keeps_nothing() -> Limits {
        Limits {
            max_bytes: 0,
            ..Limits::default()
        }
    }

    /// A clock the test moves: the cache's time is the real one plus
    /// the milliseconds in the returned counter.
    fn moved_clock() -> (Clock, Arc<AtomicU64>) {
        let ahead = Arc::new(AtomicU64::new(0));
        let read = Arc::clone(&ahead);
        let clock: Clock =
            Arc::new(move || Instant::now() + Duration::from_millis(read.load(Ordering::SeqCst)));
        (clock, ahead)
    }

    fn body(text: &str) -> Result<CachedMap, String> {
        Ok(CachedMap::new(text.as_bytes().to_vec()))
    }

    /// Poll `future` once: its output when it is ready already.
    async fn poll_once<F: Future + Unpin>(future: &mut F) -> Option<F::Output> {
        tokio::select! {
            biased;
            output = future => Some(output),
            () = std::future::ready(()) => None,
        }
    }

    /// Room for ten bytes of bodies.
    fn small() -> Limits {
        Limits {
            max_bytes: 10,
            ..Limits::default()
        }
    }

    fn map(body: &str) -> Arc<CachedMap> {
        Arc::new(CachedMap::new(body.as_bytes().to_vec()))
    }

    /// A build that counts how often it ran.
    async fn counted(runs: &AtomicUsize, body: &str) -> Result<CachedMap, String> {
        runs.fetch_add(1, Ordering::SeqCst);
        // sleep: stands in for a lake read, so requests made together overlap
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
        let cache = StarMapCache::with_limits(small());
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
        let cache = StarMapCache::with_limits(small());
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
    async fn a_forced_refresh_past_the_floor_reads_again() {
        let (clock, ahead) = moved_clock();
        let cache = StarMapCache::new(Limits::default(), clock);
        let runs = AtomicUsize::new(0);
        let first = cache.get_or_build("k", REFRESH_FLOOR, || counted(&runs, "a"));
        assert_eq!(first.await.unwrap().body, "a");

        ahead.store(1_900, Ordering::SeqCst);
        let inside = cache.get_or_build("k", REFRESH_FLOOR, || counted(&runs, "b"));
        assert_eq!(inside.await.unwrap().body, "a", "younger than the floor");
        assert_eq!(runs.load(Ordering::SeqCst), 1);

        ahead.store(2_100, Ordering::SeqCst);
        let past = cache.get_or_build("k", REFRESH_FLOOR, || counted(&runs, "b"));
        assert_eq!(past.await.unwrap().body, "b");
        assert_eq!(runs.load(Ordering::SeqCst), 2);

        // A plain request is then served what the forced one read.
        let plain = cache.get_or_build("k", TTL, || counted(&runs, "c"));
        assert_eq!(plain.await.unwrap().body, "b");
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_build_that_is_not_stored_is_handed_to_those_who_waited_for_it() {
        let cache = Arc::new(StarMapCache::with_limits(keeps_nothing()));
        let runs = Arc::new(AtomicUsize::new(0));
        let answers = ask_at_once(&cache, &runs, TTL, "a").await;
        assert_eq!(runs.load(Ordering::SeqCst), 1, "one lake read, not sixteen");
        assert!(answers.iter().all(|answer| answer.body == "a"));
        assert_eq!(cache.len(), 0, "nothing was stored");
        assert_eq!(cache.flights(), 0, "what was handed goes with the queue");

        // A request that did not wait for that build gets its own.
        let later = cache.get_or_build("k", TTL, || counted(&runs, "b"));
        assert_eq!(later.await.unwrap().body, "b");
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_waiter_that_arrived_after_a_write_is_not_handed_the_view_before_it() {
        let cache = StarMapCache::default();
        let release = Notify::new();
        let runs = AtomicUsize::new(0);
        let leader = cache.get_or_build("k", TTL, || async {
            release.notified().await;
            body("before the write")
        });
        let early = cache.get_or_build("k", TTL, || counted(&runs, "early"));
        tokio::pin!(leader, early);
        assert!(poll_once(&mut leader).await.is_none());
        assert!(poll_once(&mut early).await.is_none());

        // The write lands while the leader reads; one more request
        // arrives after it.
        cache.clear();
        let late = cache.get_or_build("k", TTL, || counted(&runs, "after the write"));
        tokio::pin!(late);
        assert!(poll_once(&mut late).await.is_none());

        release.notify_one();
        assert_eq!(leader.await.unwrap().body, "before the write");
        // It asked before the write, and the leader began no earlier
        // than it arrived: the leader's answer is its answer.
        assert_eq!(early.await.unwrap().body, "before the write");
        assert_eq!(runs.load(Ordering::SeqCst), 0);
        assert_eq!(late.await.unwrap().body, "after the write");
        assert_eq!(runs.load(Ordering::SeqCst), 1);

        // Only the view after the write was stored.
        let stored = cache.get("k", TTL, Instant::now()).expect("stored");
        assert_eq!(stored.body, "after the write");
    }

    #[tokio::test]
    async fn a_handed_result_is_no_older_than_the_caller_accepts() {
        let (clock, ahead) = moved_clock();
        let cache = StarMapCache::new(keeps_nothing(), clock);
        let release = Notify::new();
        let runs = AtomicUsize::new(0);
        let leader = cache.get_or_build("k", TTL, || async {
            release.notified().await;
            body("old")
        });
        let patient = cache.get_or_build("k", TTL, || counted(&runs, "patient"));
        let forcing = cache.get_or_build("k", REFRESH_FLOOR, || counted(&runs, "new"));
        tokio::pin!(leader, patient, forcing);
        assert!(poll_once(&mut leader).await.is_none());
        assert!(poll_once(&mut patient).await.is_none());
        assert!(poll_once(&mut forcing).await.is_none());

        release.notify_one();
        assert_eq!(leader.await.unwrap().body, "old");
        ahead.store(3_000, Ordering::SeqCst);
        assert_eq!(patient.await.unwrap().body, "old", "three seconds is fine");
        assert_eq!(forcing.await.unwrap().body, "new", "older than its floor");
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_request_that_waited_too_long_is_refused_and_the_key_is_not_stuck() {
        let cache = StarMapCache::with_limits(Limits {
            max_wait: Duration::from_millis(20),
            ..Limits::default()
        });
        let runs = AtomicUsize::new(0);
        {
            let leader = cache.get_or_build("k", TTL, || async {
                std::future::pending::<Result<CachedMap, String>>().await
            });
            tokio::pin!(leader);
            assert!(poll_once(&mut leader).await.is_none());

            let refused = cache.get_or_build("k", TTL, || counted(&runs, "a")).await;
            assert_eq!(refused.unwrap_err(), Miss::Busy);
            assert_eq!(runs.load(Ordering::SeqCst), 0);
            assert_eq!(cache.waiting(), 0, "the refused request left the line");
        }
        // The leader's caller went away.
        assert_eq!(cache.flights(), 0);
        let next = cache.get_or_build("k", TTL, || counted(&runs, "a"));
        assert_eq!(next.await.unwrap().body, "a");
    }

    #[tokio::test]
    async fn requests_beyond_the_waiter_cap_are_refused_at_once() {
        let cache = StarMapCache::with_limits(Limits {
            max_waiting: 2,
            max_wait: Duration::from_secs(3_600),
            ..Limits::default()
        });
        let release = Notify::new();
        let runs = AtomicUsize::new(0);
        let leader = cache.get_or_build("k", TTL, || async {
            release.notified().await;
            body("a")
        });
        let second = cache.get_or_build("k", TTL, || counted(&runs, "b"));
        let third = cache.get_or_build("k", TTL, || counted(&runs, "b"));
        tokio::pin!(leader, second, third);
        assert!(poll_once(&mut leader).await.is_none());
        assert_eq!(cache.waiting(), 0, "a request that builds does not wait");
        assert!(poll_once(&mut second).await.is_none());
        assert!(poll_once(&mut third).await.is_none());
        assert_eq!(cache.waiting(), 2);

        // The line is full, for this map and for any other.
        for key in ["k", "other"] {
            let refused = cache.get_or_build(key, TTL, || counted(&runs, "b"));
            let refused = tokio::time::timeout(Duration::from_secs(5), refused).await;
            assert_eq!(refused.expect("at once").unwrap_err(), Miss::Busy);
        }
        assert_eq!(cache.waiting(), 2);

        // A stored map is still served to everybody.
        release.notify_one();
        assert_eq!(leader.await.unwrap().body, "a");
        assert_eq!(second.await.unwrap().body, "a");
        assert_eq!(third.await.unwrap().body, "a");
        assert_eq!(runs.load(Ordering::SeqCst), 0);
        assert_eq!(cache.waiting(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn builds_of_different_maps_are_limited_together() {
        let cache = Arc::new(StarMapCache::with_limits(Limits {
            builds: 2,
            ..Limits::default()
        }));
        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut asks = tokio::task::JoinSet::new();
        for i in 0..6 {
            let (cache, running, peak) =
                (Arc::clone(&cache), Arc::clone(&running), Arc::clone(&peak));
            asks.spawn(async move {
                let key = format!("k{i}");
                let build = || async {
                    let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    running.fetch_sub(1, Ordering::SeqCst);
                    body("a")
                };
                cache.get_or_build(&key, TTL, build).await.unwrap()
            });
        }
        assert_eq!(asks.join_all().await.len(), 6, "every request is answered");
        assert_eq!(peak.load(Ordering::SeqCst), 2, "two lake reads at a time");
        assert_eq!(cache.len(), 6);
    }

    #[tokio::test]
    async fn a_build_that_cannot_start_in_time_is_refused() {
        let cache = StarMapCache::with_limits(Limits {
            builds: 1,
            max_wait: Duration::from_millis(20),
            ..Limits::default()
        });
        let busy = cache.get_or_build("a", TTL, || async {
            std::future::pending::<Result<CachedMap, String>>().await
        });
        tokio::pin!(busy);
        assert!(poll_once(&mut busy).await.is_none());

        let runs = AtomicUsize::new(0);
        let refused = cache.get_or_build("b", TTL, || counted(&runs, "b")).await;
        assert_eq!(refused.unwrap_err(), Miss::Busy);
        assert_eq!(runs.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_failed_build_stores_nothing_and_frees_the_key() {
        let cache = StarMapCache::default();
        let failed = cache
            .get_or_build("k", TTL, || async {
                Err::<CachedMap, _>("lake gone".to_string())
            })
            .await;
        assert_eq!(failed.unwrap_err(), Miss::Failed("lake gone".to_string()));
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
