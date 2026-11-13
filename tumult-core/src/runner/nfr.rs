//! Objective evaluation: turn a run's measurements into
//! [`NfrVerdict`]s and the recovery-target outcome.
//!
//! Metric catalogue (the `metric` field of an [`NfrObjective`]):
//!
//! | metric | source |
//! |---|---|
//! | `load.throughput_rps`, `load.latency_p50_ms`, `load.latency_p95_ms`, `load.latency_p99_ms`, `load.error_rate`, `load.total_requests` | the load result |
//! | `recovery.mttr_s`, `recovery.time_s` | the post-phase result |
//! | `during.error_rate`, `during.availability`, `post.error_rate`, `post.availability` | raw probe samples of that phase |
//! | `probe.<name>.mean_ms`, `probe.<name>.max_ms`, `probe.<name>.p95_ms`, `probe.<name>.error_rate` | raw during-phase samples of one probe |
//! | `fault.duration_s` | the during-phase result |
//!
//! A metric that the run did not produce yields an `inconclusive` verdict:
//! an objective is never silently met.

use crate::types::{
    DuringResult, Experiment, LoadResult, NfrCriticality, NfrKind, NfrObjective, NfrOperator,
    NfrState, NfrVerdict, ObjectiveOutcome, PostResult, ProbeSample, SamplePhase,
};

/// Measurements an objective can be evaluated against.
pub(crate) struct Measurements<'a> {
    pub during: Option<&'a DuringResult>,
    pub post: Option<&'a PostResult>,
    pub load: Option<&'a LoadResult>,
    pub samples: &'a [ProbeSample],
}

/// Evaluate every declared objective and the recovery targets. Returns
/// `None` when the definition declares neither objectives nor targets, so
/// journals of plain experiments stay unchanged.
pub(crate) fn evaluate(experiment: &Experiment, m: &Measurements<'_>) -> Option<ObjectiveOutcome> {
    if experiment.nfr.is_empty()
        && experiment.rto_target_s.is_none()
        && experiment.rpo_target_s.is_none()
    {
        return None;
    }
    let mut verdicts: Vec<NfrVerdict> = experiment
        .nfr
        .iter()
        .map(|o| verdict_for(o, resolve(&o.metric, m)))
        .collect();
    let rto_measured_s = m.post.and_then(|p| p.mttr_s);
    if let Some(target) = experiment.rto_target_s {
        let implicit = NfrObjective {
            id: "rto".into(),
            kind: NfrKind::Rto,
            metric: "recovery.mttr_s".into(),
            operator: NfrOperator::Lte,
            threshold: target,
            window_s: None,
            criticality: NfrCriticality::Blocking,
            description: Some("recovery time objective".into()),
        };
        verdicts.push(verdict_for(&implicit, rto_measured_s));
    }
    let rpo_measured_s = verdicts
        .iter()
        .find(|v| v.kind == NfrKind::Rpo)
        .and_then(|v| v.observed);
    if let Some(target) = experiment.rpo_target_s {
        if !experiment.nfr.iter().any(|o| o.kind == NfrKind::Rpo) {
            // No probe measures data loss: record the target and say so.
            verdicts.push(NfrVerdict {
                nfr_id: "rpo".into(),
                kind: NfrKind::Rpo,
                metric: "rpo".into(),
                operator: NfrOperator::Lte,
                threshold: target,
                observed: None,
                state: NfrState::Inconclusive,
                margin: None,
                criticality: NfrCriticality::Blocking,
            });
        }
    }
    Some(ObjectiveOutcome {
        rto_target_s: experiment.rto_target_s,
        rpo_target_s: experiment.rpo_target_s,
        rto_measured_s,
        rpo_measured_s,
        verdicts,
    })
}

/// Compare one observation against an objective. Pure.
pub(crate) fn verdict_for(o: &NfrObjective, observed: Option<f64>) -> NfrVerdict {
    let (state, margin) = match observed {
        None => (NfrState::Inconclusive, None),
        Some(v) if !v.is_finite() => (NfrState::Inconclusive, None),
        Some(v) => {
            let margin = match o.operator {
                NfrOperator::Lte => o.threshold - v,
                NfrOperator::Gte => v - o.threshold,
                NfrOperator::Eq => -(v - o.threshold).abs(),
            };
            let met = match o.operator {
                NfrOperator::Lte => v <= o.threshold,
                NfrOperator::Gte => v >= o.threshold,
                NfrOperator::Eq => (v - o.threshold).abs() <= f64::EPSILON,
            };
            (
                if met {
                    NfrState::Met
                } else {
                    NfrState::Breached
                },
                Some(margin),
            )
        }
    };
    NfrVerdict {
        nfr_id: o.id.clone(),
        kind: o.kind,
        metric: o.metric.clone(),
        operator: o.operator,
        threshold: o.threshold,
        observed,
        state,
        margin,
        criticality: o.criticality,
    }
}

/// Look a metric name up in the measurements. `None` when the run did not
/// produce it.
pub(crate) fn resolve(metric: &str, m: &Measurements<'_>) -> Option<f64> {
    if let Some(rest) = metric.strip_prefix("load.") {
        let l = m.load?;
        // A run that sent no request measured no latency and no error
        // rate; its zeroes are absence of evidence, not a fast service.
        let measured = |value: f64| (l.total_requests > 0).then_some(value);
        // Nor is there a latency when every request failed.
        let answered = |value: f64| measured(value).filter(|_| l.error_rate < 1.0);
        return match rest {
            "throughput_rps" => Some(l.throughput_rps),
            "latency_p50_ms" => answered(l.latency_p50_ms),
            "latency_p95_ms" => answered(l.latency_p95_ms),
            "latency_p99_ms" => answered(l.latency_p99_ms),
            "error_rate" => measured(l.error_rate),
            // u64 → f64: request counts are far below 2^53.
            #[allow(clippy::cast_precision_loss)]
            "total_requests" => Some(l.total_requests as f64),
            _ => None,
        };
    }
    if let Some(rest) = metric.strip_prefix("recovery.") {
        let p = m.post?;
        return match rest {
            "mttr_s" => p.mttr_s,
            "time_s" => Some(p.recovery_time_s),
            _ => None,
        };
    }
    if let Some(rest) = metric.strip_prefix("fault.") {
        return match rest {
            "duration_s" => m.during.map(|d| d.fault_active_s),
            _ => None,
        };
    }
    for (prefix, phase) in [
        ("during.", SamplePhase::During),
        ("post.", SamplePhase::Post),
    ] {
        if let Some(rest) = metric.strip_prefix(prefix) {
            let samples: Vec<&ProbeSample> =
                m.samples.iter().filter(|s| s.phase == phase).collect();
            let err = error_rate(&samples)?;
            return match rest {
                "error_rate" => Some(err),
                "availability" => Some(1.0 - err),
                _ => None,
            };
        }
    }
    if let Some(rest) = metric.strip_prefix("probe.") {
        let (name, stat) = rest.rsplit_once('.')?;
        let samples: Vec<&ProbeSample> = m
            .samples
            .iter()
            .filter(|s| s.phase == SamplePhase::During && s.probe == name)
            .collect();
        if samples.is_empty() {
            return None;
        }
        // u64 → f64: millisecond durations are far below 2^53.
        #[allow(clippy::cast_precision_loss)]
        let mut ms: Vec<f64> = samples.iter().map(|s| s.duration_ms as f64).collect();
        return match stat {
            // usize → f64: sample counts are small.
            #[allow(clippy::cast_precision_loss)]
            "mean_ms" => Some(ms.iter().sum::<f64>() / ms.len() as f64),
            "max_ms" => ms.iter().copied().reduce(f64::max),
            "p95_ms" => {
                ms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                // The index is clamped into range, so the truncation is intended.
                #[allow(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    clippy::cast_precision_loss
                )]
                let idx = (((ms.len() as f64) * 0.95).ceil() as usize).clamp(1, ms.len()) - 1;
                Some(ms[idx])
            }
            "error_rate" => error_rate(&samples),
            _ => None,
        };
    }
    None
}

/// Fraction of samples outside tolerance; `None` without samples.
fn error_rate(samples: &[&ProbeSample]) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    let failed = samples.iter().filter(|s| !s.ok).count();
    // usize → f64: sample counts are small.
    #[allow(clippy::cast_precision_loss)]
    Some(failed as f64 / samples.len() as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::LoadTool;

    fn objective(id: &str, metric: &str, op: NfrOperator, threshold: f64) -> NfrObjective {
        NfrObjective {
            id: id.into(),
            kind: NfrKind::Latency,
            metric: metric.into(),
            operator: op,
            threshold,
            window_s: None,
            criticality: NfrCriticality::Major,
            description: None,
        }
    }

    fn sample(probe: &str, phase: SamplePhase, ts: i64, ms: u64, ok: bool) -> ProbeSample {
        ProbeSample {
            probe: probe.into(),
            phase,
            ts_ns: ts,
            duration_ms: ms,
            ok,
        }
    }

    fn load() -> LoadResult {
        LoadResult {
            tool: LoadTool::K6,
            started_at_ns: 0,
            ended_at_ns: 10,
            duration_s: 10.0,
            vus: 5,
            throughput_rps: 120.0,
            latency_p50_ms: 40.0,
            latency_p95_ms: 180.0,
            latency_p99_ms: 400.0,
            error_rate: 0.02,
            total_requests: 1200,
            thresholds_met: true,
            samples: vec![],
            execution: crate::types::LoadExecution::default(),
        }
    }

    #[test]
    fn verdict_is_met_breached_or_inconclusive_with_signed_margin() {
        let o = objective("p95", "load.latency_p95_ms", NfrOperator::Lte, 200.0);
        let v = verdict_for(&o, Some(180.0));
        assert_eq!(v.state, NfrState::Met);
        assert_eq!(v.margin, Some(20.0));

        let v = verdict_for(&o, Some(250.0));
        assert_eq!(v.state, NfrState::Breached);
        assert_eq!(v.margin, Some(-50.0));

        let v = verdict_for(&o, None);
        assert_eq!(v.state, NfrState::Inconclusive);
        assert_eq!(v.margin, None);

        let g = objective("avail", "during.availability", NfrOperator::Gte, 0.99);
        let v = verdict_for(&g, Some(0.995));
        assert_eq!(v.state, NfrState::Met);
        assert!((v.margin.unwrap() - 0.005).abs() < 1e-9);
    }

    #[test]
    fn resolves_load_recovery_phase_and_probe_metrics() {
        let l = load();
        let post = PostResult {
            started_at_ns: 0,
            ended_at_ns: 5_000_000_000,
            duration_s: 5.0,
            samples: 3,
            probes: vec![],
            recovery_time_s: 4.0,
            full_recovery: true,
            residual_degradation: None,
            data_integrity_verified: None,
            data_loss_detected: None,
            mttr_s: Some(3.5),
        };
        let samples = vec![
            sample("health", SamplePhase::During, 1, 100, true),
            sample("health", SamplePhase::During, 2, 300, false),
            sample("health", SamplePhase::During, 3, 200, true),
            sample("health", SamplePhase::During, 4, 900, true),
            sample("health", SamplePhase::Post, 5, 50, true),
        ];
        let m = Measurements {
            during: None,
            post: Some(&post),
            load: Some(&l),
            samples: &samples,
        };
        assert_eq!(resolve("load.latency_p95_ms", &m), Some(180.0));
        assert_eq!(resolve("load.total_requests", &m), Some(1200.0));
        // A load run that sent nothing has no latency or error evidence:
        // its zeroes must not satisfy an upper bound.
        let idle = LoadResult {
            total_requests: 0,
            throughput_rps: 0.0,
            latency_p50_ms: 0.0,
            latency_p95_ms: 0.0,
            latency_p99_ms: 0.0,
            error_rate: 0.0,
            thresholds_met: false,
            ..load()
        };
        let no_requests = Measurements {
            during: None,
            post: Some(&post),
            load: Some(&idle),
            samples: &samples,
        };
        assert_eq!(resolve("load.latency_p50_ms", &no_requests), None);
        // Every request failed: an error rate, but still no latency.
        let all_failed = LoadResult {
            total_requests: 40,
            latency_p50_ms: 0.0,
            latency_p95_ms: 0.0,
            latency_p99_ms: 0.0,
            error_rate: 1.0,
            thresholds_met: false,
            ..load()
        };
        let nothing_answered = Measurements {
            during: None,
            post: Some(&post),
            load: Some(&all_failed),
            samples: &samples,
        };
        assert_eq!(resolve("load.latency_p95_ms", &nothing_answered), None);
        assert_eq!(resolve("load.error_rate", &nothing_answered), Some(1.0));
        assert_eq!(resolve("load.latency_p95_ms", &no_requests), None);
        assert_eq!(resolve("load.latency_p99_ms", &no_requests), None);
        assert_eq!(resolve("load.error_rate", &no_requests), None);
        assert_eq!(resolve("load.throughput_rps", &no_requests), Some(0.0));
        assert_eq!(resolve("load.total_requests", &no_requests), Some(0.0));
        assert_eq!(resolve("recovery.mttr_s", &m), Some(3.5));
        assert_eq!(resolve("recovery.time_s", &m), Some(4.0));
        assert_eq!(resolve("during.error_rate", &m), Some(0.25));
        assert_eq!(resolve("during.availability", &m), Some(0.75));
        assert_eq!(resolve("post.availability", &m), Some(1.0));
        assert_eq!(resolve("probe.health.mean_ms", &m), Some(375.0));
        assert_eq!(resolve("probe.health.max_ms", &m), Some(900.0));
        assert_eq!(resolve("probe.health.p95_ms", &m), Some(900.0));
        assert_eq!(resolve("probe.health.error_rate", &m), Some(0.25));
        assert_eq!(resolve("probe.other.mean_ms", &m), None);
        assert_eq!(resolve("fault.duration_s", &m), None);
        assert_eq!(resolve("nope.metric", &m), None);
    }

    #[test]
    fn evaluate_is_none_without_objectives_and_adds_implicit_rto_rpo() {
        let mut exp = Experiment::default();
        let m = Measurements {
            during: None,
            post: None,
            load: None,
            samples: &[],
        };
        assert!(evaluate(&exp, &m).is_none());

        exp.rto_target_s = Some(900.0);
        exp.rpo_target_s = Some(60.0);
        exp.nfr.push(objective(
            "p95",
            "load.latency_p95_ms",
            NfrOperator::Lte,
            200.0,
        ));
        let out = evaluate(&exp, &m).expect("targets declared");
        assert_eq!(out.rto_target_s, Some(900.0));
        assert_eq!(out.rto_measured_s, None);
        assert_eq!(out.verdicts.len(), 3);
        assert!(out
            .verdicts
            .iter()
            .all(|v| v.state == NfrState::Inconclusive));
        assert!(!out.has_blocking_breach());

        let post = PostResult {
            started_at_ns: 0,
            ended_at_ns: 0,
            duration_s: 0.0,
            samples: 1,
            probes: vec![],
            recovery_time_s: 1000.0,
            full_recovery: true,
            residual_degradation: None,
            data_integrity_verified: None,
            data_loss_detected: None,
            mttr_s: Some(1000.0),
        };
        let l = load();
        let m = Measurements {
            during: None,
            post: Some(&post),
            load: Some(&l),
            samples: &[],
        };
        let out = evaluate(&exp, &m).unwrap();
        assert_eq!(out.rto_measured_s, Some(1000.0));
        let rto = out.verdicts.iter().find(|v| v.nfr_id == "rto").unwrap();
        assert_eq!(rto.state, NfrState::Breached);
        assert_eq!(rto.criticality, NfrCriticality::Blocking);
        assert!(out.has_blocking_breach());
        let p95 = out.verdicts.iter().find(|v| v.nfr_id == "p95").unwrap();
        assert_eq!(p95.state, NfrState::Met);
    }
}
