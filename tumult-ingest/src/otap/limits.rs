//! Resource limits of the OTel-Arrow receiver.
//!
//! Everything a peer declares (row counts, message body lengths,
//! decompressed sizes, attribute counts) is checked against these limits
//! before memory is allocated for it, and what a stream makes the receiver
//! keep between batches (schemas, dictionaries) is bounded per stream and
//! across all streams. Each configurable limit has a named default, a
//! maximum and an environment variable; an unparsable or zero value is
//! logged and ignored, a value above the maximum is logged and clamped.
//!
//! The structural caps ([`MAX_FIELDS_PER_SCHEMA`] and friends) are not
//! configurable: the OTAP tables are far below them.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::OtapError;

/// Rows one OTAP table (one Arrow record batch or dictionary) may declare.
/// The exporter's batches are a few thousand spans; attribute tables carry
/// one row per attribute. `KRONIKA_OTAP_MAX_ROWS`.
pub const DEFAULT_MAX_ROWS_PER_TABLE: usize = 250_000;

/// Attributes one batch may carry across all its attribute tables, counting
/// every copy handed to a second scope group. `KRONIKA_OTAP_MAX_ATTRIBUTES`.
pub const DEFAULT_MAX_ATTRIBUTES_PER_BATCH: usize = 1_000_000;

/// Bytes one batch may occupy once its Arrow buffers are decompressed and
/// its dictionary columns expanded. `KRONIKA_OTAP_MAX_DECODED_BYTES`.
pub const DEFAULT_MAX_DECODED_BYTES_PER_BATCH: usize = 64 * 1024 * 1024;

/// Size of one `BatchArrowRecords` gRPC message, before and after gRPC-level
/// decompression (gRPC's own default). `KRONIKA_OTAP_MAX_MESSAGE_BYTES`.
pub const DEFAULT_MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;

/// Arrow streams open at once across the three services; further streams
/// are refused with `RESOURCE_EXHAUSTED`. `KRONIKA_OTAP_MAX_STREAMS`.
pub const DEFAULT_MAX_STREAMS: usize = 64;

/// Arrow streams one connection may hold of [`DEFAULT_MAX_STREAMS`], so a
/// single connection cannot take every slot.
/// `KRONIKA_OTAP_MAX_STREAMS_PER_CONNECTION`.
pub const DEFAULT_MAX_STREAMS_PER_CONNECTION: usize = 32;

/// Batches decoded, converted and written at once; further batches wait,
/// which is the backpressure the exporter sees. The permit is held until the
/// batch is stored, so this also bounds the converted batches in memory.
/// `KRONIKA_OTAP_MAX_CONCURRENT_DECODES`.
pub const DEFAULT_MAX_CONCURRENT_DECODES: usize = 4;

/// How long a stream may stay silent before it is closed with
/// `UNAVAILABLE`. The exporter recycles its streams every 30 s by default.
/// `KRONIKA_OTAP_STREAM_IDLE_SECS`.
pub const DEFAULT_STREAM_IDLE: Duration = Duration::from_secs(300);

/// How long a stream may live at all before it is closed with
/// `UNAVAILABLE` between two batches. `KRONIKA_OTAP_STREAM_LIFETIME_SECS`.
pub const DEFAULT_STREAM_LIFETIME: Duration = Duration::from_secs(600);

/// How long a `BatchStatus` may wait for the client to read it before the
/// stream is dropped. `KRONIKA_OTAP_ACK_TIMEOUT_SECS`.
pub const DEFAULT_ACK_TIMEOUT: Duration = Duration::from_secs(30);

/// Wall-clock time one batch may spend being decoded and converted.
/// `KRONIKA_OTAP_BATCH_DEADLINE_MS`.
pub const DEFAULT_BATCH_DEADLINE: Duration = Duration::from_secs(10);

/// Bytes of schemas and dictionaries one stream may make the receiver keep
/// between batches. `KRONIKA_OTAP_MAX_STREAM_RETAINED_BYTES`.
pub const DEFAULT_MAX_STREAM_RETAINED_BYTES: usize = 32 * 1024 * 1024;

/// The same, summed over every open stream.
/// `KRONIKA_OTAP_MAX_RETAINED_BYTES`.
pub const DEFAULT_MAX_RETAINED_BYTES: usize = 256 * 1024 * 1024;

/// The part of [`DEFAULT_MAX_RETAINED_BYTES`] the streams of one connection,
/// and of one peer, may hold: one eighth, unless set.
/// `KRONIKA_OTAP_MAX_CONNECTION_RETAINED_BYTES`,
/// `KRONIKA_OTAP_MAX_PEER_RETAINED_BYTES`.
pub const RETAINED_SHARE_DIVISOR: usize = 8;

/// Delta dictionary batches one schema of a stream may apply; each one
/// copies the dictionary it extends. `KRONIKA_OTAP_MAX_DICTIONARY_DELTAS`.
pub const DEFAULT_MAX_DICTIONARY_DELTAS: usize = 65_536;

/// Batches a stream may have rejected before it is closed.
/// `KRONIKA_OTAP_MAX_REJECTED_BATCHES`.
pub const DEFAULT_MAX_REJECTED_BATCHES: usize = 8;

/// Fields (columns, struct members and list items together) one Arrow schema
/// may declare. The widest OTAP table has under 40.
pub const MAX_FIELDS_PER_SCHEMA: usize = 256;

/// Dictionary-encoded fields one Arrow schema may declare.
pub const MAX_DICTIONARIES_PER_SCHEMA: usize = 64;

/// Bytes of a payload's `schema_id`.
pub const MAX_SCHEMA_ID_BYTES: usize = 256;

/// Bytes of one Arrow field name.
pub const MAX_FIELD_NAME_BYTES: usize = 128;

/// How deep struct and list fields may nest.
pub const MAX_SCHEMA_DEPTH: usize = 4;

/// Nesting depth of a CBOR-encoded map or array attribute value.
pub const MAX_CBOR_DEPTH: usize = 16;

/// Rows between two looks at the clock in the decoding loops.
const DEADLINE_STRIDE: usize = 256;

/// The limits one receiver runs with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OtapLimits {
    pub max_rows_per_table: usize,
    pub max_attributes_per_batch: usize,
    pub max_decoded_bytes_per_batch: usize,
    pub max_message_bytes: usize,
    pub max_streams: usize,
    pub max_streams_per_connection: usize,
    pub max_concurrent_decodes: usize,
    pub stream_idle: Duration,
    pub stream_lifetime: Duration,
    pub ack_timeout: Duration,
    pub batch_deadline: Duration,
    pub max_stream_retained_bytes: usize,
    pub max_retained_bytes: usize,
    /// What the streams of one connection may hold of
    /// [`Self::max_retained_bytes`].
    pub max_connection_retained_bytes: usize,
    /// What the streams of one peer (see `crate::listener::peer_key`) may
    /// hold of it, over all its connections.
    pub max_peer_retained_bytes: usize,
    pub max_dictionary_deltas: usize,
    pub max_rejected_batches: usize,
}

/// The default share of a pool of `pool` bytes.
fn retained_share(pool: usize) -> usize {
    (pool / RETAINED_SHARE_DIVISOR).max(1)
}

impl Default for OtapLimits {
    fn default() -> Self {
        Self {
            max_rows_per_table: DEFAULT_MAX_ROWS_PER_TABLE,
            max_attributes_per_batch: DEFAULT_MAX_ATTRIBUTES_PER_BATCH,
            max_decoded_bytes_per_batch: DEFAULT_MAX_DECODED_BYTES_PER_BATCH,
            max_message_bytes: DEFAULT_MAX_MESSAGE_BYTES,
            max_streams: DEFAULT_MAX_STREAMS,
            max_streams_per_connection: DEFAULT_MAX_STREAMS_PER_CONNECTION,
            max_concurrent_decodes: DEFAULT_MAX_CONCURRENT_DECODES,
            stream_idle: DEFAULT_STREAM_IDLE,
            stream_lifetime: DEFAULT_STREAM_LIFETIME,
            ack_timeout: DEFAULT_ACK_TIMEOUT,
            batch_deadline: DEFAULT_BATCH_DEADLINE,
            max_stream_retained_bytes: DEFAULT_MAX_STREAM_RETAINED_BYTES,
            max_retained_bytes: DEFAULT_MAX_RETAINED_BYTES,
            max_connection_retained_bytes: retained_share(DEFAULT_MAX_RETAINED_BYTES),
            max_peer_retained_bytes: retained_share(DEFAULT_MAX_RETAINED_BYTES),
            max_dictionary_deltas: DEFAULT_MAX_DICTIONARY_DELTAS,
            max_rejected_batches: DEFAULT_MAX_REJECTED_BATCHES,
        }
    }
}

/// The largest value each variable accepts. Far above any sensible setting,
/// low enough that no limit overflows what it is used for (semaphore
/// permits, timer durations, byte counts).
mod ceiling {
    pub const ROWS: usize = 10_000_000;
    pub const ATTRIBUTES: usize = 100_000_000;
    pub const DECODED_BYTES: usize = 4 << 30;
    pub const MESSAGE_BYTES: usize = 256 << 20;
    pub const STREAMS: usize = 4096;
    pub const CONCURRENT_DECODES: usize = 256;
    pub const IDLE_SECS: usize = 86_400;
    pub const LIFETIME_SECS: usize = 604_800;
    pub const ACK_TIMEOUT_SECS: usize = 3600;
    pub const DEADLINE_MS: usize = 600_000;
    pub const STREAM_RETAINED_BYTES: usize = 1 << 30;
    pub const RETAINED_BYTES: usize = 16 << 30;
    pub const DICTIONARY_DELTAS: usize = 16_777_216;
    pub const REJECTED_BATCHES: usize = 1_000_000;
}

fn whole(units: u128) -> usize {
    usize::try_from(units).unwrap_or(usize::MAX)
}

fn secs(n: usize) -> Duration {
    Duration::from_secs(u64::try_from(n).unwrap_or(u64::MAX))
}

fn millis(n: usize) -> Duration {
    Duration::from_millis(u64::try_from(n).unwrap_or(u64::MAX))
}

impl OtapLimits {
    /// The defaults overridden by the `KRONIKA_OTAP_*` environment variables.
    #[must_use]
    pub fn from_env() -> Self {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// [`Self::from_env`] with the variable lookup injected.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let bounded = |name: &str, default: usize, max: usize| -> usize {
            let Some(raw) = lookup(name) else {
                return default;
            };
            match raw.trim().parse::<usize>() {
                Ok(v) if v > max => {
                    tracing::warn!(
                        variable = name,
                        max,
                        "OTAP limit above its maximum, using the maximum"
                    );
                    max
                }
                Ok(v) if v > 0 => v,
                _ => {
                    tracing::warn!(
                        variable = name,
                        default,
                        "ignoring OTAP limit that is not a positive integer"
                    );
                    default
                }
            }
        };
        let d = Self::default();
        let max_streams = bounded("KRONIKA_OTAP_MAX_STREAMS", d.max_streams, ceiling::STREAMS);
        let max_retained_bytes = bounded(
            "KRONIKA_OTAP_MAX_RETAINED_BYTES",
            d.max_retained_bytes,
            ceiling::RETAINED_BYTES,
        );
        Self {
            max_rows_per_table: bounded(
                "KRONIKA_OTAP_MAX_ROWS",
                d.max_rows_per_table,
                ceiling::ROWS,
            ),
            max_attributes_per_batch: bounded(
                "KRONIKA_OTAP_MAX_ATTRIBUTES",
                d.max_attributes_per_batch,
                ceiling::ATTRIBUTES,
            ),
            max_decoded_bytes_per_batch: bounded(
                "KRONIKA_OTAP_MAX_DECODED_BYTES",
                d.max_decoded_bytes_per_batch,
                ceiling::DECODED_BYTES,
            ),
            max_message_bytes: bounded(
                "KRONIKA_OTAP_MAX_MESSAGE_BYTES",
                d.max_message_bytes,
                ceiling::MESSAGE_BYTES,
            ),
            max_streams,
            max_streams_per_connection: bounded(
                "KRONIKA_OTAP_MAX_STREAMS_PER_CONNECTION",
                d.max_streams_per_connection.min(max_streams),
                max_streams,
            ),
            max_concurrent_decodes: bounded(
                "KRONIKA_OTAP_MAX_CONCURRENT_DECODES",
                d.max_concurrent_decodes,
                ceiling::CONCURRENT_DECODES,
            ),
            stream_idle: secs(bounded(
                "KRONIKA_OTAP_STREAM_IDLE_SECS",
                whole(d.stream_idle.as_secs().into()),
                ceiling::IDLE_SECS,
            )),
            stream_lifetime: secs(bounded(
                "KRONIKA_OTAP_STREAM_LIFETIME_SECS",
                whole(d.stream_lifetime.as_secs().into()),
                ceiling::LIFETIME_SECS,
            )),
            ack_timeout: secs(bounded(
                "KRONIKA_OTAP_ACK_TIMEOUT_SECS",
                whole(d.ack_timeout.as_secs().into()),
                ceiling::ACK_TIMEOUT_SECS,
            )),
            batch_deadline: millis(bounded(
                "KRONIKA_OTAP_BATCH_DEADLINE_MS",
                whole(d.batch_deadline.as_millis()),
                ceiling::DEADLINE_MS,
            )),
            max_stream_retained_bytes: bounded(
                "KRONIKA_OTAP_MAX_STREAM_RETAINED_BYTES",
                d.max_stream_retained_bytes,
                ceiling::STREAM_RETAINED_BYTES,
            ),
            max_retained_bytes,
            // A share follows the pool unless it is set, and never
            // exceeds it.
            max_connection_retained_bytes: bounded(
                "KRONIKA_OTAP_MAX_CONNECTION_RETAINED_BYTES",
                retained_share(max_retained_bytes),
                max_retained_bytes,
            ),
            max_peer_retained_bytes: bounded(
                "KRONIKA_OTAP_MAX_PEER_RETAINED_BYTES",
                retained_share(max_retained_bytes),
                max_retained_bytes,
            ),
            max_dictionary_deltas: bounded(
                "KRONIKA_OTAP_MAX_DICTIONARY_DELTAS",
                d.max_dictionary_deltas,
                ceiling::DICTIONARY_DELTAS,
            ),
            max_rejected_batches: bounded(
                "KRONIKA_OTAP_MAX_REJECTED_BATCHES",
                d.max_rejected_batches,
                ceiling::REJECTED_BATCHES,
            ),
        }
    }
}

/// An account of bytes streams make the receiver keep between batches:
/// the pool all streams of a receiver share, or the share of it one
/// connection or one peer may hold.
#[derive(Debug)]
pub struct RetainedPool {
    what: &'static str,
    limit: usize,
    /// Whether streams of other clients draw on the account too. Then a
    /// batch that does not fit may fit later.
    shared: bool,
    used: AtomicUsize,
}

impl RetainedPool {
    /// The pool of a receiver.
    #[must_use]
    pub fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            what: "retained bytes across streams",
            limit,
            shared: true,
            used: AtomicUsize::new(0),
        })
    }

    /// One client's share of a pool; `what` names it in the error.
    #[must_use]
    pub fn share(what: &'static str, limit: usize) -> Arc<Self> {
        Arc::new(Self {
            what,
            limit,
            shared: false,
            used: AtomicUsize::new(0),
        })
    }

    /// Bytes streams hold right now.
    #[must_use]
    pub fn used(&self) -> usize {
        self.used.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn limit(&self) -> usize {
        self.limit
    }

    fn take(&self, extra: usize) -> Result<(), OtapError> {
        let fits = |used: usize| used.checked_add(extra).filter(|total| *total <= self.limit);
        self.used
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, fits)
            .map_err(|_| OtapError::StreamState {
                what: self.what,
                limit: self.limit,
                shared: self.shared,
            })?;
        Ok(())
    }

    fn give(&self, back: usize) {
        self.used.fetch_sub(back, Ordering::Relaxed);
    }
}

/// What one stream holds in every account it draws on: its connection's
/// share, its peer's share and the receiver's pool, in that order. The
/// same number of bytes is held in each; everything is given back on drop.
#[derive(Debug, Default)]
pub(super) struct Retained {
    accounts: Vec<Arc<RetainedPool>>,
    held: usize,
}

impl Retained {
    /// `accounts` from the narrowest to the widest: the first one without
    /// room is the one the error names, so a client over its own share is
    /// told so, and only a full pool is reported as shared.
    pub fn new(accounts: Vec<Arc<RetainedPool>>) -> Self {
        Self { accounts, held: 0 }
    }

    /// Change the stream's holding to `wanted` bytes. Refused, it holds
    /// what it held before, in every account.
    pub fn resize(&mut self, wanted: usize) -> Result<(), OtapError> {
        if wanted <= self.held {
            let back = self.held - wanted;
            self.accounts.iter().for_each(|account| account.give(back));
            self.held = wanted;
            return Ok(());
        }
        let extra = wanted - self.held;
        for (taken, account) in self.accounts.iter().enumerate() {
            if let Err(full) = account.take(extra) {
                self.accounts[..taken]
                    .iter()
                    .for_each(|account| account.give(extra));
                return Err(full);
            }
        }
        self.held = wanted;
        Ok(())
    }
}

impl Drop for Retained {
    fn drop(&mut self) {
        // Shrinking cannot fail.
        let _ = self.resize(0);
    }
}

/// What one batch has left of its per-batch limits. Shared by the IPC
/// decoder and the table decoders of that batch.
#[derive(Debug)]
pub(super) struct Budget {
    limits: OtapLimits,
    bytes: AtomicUsize,
    attributes: AtomicUsize,
    /// When the batch must be done (`None`: too far away to represent).
    deadline: Option<Instant>,
    ticks: AtomicUsize,
}

impl Budget {
    pub fn new(limits: OtapLimits) -> Self {
        Self {
            limits,
            bytes: AtomicUsize::new(limits.max_decoded_bytes_per_batch),
            attributes: AtomicUsize::new(limits.max_attributes_per_batch),
            deadline: Instant::now().checked_add(limits.batch_deadline),
            ticks: AtomicUsize::new(0),
        }
    }

    pub fn limits(&self) -> &OtapLimits {
        &self.limits
    }

    /// Account for `n` decoded bytes.
    pub fn charge_bytes(&self, n: usize) -> Result<(), OtapError> {
        charge(&self.bytes, n).ok_or(OtapError::Limit {
            what: "decoded bytes per batch",
            limit: self.limits.max_decoded_bytes_per_batch,
        })
    }

    /// Account for `n` attributes.
    pub fn charge_attributes(&self, n: usize) -> Result<(), OtapError> {
        charge(&self.attributes, n).ok_or(OtapError::Limit {
            what: "attributes per batch",
            limit: self.limits.max_attributes_per_batch,
        })
    }

    /// Fail once the batch has used up its wall-clock time.
    pub fn check_deadline(&self) -> Result<(), OtapError> {
        match self.deadline {
            Some(deadline) if Instant::now() >= deadline => Err(OtapError::Deadline {
                limit_ms: u64::try_from(self.limits.batch_deadline.as_millis()).unwrap_or(u64::MAX),
            }),
            _ => Ok(()),
        }
    }

    /// [`Self::check_deadline`] for row loops: looks at the clock on the
    /// first call and then once every [`DEADLINE_STRIDE`] calls.
    pub fn tick(&self) -> Result<(), OtapError> {
        let tick = self.ticks.fetch_add(1, Ordering::Relaxed);
        if !tick.is_multiple_of(DEADLINE_STRIDE) {
            return Ok(());
        }
        self.check_deadline()
    }
}

fn charge(left: &AtomicUsize, n: usize) -> Option<()> {
    left.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_sub(n))
        .ok()
        .map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_overrides_defaults_and_bad_values_are_ignored() {
        let limits = OtapLimits::from_lookup(|name| match name {
            "KRONIKA_OTAP_MAX_ROWS" => Some("1000".into()),
            "KRONIKA_OTAP_MAX_MESSAGE_BYTES" => Some(" 8388608 ".into()),
            "KRONIKA_OTAP_STREAM_IDLE_SECS" => Some("45".into()),
            "KRONIKA_OTAP_BATCH_DEADLINE_MS" => Some("250".into()),
            "KRONIKA_OTAP_MAX_STREAMS" => Some("0".into()),
            "KRONIKA_OTAP_MAX_ATTRIBUTES" => Some("many".into()),
            _ => None,
        });
        assert_eq!(limits.max_rows_per_table, 1000);
        assert_eq!(limits.max_message_bytes, 8 * 1024 * 1024);
        assert_eq!(limits.stream_idle, Duration::from_secs(45));
        assert_eq!(limits.batch_deadline, Duration::from_millis(250));
        assert_eq!(limits.max_streams, DEFAULT_MAX_STREAMS);
        assert_eq!(
            limits.max_attributes_per_batch,
            DEFAULT_MAX_ATTRIBUTES_PER_BATCH
        );
        assert_eq!(
            OtapLimits::from_lookup(|_| None),
            OtapLimits::default(),
            "no variables, no change"
        );
    }

    #[test]
    fn every_limit_is_clamped_to_its_maximum() {
        // usize::MAX permits would panic in Semaphore::new at startup.
        let limits = OtapLimits::from_lookup(|_| Some(usize::MAX.to_string()));
        assert_eq!(limits.max_streams, ceiling::STREAMS);
        assert_eq!(limits.max_streams_per_connection, ceiling::STREAMS);
        assert_eq!(limits.max_concurrent_decodes, ceiling::CONCURRENT_DECODES);
        assert_eq!(limits.max_rows_per_table, ceiling::ROWS);
        assert_eq!(limits.max_attributes_per_batch, ceiling::ATTRIBUTES);
        assert_eq!(limits.max_decoded_bytes_per_batch, ceiling::DECODED_BYTES);
        assert_eq!(limits.max_message_bytes, ceiling::MESSAGE_BYTES);
        assert_eq!(limits.stream_idle, secs(ceiling::IDLE_SECS));
        assert_eq!(limits.stream_lifetime, secs(ceiling::LIFETIME_SECS));
        assert_eq!(limits.ack_timeout, secs(ceiling::ACK_TIMEOUT_SECS));
        assert_eq!(limits.batch_deadline, millis(ceiling::DEADLINE_MS));
        assert_eq!(
            limits.max_stream_retained_bytes,
            ceiling::STREAM_RETAINED_BYTES
        );
        assert_eq!(limits.max_retained_bytes, ceiling::RETAINED_BYTES);
        assert_eq!(
            limits.max_connection_retained_bytes,
            ceiling::RETAINED_BYTES
        );
        assert_eq!(limits.max_peer_retained_bytes, ceiling::RETAINED_BYTES);
        assert_eq!(limits.max_dictionary_deltas, ceiling::DICTIONARY_DELTAS);
        assert_eq!(limits.max_rejected_batches, ceiling::REJECTED_BATCHES);
        // What the limits feed must accept them.
        let _streams = tokio::sync::Semaphore::new(limits.max_streams);
        let _decodes = tokio::sync::Semaphore::new(limits.max_concurrent_decodes);
        let _budget = Budget::new(limits);

        // A connection's share never exceeds the total.
        let limits = OtapLimits::from_lookup(|name| match name {
            "KRONIKA_OTAP_MAX_STREAMS" => Some("8".into()),
            "KRONIKA_OTAP_MAX_STREAMS_PER_CONNECTION" => Some("100".into()),
            _ => None,
        });
        assert_eq!(limits.max_streams_per_connection, 8);
        let limits = OtapLimits::from_lookup(|name| match name {
            "KRONIKA_OTAP_MAX_STREAMS" => Some("8".into()),
            _ => None,
        });
        assert_eq!(limits.max_streams_per_connection, 8);
    }

    #[test]
    fn budget_refuses_what_exceeds_the_limit_and_names_it() {
        let budget = Budget::new(OtapLimits {
            max_decoded_bytes_per_batch: 10,
            max_attributes_per_batch: 2,
            ..OtapLimits::default()
        });
        budget.charge_bytes(6).unwrap();
        budget.charge_bytes(4).unwrap();
        let err = budget.charge_bytes(1).unwrap_err().to_string();
        assert!(
            err.contains("decoded bytes per batch") && err.contains("10"),
            "{err}"
        );
        budget.charge_attributes(2).unwrap();
        let err = budget.charge_attributes(1).unwrap_err().to_string();
        assert!(err.contains("attributes per batch"), "{err}");
    }

    #[test]
    fn budget_deadline_is_checked_on_the_first_tick_and_every_stride() {
        let expired = Budget::new(OtapLimits {
            batch_deadline: Duration::ZERO,
            ..OtapLimits::default()
        });
        let err = expired.tick().unwrap_err();
        assert!(matches!(err, OtapError::Deadline { limit_ms: 0 }), "{err}");
        // Between strides the clock is not consulted.
        for _ in 1..DEADLINE_STRIDE {
            expired.tick().unwrap();
        }
        expired.tick().unwrap_err();
        let fresh = Budget::new(OtapLimits::default());
        for _ in 0..3 * DEADLINE_STRIDE {
            fresh.tick().unwrap();
        }
    }

    #[test]
    fn retained_pool_is_shared_and_gives_back_what_a_stream_drops() {
        let pool = RetainedPool::new(100);
        let mut a = Retained::new(vec![pool.clone()]);
        let mut b = Retained::new(vec![pool.clone()]);
        a.resize(60).unwrap();
        b.resize(40).unwrap();
        let err = b.resize(41).unwrap_err();
        assert!(
            matches!(err, OtapError::StreamState { shared: true, .. }),
            "{err}"
        );
        assert_eq!(pool.used(), 100, "a refused resize holds on");
        a.resize(0).unwrap();
        b.resize(100).unwrap();
        assert_eq!(pool.used(), 100);
        drop(b);
        assert_eq!(pool.used(), 0, "a dropped stream holds nothing");
    }

    fn state(err: &OtapError) -> (&'static str, usize, bool) {
        match err {
            OtapError::StreamState {
                what,
                limit,
                shared,
            } => (what, *limit, *shared),
            other => panic!("not a stream state error: {other}"),
        }
    }

    #[test]
    fn the_narrowest_full_account_is_the_one_named() {
        let pool = RetainedPool::new(100);
        let connection = RetainedPool::share("retained bytes per connection", 30);
        let peer = RetainedPool::share("retained bytes per peer", 50);
        let accounts = || vec![connection.clone(), peer.clone(), pool.clone()];
        let mut first = Retained::new(accounts());
        let mut second = Retained::new(accounts());
        first.resize(20).unwrap();
        // The connection's own share: its fault, not a shared shortage.
        let err = second.resize(11).unwrap_err();
        assert_eq!(state(&err), ("retained bytes per connection", 30, false));
        assert_eq!(
            (connection.used(), peer.used(), pool.used()),
            (20, 20, 20),
            "a refused stream holds nothing anywhere"
        );
        second.resize(10).unwrap();

        // Another connection of the same peer runs into the peer's share.
        let other = RetainedPool::share("retained bytes per connection", 30);
        let mut third = Retained::new(vec![other.clone(), peer.clone(), pool.clone()]);
        let err = third.resize(21).unwrap_err();
        assert_eq!(state(&err), ("retained bytes per peer", 50, false));
        assert_eq!((other.used(), peer.used(), pool.used()), (0, 30, 30));
        third.resize(20).unwrap();

        // Another peer runs into the pool: that is the shared shortage.
        let far = RetainedPool::share("retained bytes per peer", 80);
        let mut fourth = Retained::new(vec![far.clone(), pool.clone()]);
        let err = fourth.resize(51).unwrap_err();
        assert_eq!(state(&err), ("retained bytes across streams", 100, true));
        assert_eq!(
            (far.used(), pool.used()),
            (0, 50),
            "the share is given back"
        );
        fourth.resize(50).unwrap();

        drop((first, second, third, fourth));
        let all = [&connection, &peer, &other, &far, &pool];
        assert!(all.iter().all(|account| account.used() == 0));
    }

    #[test]
    fn shares_default_to_an_eighth_of_the_pool_and_never_exceed_it() {
        let d = OtapLimits::default();
        assert_eq!(d.max_connection_retained_bytes, 32 * 1024 * 1024);
        assert_eq!(d.max_peer_retained_bytes, 32 * 1024 * 1024);
        // A stream at its own limit fits its connection's share once.
        assert!(d.max_stream_retained_bytes <= d.max_connection_retained_bytes);
        assert!(2 * d.max_stream_retained_bytes > d.max_connection_retained_bytes);

        let pool_only = OtapLimits::from_lookup(|name| match name {
            "KRONIKA_OTAP_MAX_RETAINED_BYTES" => Some("8000".into()),
            _ => None,
        });
        assert_eq!(pool_only.max_connection_retained_bytes, 1000);
        assert_eq!(pool_only.max_peer_retained_bytes, 1000);

        let set = OtapLimits::from_lookup(|name| match name {
            "KRONIKA_OTAP_MAX_RETAINED_BYTES" => Some("8000".into()),
            "KRONIKA_OTAP_MAX_CONNECTION_RETAINED_BYTES" => Some("2000".into()),
            "KRONIKA_OTAP_MAX_PEER_RETAINED_BYTES" => Some("9000".into()),
            _ => None,
        });
        assert_eq!(set.max_connection_retained_bytes, 2000);
        assert_eq!(set.max_peer_retained_bytes, 8000, "clamped to the pool");

        let tiny = OtapLimits::from_lookup(|name| match name {
            "KRONIKA_OTAP_MAX_RETAINED_BYTES" => Some("3".into()),
            _ => None,
        });
        assert_eq!(tiny.max_connection_retained_bytes, 1, "never zero");
    }
}
