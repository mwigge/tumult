//! Baseline acquisition — orchestrates warmup, sampling, and derivation.
//!
//! The acquisition module takes pre-collected probe samples and produces
//! a complete `AcquisitionResult` with per-probe statistics, tolerance
//! bounds, and anomaly detection.
//!
//! This module is intentionally synchronous. The async probe execution
//! loop lives in `tumult-core`'s runner; this module consumes the
//! collected samples and derives the baseline.

mod derive;
mod stream;
mod types;

pub use derive::derive_baseline;
pub use stream::AcquisitionStream;
pub use types::{AcquisitionConfig, AcquisitionError, AcquisitionResult, ProbeSamples, ProbeStats};
