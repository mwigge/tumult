//! Streaming baseline acquisition builder.

use super::derive::derive_baseline;
use super::types::{AcquisitionConfig, AcquisitionError, AcquisitionResult, ProbeSamples};

/// Streaming baseline acquisition builder.
///
/// Accepts probe samples incrementally — one value at a time — and
/// derives the final baseline when [`finish`] is called.
///
/// This is a synchronous, allocation-friendly alternative to building a
/// complete [`ProbeSamples`] vector before calling [`derive_baseline`].
/// The async probe loop pushes each result here as it arrives; the runner
/// calls [`finish`] at the end of the warmup window.
///
/// # Examples
///
/// ```
/// use tumult_baseline::acquisition::{AcquisitionStream, AcquisitionConfig};
/// use tumult_baseline::tolerance::Method;
///
/// let mut stream = AcquisitionStream::new(
///     "api-latency".into(),
///     AcquisitionConfig {
///         method: Method::MeanStddev { sigma: 2.0 },
///         min_samples: 3,
///     },
/// );
///
/// stream.push_sample(100.0);
/// stream.push_sample(102.0);
/// stream.push_sample(98.0);
///
/// let result = stream.finish().unwrap();
/// assert_eq!(result.probes.len(), 1);
/// assert!(!result.anomaly_detected);
/// ```
pub struct AcquisitionStream {
    probe_name: String,
    config: AcquisitionConfig,
    values: Vec<f64>,
    errors: u32,
    total_attempts: u32,
}

impl AcquisitionStream {
    /// Creates a new streaming acquisition for a single probe.
    #[must_use]
    pub fn new(probe_name: String, config: AcquisitionConfig) -> Self {
        Self {
            probe_name,
            config,
            values: Vec::new(),
            errors: 0,
            total_attempts: 0,
        }
    }

    /// Records a successful probe sample value.
    pub fn push_sample(&mut self, value: f64) {
        self.values.push(value);
        self.total_attempts += 1;
    }

    /// Records a probe error (no value collected).
    pub fn push_error(&mut self) {
        self.errors += 1;
        self.total_attempts += 1;
    }

    /// Returns the number of successful samples pushed so far.
    #[must_use]
    pub fn sample_count(&self) -> usize {
        self.values.len()
    }

    /// Derives the baseline from all pushed samples.
    ///
    /// Equivalent to calling [`derive_baseline`] with the accumulated
    /// [`ProbeSamples`]. Does not consume the stream — samples can continue
    /// to be pushed after calling `derive`.
    ///
    /// # Performance
    ///
    /// This method clones the entire `values` buffer on every call. For
    /// one-shot derivation, prefer [`Self::finish`] which moves the buffer
    /// instead of cloning it.
    ///
    /// # Errors
    ///
    /// Returns [`AcquisitionError::NoSamplesAfterWarmup`] if no successful
    /// samples have been pushed.
    pub fn derive(&self) -> Result<AcquisitionResult, AcquisitionError> {
        let probe = ProbeSamples {
            name: self.probe_name.clone(),
            values: self.values.clone(),
            errors: self.errors,
            total_attempts: self.total_attempts,
            sampled_at: vec![],
        };
        derive_baseline(&[probe], &self.config)
    }

    /// Finalises the stream and derives the baseline, consuming `self`.
    ///
    /// # Errors
    ///
    /// Returns [`AcquisitionError::NoSamplesAfterWarmup`] if no successful
    /// samples were pushed.
    pub fn finish(self) -> Result<AcquisitionResult, AcquisitionError> {
        let probe = ProbeSamples {
            name: self.probe_name,
            values: self.values,
            errors: self.errors,
            total_attempts: self.total_attempts,
            sampled_at: vec![],
        };
        derive_baseline(&[probe], &self.config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tolerance::Method;

    fn config_mean_stddev() -> AcquisitionConfig {
        AcquisitionConfig {
            method: Method::MeanStddev { sigma: 2.0 },
            min_samples: 5,
        }
    }

    #[test]
    fn acquisition_stream_finish_derives_baseline() {
        let mut stream = AcquisitionStream::new("latency".into(), config_mean_stddev());
        for v in [100.0, 102.0, 98.0, 101.0, 99.0] {
            stream.push_sample(v);
        }
        let result = stream.finish().unwrap();
        assert_eq!(result.probes.len(), 1);
        assert_eq!(result.probes[0].name, "latency");
        assert!((result.probes[0].mean - 100.0).abs() < 1.0);
        assert!(!result.anomaly_detected);
    }

    #[test]
    fn acquisition_stream_push_error_tracks_error_rate() {
        let mut stream = AcquisitionStream::new("check".into(), config_mean_stddev());
        for v in [100.0, 101.0, 99.0, 100.0, 102.0] {
            stream.push_sample(v);
        }
        stream.push_error();
        stream.push_error();
        let result = stream.finish().unwrap();
        let expected_rate = 2.0 / 7.0;
        assert!((result.probes[0].error_rate - expected_rate).abs() < 0.001);
    }

    #[test]
    fn acquisition_stream_derive_does_not_consume() {
        let mut stream = AcquisitionStream::new("latency".into(), config_mean_stddev());
        for v in [100.0, 102.0, 98.0, 101.0, 99.0] {
            stream.push_sample(v);
        }
        // derive() borrows; can push more after
        let mid_result = stream.derive().unwrap();
        assert_eq!(mid_result.probes[0].samples, 5);
        stream.push_sample(103.0);
        assert_eq!(stream.sample_count(), 6);
        let final_result = stream.finish().unwrap();
        assert_eq!(final_result.probes[0].samples, 6);
    }
}
