//! Data structures for baseline acquisition inputs and results.

use crate::tolerance::Method;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum AcquisitionError {
    #[error("no probes provided")]
    NoProbes,
    #[error("probe '{name}' has no samples after warmup")]
    NoSamplesAfterWarmup { name: String },
}

/// Per-probe statistics derived from baseline samples.
#[derive(Debug, Clone)]
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
}

/// Result of a complete baseline acquisition.
#[derive(Debug, Clone)]
pub struct AcquisitionResult {
    pub probes: Vec<ProbeStats>,
    pub tolerance_lower: f64,
    pub tolerance_upper: f64,
    pub anomaly_detected: bool,
    pub anomaly_reason: Option<String>,
    pub total_samples: u32,
}

/// Configuration for baseline acquisition.
#[derive(Debug, Clone)]
pub struct AcquisitionConfig {
    pub method: Method,
    /// Minimum number of samples required before declaring anomaly.
    pub min_samples: usize,
}

/// Samples collected for a single probe during baseline.
#[derive(Debug, Clone)]
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
