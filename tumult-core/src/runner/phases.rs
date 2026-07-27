//! Phase computation: analysis (Phase 4), probe sampling, and during/post
//! result construction.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use crate::types::{
    ActivityResult, ActivityStatus, ActivityType, AnalysisResult, DuringResult, ExpectedOutcome,
    Experiment, ExperimentStatus, Hypothesis, PostResult, ProbeDuring, ProbePost,
};

use opentelemetry::trace::{TraceContextExt, Tracer};
use opentelemetry::KeyValue;

use super::activity::probe_outcome_ok;
use super::telemetry::{
    current_span_id, current_trace_id, epoch_nanos_now, fault_attributes, plugin_name,
    set_span_status_from_outcome, target_attributes,
};
use super::{ActivityExecutor, TRACER_NAME};

/// Probe samples grouped by probe name, each group in collection order.
pub(crate) type ProbeSamples = Vec<(String, Vec<ActivityResult>)>;

/// Mutable per-probe sample sink used while sampling is in progress.
pub(crate) type ProbeSampleMap = HashMap<String, Vec<ActivityResult>>;

/// Compute Phase 4 analysis from estimate and actual results.
///
/// Reserved/unpopulated fields in this version (kept as `None` rather than
/// filled with fabricated values):
/// - `estimate_recovery_delta_s` — requires comparing the estimated recovery
///   time against the measured post-phase recovery; not computed yet.
/// - `trend` — requires historical journals from the analytics store; not
///   computed yet.
///
/// `estimate_accuracy` and `resilience_score` are populated but deliberately
/// coarse binary indicators (1.0 when the actual recovery outcome matches
/// the estimate / the run completed, 0.0 otherwise); richer scoring is left
/// to the analytics layer.
pub(crate) fn compute_analysis(
    experiment: &Experiment,
    status: &ExperimentStatus,
) -> Option<AnalysisResult> {
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
        // Reserved: not populated in this version (see fn docs).
        estimate_recovery_delta_s: None,
        // Reserved: not populated in this version (see fn docs).
        trend: None,
        resilience_score: if actual_recovered {
            Some(1.0)
        } else {
            Some(0.0)
        },
    })
}

/// Run every hypothesis probe once, returning the sample results and
/// whether all probes passed their tolerance in this round.
fn sample_probe_round(
    hypothesis: &Hypothesis,
    executor: &dyn ActivityExecutor,
) -> (Vec<ActivityResult>, bool) {
    let mut round = Vec::with_capacity(hypothesis.probes.len());
    let mut all_within_tolerance = true;

    for probe in &hypothesis.probes {
        // Each sample is a real probe execution — give it the same span
        // coverage as method-phase activities so during/post operations
        // show up in traces and join the experiment's trace tree.
        let tracer = opentelemetry::global::tracer(TRACER_NAME);
        let mut attrs = vec![
            KeyValue::new("resilience.action.name", probe.name.clone()),
            KeyValue::new("resilience.activity.type", ActivityType::Probe.to_string()),
        ];
        attrs.extend(target_attributes(probe));
        attrs.extend(fault_attributes(probe));
        let span = tracer
            .span_builder("resilience.probe")
            .with_attributes(attrs)
            .start(&tracer);
        let cx = opentelemetry::Context::current_with_span(span);
        let guard = cx.attach();

        let start = Instant::now();
        let started_at_ns = epoch_nanos_now();
        let outcome = executor.execute(probe);
        set_span_status_from_outcome(outcome.success, outcome.error.as_deref());
        tumult_otel::instrument::record_probe(
            tumult_otel::TumultMetrics::global(),
            &plugin_name(probe),
            &probe.name,
            start,
            outcome.success,
        );
        let trace_id = current_trace_id();
        let span_id = current_span_id();
        drop(guard);
        // Probe durations never exceed u64::MAX milliseconds (~585M years).
        #[allow(clippy::cast_possible_truncation)]
        let elapsed = start.elapsed().as_millis() as u64;

        let within_tolerance = probe_outcome_ok(probe, outcome.success, outcome.output.as_deref());
        if !within_tolerance {
            all_within_tolerance = false;
        }
        let status = if within_tolerance {
            ActivityStatus::Succeeded
        } else {
            ActivityStatus::Failed
        };

        round.push(ActivityResult {
            name: probe.name.clone(),
            activity_type: ActivityType::Probe,
            status,
            started_at_ns,
            duration_ms: elapsed,
            output: outcome.output,
            error: outcome.error,
            trace_id,
            span_id,
        });
    }

    (round, all_within_tolerance)
}

/// Append one round of samples to the per-probe map.
fn append_round(per_probe: &mut ProbeSampleMap, round: Vec<ActivityResult>) {
    for sample in round {
        per_probe
            .entry(sample.name.clone())
            .or_default()
            .push(sample);
    }
}

/// Collect during-phase probe samples into `per_probe`, one round every
/// `interval`, until the method finishes (signalled by `stop_rx`
/// disconnecting) or `max_samples` rounds have been taken.
///
/// Samples are written to the shared map incrementally so the caller can
/// still use whatever was collected if the sampling thread panics mid-run.
pub(crate) fn collect_during_samples(
    hypothesis: &Hypothesis,
    executor: &dyn ActivityExecutor,
    interval: Duration,
    max_samples: u32,
    stop_rx: &Receiver<()>,
    per_probe: &Mutex<ProbeSampleMap>,
) {
    for round in 0..max_samples {
        let (samples, _) = sample_probe_round(hypothesis, executor);
        {
            let mut guard = per_probe
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            append_round(&mut guard, samples);
        }
        if round + 1 == max_samples {
            break;
        }
        // The receive timeout doubles as the inter-sample pause: it returns
        // early (`Disconnected`) as soon as the runner drops the stop sender,
        // so a fast method incurs no sampling latency.
        match stop_rx.recv_timeout(interval) {
            Err(RecvTimeoutError::Timeout) => {}
            Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// Collect post-phase probe samples, one round every `interval`, until every
/// probe passes its tolerance (recovery detected), `timeout` elapses, or the
/// run is cancelled. Always takes at least one round, so a system that has
/// already recovered is observed without any added latency.
pub(crate) fn collect_post_samples(
    hypothesis: &Hypothesis,
    executor: &dyn ActivityExecutor,
    interval: Duration,
    timeout: Duration,
    cancellation_token: Option<&CancellationToken>,
) -> ProbeSamples {
    let mut per_probe = ProbeSampleMap::new();
    let deadline = Instant::now() + timeout;

    loop {
        let (samples, recovered) = sample_probe_round(hypothesis, executor);
        append_round(&mut per_probe, samples);

        let cancelled = cancellation_token.is_some_and(CancellationToken::is_cancelled);
        if recovered || cancelled || Instant::now() + interval > deadline {
            break;
        }
        std::thread::sleep(interval);
    }

    per_probe.into_iter().collect()
}

/// Build a `DuringResult` from probe samples collected while fault injection
/// was active. `sample_interval_s` is the actual interval the sampler ran
/// with. Returns `None` if no samples were collected.
///
/// The degradation fields (`degradation_onset_s`, `degradation_peak_s`,
/// `degradation_magnitude`, `graceful_degradation`) are reserved and always
/// `None` in this version: deriving them requires latency-shape analysis
/// over the samples that is not implemented yet, so they are left
/// unpopulated rather than filled with placeholder values.
pub(crate) fn build_during_result(
    started_at_ns: i64,
    ended_at_ns: i64,
    sample_interval_s: f64,
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
        sample_interval_s,
        probes,
        degradation_onset_s: None,
        degradation_peak_s: None,
        degradation_magnitude: None,
        graceful_degradation: None,
    })
}

/// Build a `PostResult` from probe samples collected after method completion
/// to measure system recovery. Returns `None` if no samples were collected.
///
/// `residual_degradation`, `data_integrity_verified`, and
/// `data_loss_detected` are reserved and always `None` in this version (they
/// require checks the runner does not perform yet), rather than filled with
/// placeholder values.
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
            // A probe has returned to baseline when its most recent sample
            // passed -- earlier failures are fine as long as the probe
            // recovered by the end of the post-phase sampling window.
            let returned_to_baseline = samples
                .last()
                .is_some_and(|s| s.status == ActivityStatus::Succeeded);
            let recovery_time_s = if failed == 0 {
                0.0
            } else {
                // Recovery point: the first succeeding sample after the last
                // failure, or the last (failing) sample when the probe never
                // recovered within the sampling window.
                let last_failure_idx = samples
                    .iter()
                    .rposition(|s| s.status == ActivityStatus::Failed)
                    .unwrap_or(0);
                let recovery_marker_ns = samples
                    .get(last_failure_idx + 1)
                    .map_or(samples[last_failure_idx].started_at_ns, |s| s.started_at_ns);
                // Nanosecond delta to seconds; i64 → f64 precision loss acceptable
                // for human-readable recovery time display.
                #[allow(clippy::cast_precision_loss)]
                let secs = (recovery_marker_ns - started_at_ns) as f64 / 1_000_000_000.0;
                secs
            };

            ProbePost {
                name: name.clone(),
                mean,
                p95,
                error_rate,
                returned_to_baseline,
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
