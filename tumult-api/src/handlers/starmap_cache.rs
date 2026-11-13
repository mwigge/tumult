//! The Star Map response cache: serialised responses kept in process for
//! [`TTL`], so a page that asks again while the user pans, or a second
//! viewer of the same estate, does not read the lake again.
//!
//! The key is built by the handler from the request parameters and the
//! principal's environment scopes; two principals with different scopes
//! never share an entry. The cache is bounded, and every write that can
//! change a map without new telemetry (a risk suppression) clears it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use sha2::{Digest, Sha256};

/// How long a response is served without reading the lake again.
pub(crate) const TTL: Duration = Duration::from_secs(30);
/// Most responses kept at once; the oldest goes first.
const MAX_ENTRIES: usize = 64;
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
    fn fresh(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.stored) < TTL
    }
}

/// Responses by request key. Cheap to share: handlers hold it in an `Arc`.
#[derive(Default)]
pub(crate) struct StarMapCache {
    entries: Mutex<HashMap<String, Entry>>,
}

impl StarMapCache {
    /// The entries. A panic while the lock was held cannot leave the map
    /// half-written (every change is one insert or remove), so a poisoned
    /// lock is still good to use.
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Entry>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The response stored for `key`, while it is younger than [`TTL`].
    pub(crate) fn get(&self, key: &str, now: Instant) -> Option<Arc<CachedMap>> {
        let entries = self.lock();
        let entry = entries.get(key).filter(|entry| entry.fresh(now))?;
        Some(Arc::clone(&entry.map))
    }

    /// Store a response, dropping what has expired and, when the cache is
    /// still full, the oldest entry.
    pub(crate) fn put(&self, key: String, map: Arc<CachedMap>, now: Instant) {
        let mut entries = self.lock();
        entries.retain(|_, entry| entry.fresh(now));
        if entries.len() >= MAX_ENTRIES && !entries.contains_key(&key) {
            let oldest = entries
                .iter()
                .min_by_key(|(_, entry)| entry.stored)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                entries.remove(&oldest);
            }
        }
        entries.insert(key, Entry { stored: now, map });
    }

    /// Forget every response: something a map shows was just written.
    pub(crate) fn clear(&self) {
        self.lock().clear();
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.lock().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(body: &str) -> Arc<CachedMap> {
        Arc::new(CachedMap::new(body.as_bytes().to_vec()))
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
        cache.put("k".into(), map("a"), start);
        let just_inside = start + TTL - Duration::from_millis(1);
        assert_eq!(cache.get("k", start), Some(map("a")));
        assert_eq!(cache.get("k", just_inside), Some(map("a")));
        assert_eq!(cache.get("k", start + TTL), None);
        assert_eq!(cache.get("other", start), None);
    }

    #[test]
    fn a_newer_response_replaces_the_stored_one() {
        let cache = StarMapCache::default();
        let start = Instant::now();
        cache.put("k".into(), map("a"), start);
        cache.put("k".into(), map("b"), start + Duration::from_secs(40));
        assert_eq!(
            cache.get("k", start + Duration::from_secs(60)),
            Some(map("b"))
        );
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn the_cache_is_bounded_and_drops_the_oldest_first() {
        let cache = StarMapCache::default();
        let start = Instant::now();
        for i in 0..MAX_ENTRIES {
            let at = start + Duration::from_millis(i as u64);
            cache.put(format!("k{i}"), map("a"), at);
        }
        let later = start + Duration::from_secs(1);
        cache.put("new".into(), map("b"), later);
        assert_eq!(cache.len(), MAX_ENTRIES);
        assert_eq!(cache.get("k0", later), None, "the oldest went");
        assert!(cache.get("k1", later).is_some());
        assert!(cache.get("new", later).is_some());

        // Expired entries make room before anything live is dropped.
        let expired = start + TTL + Duration::from_secs(2);
        cache.put("fresh".into(), map("c"), expired);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn clear_forgets_everything() {
        let cache = StarMapCache::default();
        let start = Instant::now();
        cache.put("k".into(), map("a"), start);
        cache.clear();
        assert_eq!(cache.get("k", start), None);
        assert_eq!(cache.len(), 0);
    }
}
