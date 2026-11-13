//! Resource limits of the OTel-Arrow receiver.
//!
//! Everything a peer declares (row counts, message body lengths,
//! decompressed sizes, attribute counts) is checked against these limits
//! before memory is allocated for it. Each limit has a named default and an
//! environment variable; an unparsable or zero value is logged and ignored.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

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

/// Batches decoded at once on the blocking pool; further batches wait, which
/// is the backpressure the exporter sees.
/// `KRONIKA_OTAP_MAX_CONCURRENT_DECODES`.
pub const DEFAULT_MAX_CONCURRENT_DECODES: usize = 4;

/// How long a stream may stay silent before it is closed with
/// `UNAVAILABLE`. The exporter recycles its streams every 30 s by default.
/// `KRONIKA_OTAP_STREAM_IDLE_SECS`.
pub const DEFAULT_STREAM_IDLE: Duration = Duration::from_secs(300);

/// The limits one receiver runs with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OtapLimits {
    pub max_rows_per_table: usize,
    pub max_attributes_per_batch: usize,
    pub max_decoded_bytes_per_batch: usize,
    pub max_message_bytes: usize,
    pub max_streams: usize,
    pub max_concurrent_decodes: usize,
    pub stream_idle: Duration,
}

impl Default for OtapLimits {
    fn default() -> Self {
        Self {
            max_rows_per_table: DEFAULT_MAX_ROWS_PER_TABLE,
            max_attributes_per_batch: DEFAULT_MAX_ATTRIBUTES_PER_BATCH,
            max_decoded_bytes_per_batch: DEFAULT_MAX_DECODED_BYTES_PER_BATCH,
            max_message_bytes: DEFAULT_MAX_MESSAGE_BYTES,
            max_streams: DEFAULT_MAX_STREAMS,
            max_concurrent_decodes: DEFAULT_MAX_CONCURRENT_DECODES,
            stream_idle: DEFAULT_STREAM_IDLE,
        }
    }
}

impl OtapLimits {
    /// The defaults overridden by the `KRONIKA_OTAP_*` environment variables.
    #[must_use]
    pub fn from_env() -> Self {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// [`Self::from_env`] with the variable lookup injected.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let positive = |name: &str, default: usize| -> usize {
            let Some(raw) = lookup(name) else {
                return default;
            };
            match raw.trim().parse::<usize>() {
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
        let defaults = Self::default();
        let idle_secs = positive(
            "KRONIKA_OTAP_STREAM_IDLE_SECS",
            usize::try_from(defaults.stream_idle.as_secs()).unwrap_or(usize::MAX),
        );
        Self {
            max_rows_per_table: positive("KRONIKA_OTAP_MAX_ROWS", defaults.max_rows_per_table),
            max_attributes_per_batch: positive(
                "KRONIKA_OTAP_MAX_ATTRIBUTES",
                defaults.max_attributes_per_batch,
            ),
            max_decoded_bytes_per_batch: positive(
                "KRONIKA_OTAP_MAX_DECODED_BYTES",
                defaults.max_decoded_bytes_per_batch,
            ),
            max_message_bytes: positive(
                "KRONIKA_OTAP_MAX_MESSAGE_BYTES",
                defaults.max_message_bytes,
            ),
            max_streams: positive("KRONIKA_OTAP_MAX_STREAMS", defaults.max_streams),
            max_concurrent_decodes: positive(
                "KRONIKA_OTAP_MAX_CONCURRENT_DECODES",
                defaults.max_concurrent_decodes,
            ),
            stream_idle: Duration::from_secs(u64::try_from(idle_secs).unwrap_or(u64::MAX)),
        }
    }
}

/// What one batch has left of its per-batch limits. Shared by the IPC
/// pre-scan and the table decoders of that batch.
#[derive(Debug)]
pub(super) struct Budget {
    limits: OtapLimits,
    bytes: AtomicUsize,
    attributes: AtomicUsize,
}

impl Budget {
    pub fn new(limits: OtapLimits) -> Self {
        Self {
            limits,
            bytes: AtomicUsize::new(limits.max_decoded_bytes_per_batch),
            attributes: AtomicUsize::new(limits.max_attributes_per_batch),
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
}

fn charge(left: &AtomicUsize, n: usize) -> Option<()> {
    let remaining = left.load(Ordering::Relaxed).checked_sub(n)?;
    left.store(remaining, Ordering::Relaxed);
    Some(())
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
            "KRONIKA_OTAP_MAX_STREAMS" => Some("0".into()),
            "KRONIKA_OTAP_MAX_ATTRIBUTES" => Some("many".into()),
            _ => None,
        });
        assert_eq!(limits.max_rows_per_table, 1000);
        assert_eq!(limits.max_message_bytes, 8 * 1024 * 1024);
        assert_eq!(limits.stream_idle, Duration::from_secs(45));
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
}
