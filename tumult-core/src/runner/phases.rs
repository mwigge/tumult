//! Phase computation: analysis (Phase 4), probe sampling, and during/post
//! result construction.

use std::time::Instant;

use crate::types::{
    ActivityResult, ActivityStatus, ActivityType, AnalysisResult, DuringResult, ExpectedOutcome,
    Experiment, ExperimentStatus, Hypothesis, PostResult, ProbeDuring, ProbePost, SpanId, TraceId,
};

use super::telemetry::epoch_nanos_now;
use super::ActivityExecutor;

/// Compute Phase 4 analysis from estimate and actual results.
pub(crate) fn compute_analysis(experiment: &Experiment, status: &ExperimentStatus) -> Option<AnalysisResult> {
    let estimate = experiment.estimate.as_ref()?;

    // Compare estimate vs actual outcome
    let actual_recovered = *status == ExperimentStatus::Completed;
    let estimated_recovered = estimate.expected_outcome == ExpectedOutcome::Recovered;
    let estimate_accuracy = if actual_recovered == estimated_recovered {
        Some(1.0)
    } else {
        Some(0.0)
    };

    Some(AnalysisResult {
        estimate_accuracy,
        estimate_recovery_delta_s: None,
        trend: None,
        resilience_score: if actual_recovered {
            Some(1.0)
        } else {
            Some(0.0)
        },
    })
}

/// Run hypothesis probes a fixed number of times and return per-probe
/// sample results. Used for during-phase and post-phase collection.
pub(crate) fn collect_probe_samples(
    hypothesis: &Hypothesis,
    executor: &dyn ActivityExecutor,
    count: usize,
) -> Vec<(String, Vec<ActivityResult>)> {
    let mut per_probe: std::collections::HashMap<String, Vec<ActivityResult>> =
        std::collections::HashMap::new();

    for _ in 0..count {
        for probe in &hypothesis.probes {
            let start = Instant::now();
            let started_at_ns = epoch_nanos_now();
            let outcome = executor.execute(probe);
            // Probe durations never exceed u64::MAX milliseconds (~585M years).
            #[allow(clippy::cast_possible_truncation)]
            let elapsed = start.elapsed().as_millis() as u64;

            let status = if outcome.success {
                ActivityStatus::Succeeded
            } else {
                ActivityStatus::Failed
            };

            per_probe
                .entry(probe.name.clone())
                .or_default()
                .push(ActivityResult {
                    name: probe.name.clone(),
                    activity_type: ActivityType::Probe,
                    status,
                    started_at_ns,
                    duration_ms: elapsed,
                    output: outcome.output,
                    error: outcome.error,
                    trace_id: TraceId::empty(),
                    span_id: SpanId::empty(),
                });
        }
    }

    per_probe.into_iter().collect()
}

/// Build a `DuringResult` from probe samples collected while fault injection
/// was active. Returns `None` if no samples were collected.
pub(crate) fn build_during_result(
    started_at_ns: i64,
    ended_at_ns: i64,
    probe_samples: &[(String, Vec<ActivityResult>)],
) -> Option<DuringResult> {
    if probe_samples.is_empty() {
        return None;
    }

    // Nanosecond delta converted to seconds; i64 → f64 precision loss is
    // acceptable for human-readable fault duration display.
    #[allow(clippy::cast_precision_loss)]
    let fault_active_s = (ended_at_ns - started_at_ns) as f64 / 1_000_000_000.0;

    let probes: Vec<ProbeDuring> = probe_samples
        .iter()
        .map(|(name, samples)| {
            // Sample counts in chaos experiments are always << u32::MAX.
            #[allow(clippy::cast_possible_truncation)]
            let total = samples.len() as u32;
            // Sample counts in chaos experiments are always << u32::MAX.
            #[allow(clippy::cast_possible_truncation)]
            let failed = samples
                .iter()
                .filter(|s| s.status == ActivityStatus::Failed)
                .count() as u32;
            // u64 → f64 precision loss is acceptable for millisecond statistics display.
            #[allow(clippy::cast_precision_loss)]
            let durations: Vec<f64> = samples.iter().map(|s| s.duration_ms as f64).collect();
            // usize → f64 precision loss is acceptable for mean calculation with small N.
            #[allow(clippy::cast_precision_loss)]
            let mean = if durations.is_empty() {
                0.0
            } else {
                durations.iter().sum::<f64>() / durations.len() as f64
            };
            let max = durations.iter().copied().fold(f64::NAN, f64::max);
            let min = durations.iter().copied().fold(f64::NAN, f64::min);
            let breached_at_ns = samples
                .iter()
                .find(|s| s.status == ActivityStatus::Failed)
                .map(|s| s.started_at_ns);

            ProbeDuring {
                name: name.clone(),
                samples: total,
                mean,
                max,
                min,
                error_rate: if total > 0 {
                    f64::from(failed) / f64::from(total)
                } else {
                    0.0
                },
                breached_at_ns,
                breach_count: failed,
            }
        })
        .collect();

    Some(DuringResult {
        started_at_ns,
        ended_at_ns,
        fault_active_s,
        sample_interval_s: 1.0,
        probes,
        degradation_onset_s: None,
        degradation_peak_s: None,
        degradation_magnitude: None,
        graceful_degradation: None,
    })
}

/// Build a `PostResult` from probe samples collected after method completion
/// to measure system recovery. Returns `None` if no samples were collected.
pub(crate) fn build_post_result(
    started_at_ns: i64,
    ended_at_ns: i64,
    probe_samples: &[(String, Vec<ActivityResult>)],
) -> Option<PostResult> {
    if probe_samples.is_empty() {
        return None;
    }

    // Nanosecond delta converted to seconds; i64 → f64 precision loss is
    // acceptable for human-readable post-phase duration display.
    #[allow(clippy::cast_precision_loss)]
    let duration_s = (ended_at_ns - started_at_ns) as f64 / 1_000_000_000.0;
    // Total sample counts in chaos experiments are always << u32::MAX.
    #[allow(clippy::cast_possible_truncation)]
    let total_samples = probe_samples.iter().map(|(_, s)| s.len()).sum::<usize>() as u32;

    let probes: Vec<ProbePost> = probe_samples
        .iter()
        .map(|(name, samples)| {
            // u64 → f64 precision loss is acceptable for millisecond statistics display.
            #[allow(clippy::cast_precision_loss)]
            let sample_ms: Vec<f64> = samples.iter().map(|s| s.duration_ms as f64).collect();
            // usize → f64 precision loss is acceptable for mean calculation with small N.
            #[allow(clippy::cast_precision_loss)]
            let mean = if sample_ms.is_empty() {
                0.0
            } else {
                sample_ms.iter().sum::<f64>() / sample_ms.len() as f64
            };
            let mut sorted = sample_ms.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let p95 = if sorted.is_empty() {
                0.0
            } else {
                // Percentile index computation: usize → f64 and f64 → usize casts
                // are acceptable for small sample sizes used in chaos probe sampling.
                #[allow(
                    clippy::cast_precision_loss,
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss
                )]
                let idx = ((sorted.len() as f64 * 0.95) as usize).min(sorted.len() - 1);
                sorted[idx]
            };
            let failed = samples
                .iter()
                .filter(|s| s.status == ActivityStatus::Failed)
                .count();
            // usize → f64 precision loss is acceptable for error rate display.
            #[allow(clippy::cast_precision_loss)]
            let error_rate = if samples.is_empty() {
                0.0
            } else {
                failed as f64 / samples.len() as f64
            };
            let all_succeeded = failed == 0;
            let recovery_time_s = if all_succeeded {
                0.0
            } else {
                let last_failure_ns = samples
                    .iter()
                    .rev()
                    .find(|s| s.status == ActivityStatus::Failed)
                    .map_or(started_at_ns, |s| s.started_at_ns);
                // Nanosecond delta to seconds; i64 → f64 precision loss acceptable
                // for human-readable recovery time display.
                #[allow(clippy::cast_precision_loss)]
                let secs = (last_failure_ns - started_at_ns) as f64 / 1_000_000_000.0;
                secs
            };

            ProbePost {
                name: name.clone(),
                mean,
                p95,
                error_rate,
                returned_to_baseline: all_succeeded,
                recovery_time_s,
            }
        })
        .collect();

    let full_recovery = probes.iter().all(|p| p.returned_to_baseline);
    let recovery_time_s = probes
        .iter()
        .map(|p| p.recovery_time_s)
        .fold(0.0_f64, f64::max);

    // MTTR: when full recovery is observed, set to the maximum recovery time
    // across all probes; when recovery was never achieved, leave as None.
    let mttr_s = if full_recovery {
        Some(recovery_time_s)
    } else {
        None
    };

    Some(PostResult {
        started_at_ns,
        ended_at_ns,
        duration_s,
        samples: total_samples,
        probes,
        recovery_time_s,
        full_recovery,
        residual_degradation: None,
        data_integrity_verified: None,
        data_loss_detected: None,
        mttr_s,
    })
}
