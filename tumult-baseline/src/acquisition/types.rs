//! Data structures for baseline acquisition inputs and results.

use crate::tolerance::Method;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AcquisitionError {
    #[error("no probes provided")]
    NoProbes,
    #[error("probe '{name}' has no samples after warmup")]
    NoSamplesAfterWarmup { name: String },
}

/// Per-probe statistics derived from baseline samples.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeStats {
    pub name: String,
    pub mean: f64,
    pub stddev: f64,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub min: f64,
    pub max: f64,
    pub error_rate: f64,
    pub samples: u32,
    /// Tolerance lower bound derived from THIS probe's samples alone.
    ///
    /// Probes measure different quantities on different scales (latency in ms
    /// vs throughput in rps), so bounds are never pooled across probes — each
    /// probe carries its own.
    pub tolerance_lower: f64,
    /// Tolerance upper bound derived from THIS probe's samples alone.
    pub tolerance_upper: f64,
}

/// Result of a complete baseline acquisition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcquisitionResult {
    pub probes: Vec<ProbeStats>,
    /// Representative tolerance lower bound.
    ///
    /// With exactly one probe this is that probe's bound (the historical
    /// behaviour). With multiple probes it is the bound of the probe with the
    /// worst (highest) coefficient of variation — the noisiest baseline drives
    /// the headline tolerance. Prefer the per-probe bounds on [`ProbeStats`]
    /// for evaluation; pooling probes of different scales into one bound is
    /// statistically meaningless.
    pub tolerance_lower: f64,
    /// Representative tolerance upper bound; see [`Self::tolerance_lower`].
    pub tolerance_upper: f64,
    pub anomaly_detected: bool,
    pub anomaly_reason: Option<String>,
    pub total_samples: u32,
}

/// Configuration for baseline acquisition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcquisitionConfig {
    pub method: Method,
    /// Minimum number of samples required before declaring anomaly.
    pub min_samples: usize,
}

/// Samples collected for a single probe during baseline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeSamples {
    pub name: String,
    /// Numeric values collected (e.g., response time in ms).
    pub values: Vec<f64>,
    /// Number of errors observed during sampling.
    pub errors: u32,
    /// Total attempts (successful + failed).
    pub total_attempts: u32,
    /// Epoch nanosecond timestamps for each sample in `values`.
    ///
    /// Used for Arrow conversion and MTTR analysis. May be empty if
    /// the caller does not track per-sample timestamps.
    pub sampled_at: Vec<i64>,
}
