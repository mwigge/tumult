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
//! an objective is never silently met. A metric outside this catalogue can
//! never be produced, so validation refuses it ([`metric_fault`]).

use crate::types::{
    DuringResult, Experiment, LoadResult, NfrCriticality, NfrKind, NfrObjective, NfrOperator,
    NfrState, NfrVerdict, ObjectiveOutcome, PostResult, ProbeSample, SamplePhase,
};

/// The metrics with one fixed spelling. [`resolve`] answers each of them.
pub(crate) const FIXED_METRICS: &[&str] = &[
    "load.throughput_rps",
    "load.latency_p50_ms",
    "load.latency_p95_ms",
    "load.latency_p99_ms",
    "load.error_rate",
    "load.total_requests",
    "recovery.mttr_s",
    "recovery.time_s",
    "during.error_rate",
    "during.availability",
    "post.error_rate",
    "post.availability",
    "fault.duration_s",
];

/// The statistics of a `probe.<name>.<stat>` metric.
pub(crate) const PROBE_STATS: &[&str] = &["mean_ms", "max_ms", "p95_ms", "error_rate"];

/// Why no run can measure `metric`, or `None` when the catalogue holds it.
/// `probes` are the probes of the steady-state hypothesis: the only ones a
/// run samples. Pure.
pub(crate) fn metric_fault(metric: &str, probes: &[&str]) -> Option<String> {
    if FIXED_METRICS.contains(&metric) {
        return None;
    }
    let probe_stat = metric
        .strip_prefix("probe.")
        .and_then(|rest| rest.rsplit_once('.'))
        .filter(|(_, stat)| PROBE_STATS.contains(stat));
    let Some((name, _)) = probe_stat else {
        return Some(format!(
            "metric '{metric}' is not in the catalogue (known: {}, probe.<name>.<{}>)",
            FIXED_METRICS.join(", "),
            PROBE_STATS.join("|")
        ));
    };
    if probes.contains(&name) {
        return None;
    }
    let sampled = if probes.is_empty() {
        "none".to_string()
    } else {
        probes.join(", ")
    };
    Some(format!(
        "metric '{metric}' names the probe '{name}', which the steady-state hypothesis does not have (its probes: {sampled})"
    ))
}

/// Measurements an objective can be evaluated against.
pub(crate) struct Measurements<'a> {
    pub during: Option<&'a DuringResult>,
    pub post: Option<&'a PostResult>,
    pub load: Option<&'a LoadResult>,
    pub samples: &'a [ProbeSample],
    /// Whether the post phase watched the recovery from the fault as
    /// written: every action of the method succeeded and no cancel cut
    /// the watch short. Only such a watch shows that a run did not recover.
    pub watched_in_full: bool,
}

/// Evaluate every declared objective and the recovery targets. Returns
/// `None` when the definition declares neither objectives nor targets, so
/// journals of plain experiments stay unchanged.
#[allow(clippy::similar_names)] // locals mirror the rto_/rpo_ fields of ObjectiveOutcome
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
        verdicts.push(rto_verdict(&implicit, m));
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
                recovered: None,
                reason: None,
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

/// The verdict on the recovery time objective. A run whose probes saw no
/// outage while the post phase watched in full measured no recovery: the
/// verdict is inconclusive and says why. A run that did not recover
/// breaches it: the time from the outage seen to the end of the watch is
/// a lower bound of the recovery time, so the verdict has no margin (such
/// a run has no MTTR, so the measured verdict has none). Pure.
fn rto_verdict(objective: &NfrObjective, m: &Measurements<'_>) -> NfrVerdict {
    let measured = verdict_for(objective, m.post.and_then(|p| p.mttr_s));
    let Some(post) = m.post.filter(|_| m.watched_in_full) else {
        return measured;
    };
    if post.impact_observed == Some(false) {
        return NfrVerdict {
            reason: Some(NfrVerdict::NO_OUTAGE_OBSERVED.into()),
            ..measured
        };
    }
    if post.full_recovery {
        return NfrVerdict {
            recovered: Some(true),
            ..measured
        };
    }
    NfrVerdict {
        observed: Some(watched_since_outage_s(post)),
        state: NfrState::Breached,
        recovered: Some(false),
        ..measured
    }
}

/// Seconds from the outage seen to the end of the watch of a run that did
/// not recover: from the outage seen to the end of the method, and the
/// time the post phase watched after it. The first part is negative when
/// the post phase saw the outage first. Pure.
fn watched_since_outage_s(post: &PostResult) -> f64 {
    // Nanosecond delta to seconds; i64 → f64 precision loss acceptable.
    #[allow(clippy::cast_precision_loss)]
    let before_the_watch_s = post
        .outage_started_at_ns
        .map_or(0.0, |at| (post.started_at_ns - at) as f64 / 1_000_000_000.0);
    before_the_watch_s + post.recovery_time_s
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
        recovered: None,
        reason: None,
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

    /// A run that measured everything the catalogue names.
    fn full_post() -> PostResult {
        PostResult {
            started_at_ns: 0,
            ended_at_ns: 5_000_000_000,
            duration_s: 5.0,
            samples: 1,
            probes: vec![],
            recovery_time_s: 4.0,
            full_recovery: true,
            mttr_s: Some(3.5),
            impact_observed: None,
            outage_started_at_ns: None,
            recovered_at_ns: None,
        }
    }

    fn full_during() -> DuringResult {
        DuringResult {
            started_at_ns: 0,
            ended_at_ns: 2_000_000_000,
            fault_active_s: 2.0,
            sample_interval_s: 1.0,
            probes: vec![],
        }
    }

    #[test]
    fn the_resolver_knows_every_metric_the_catalogue_accepts() {
        let (l, post, during) = (load(), full_post(), full_during());
        let samples = vec![
            sample("health", SamplePhase::During, 1, 100, true),
            sample("health", SamplePhase::Post, 2, 50, true),
        ];
        let m = Measurements {
            during: Some(&during),
            post: Some(&post),
            load: Some(&l),
            samples: &samples,
            watched_in_full: true,
        };
        let probe_metrics = PROBE_STATS
            .iter()
            .map(|stat| format!("probe.health.{stat}"));
        let fixed = FIXED_METRICS.iter().map(ToString::to_string);
        for metric in fixed.chain(probe_metrics) {
            assert_eq!(metric_fault(&metric, &["health"]), None, "{metric}");
            assert!(resolve(&metric, &m).is_some(), "{metric} is not resolved");
        }
    }

    #[test]
    fn a_metric_the_resolver_does_not_know_is_a_fault() {
        for metric in ["load.latency_p90_ms", "probe.health.p99_ms", "uptime"] {
            let fault = metric_fault(metric, &["health"]).expect("a fault");
            assert!(fault.contains(metric), "{fault}");
        }
        let fault = metric_fault("probe.other.p95_ms", &["health"]).expect("a fault");
        assert!(fault.contains("'other'"), "{fault}");
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
            mttr_s: Some(3.5),
            impact_observed: None,
            outage_started_at_ns: None,
            recovered_at_ns: None,
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
            watched_in_full: true,
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
            watched_in_full: true,
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
            watched_in_full: true,
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
            watched_in_full: true,
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
            mttr_s: Some(1000.0),
            impact_observed: None,
            outage_started_at_ns: None,
            recovered_at_ns: None,
        };
        let l = load();
        let m = Measurements {
            during: None,
            post: Some(&post),
            load: Some(&l),
            samples: &[],
            watched_in_full: true,
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

    /// The post phase of a restore that never came back: the probe still
    /// failed when the window closed, 29.1 s after the method.
    fn never_recovered() -> PostResult {
        PostResult {
            started_at_ns: 0,
            ended_at_ns: 30_000_000_000,
            duration_s: 30.0,
            samples: 30,
            probes: vec![],
            recovery_time_s: 29.1,
            full_recovery: false,
            mttr_s: None,
            impact_observed: None,
            outage_started_at_ns: None,
            recovered_at_ns: None,
        }
    }

    fn with_rto(target_s: f64) -> Experiment {
        Experiment {
            rto_target_s: Some(target_s),
            ..Experiment::default()
        }
    }

    fn rto(outcome: &ObjectiveOutcome) -> &NfrVerdict {
        outcome
            .verdicts
            .iter()
            .find(|v| v.nfr_id == "rto")
            .expect("a verdict on the rto")
    }

    #[test]
    fn a_run_that_never_recovered_breaches_its_rto_with_the_time_it_watched() {
        let post = never_recovered();
        let m = Measurements {
            during: None,
            post: Some(&post),
            load: None,
            samples: &[],
            watched_in_full: true,
        };

        let out = evaluate(&with_rto(2.0), &m).expect("an rto target");

        let verdict = rto(&out);
        assert_eq!(verdict.state, NfrState::Breached);
        assert_eq!(verdict.observed, Some(29.1));
        assert_eq!(verdict.margin, None);
        assert_eq!(verdict.recovered, Some(false));
        assert!(out.has_blocking_breach());
        assert_eq!(out.rto_measured_s, None);
    }

    #[test]
    fn a_run_that_never_recovered_breaches_an_rto_longer_than_its_watch() {
        let post = never_recovered();
        let m = Measurements {
            during: None,
            post: Some(&post),
            load: None,
            samples: &[],
            watched_in_full: true,
        };

        let out = evaluate(&with_rto(60.0), &m).expect("an rto target");

        let verdict = rto(&out);
        assert_eq!(verdict.state, NfrState::Breached);
        assert_eq!(verdict.observed, Some(29.1));
        assert_eq!(verdict.margin, None);
        assert_eq!(verdict.recovered, Some(false));
    }

    #[test]
    fn a_watch_that_does_not_count_measures_no_recovery() {
        let post = never_recovered();
        let m = Measurements {
            during: None,
            post: Some(&post),
            load: None,
            samples: &[],
            watched_in_full: false,
        };

        let out = evaluate(&with_rto(2.0), &m).expect("an rto target");

        let verdict = rto(&out);
        assert_eq!(verdict.state, NfrState::Inconclusive);
        assert_eq!(verdict.observed, None);
        assert_eq!(verdict.recovered, None);
        assert!(!out.has_blocking_breach());
    }

    #[test]
    fn a_run_that_recovered_late_breaches_its_rto_by_the_time_it_took() {
        let post = full_post();
        let m = Measurements {
            during: None,
            post: Some(&post),
            load: None,
            samples: &[],
            watched_in_full: true,
        };

        let out = evaluate(&with_rto(2.0), &m).expect("an rto target");

        let verdict = rto(&out);
        assert_eq!(verdict.state, NfrState::Breached);
        assert_eq!(verdict.observed, Some(3.5));
        assert_eq!(verdict.margin, Some(-1.5));
        assert_eq!(verdict.recovered, Some(true));
        assert_eq!(out.rto_measured_s, Some(3.5));
    }

    /// The post phase of a run whose samples never failed: the target
    /// passed every read, so nothing recovered.
    fn no_outage() -> PostResult {
        PostResult {
            recovery_time_s: 0.0,
            mttr_s: None,
            impact_observed: Some(false),
            ..full_post()
        }
    }

    fn watching(post: &PostResult, watched_in_full: bool) -> Measurements<'_> {
        Measurements {
            during: None,
            post: Some(post),
            load: None,
            samples: &[],
            watched_in_full,
        }
    }

    #[test]
    fn a_run_that_saw_no_outage_has_an_inconclusive_rto_that_says_so() {
        let post = no_outage();

        let out = evaluate(&with_rto(2.0), &watching(&post, true)).expect("an rto target");

        let verdict = rto(&out);
        assert_eq!(verdict.state, NfrState::Inconclusive);
        assert_eq!(
            verdict.reason.as_deref(),
            Some(NfrVerdict::NO_OUTAGE_OBSERVED)
        );
        assert_eq!(verdict.observed, None);
        assert_eq!(verdict.margin, None);
        assert_eq!(verdict.recovered, None);
        assert_eq!(out.rto_measured_s, None);
        assert!(!out.has_blocking_breach());
    }

    #[test]
    fn a_watch_that_does_not_count_gives_its_rto_no_reason() {
        let post = no_outage();

        let out = evaluate(&with_rto(2.0), &watching(&post, false)).expect("an rto target");

        let verdict = rto(&out);
        assert_eq!(verdict.state, NfrState::Inconclusive);
        assert_eq!(verdict.reason, None);
    }

    #[test]
    fn a_run_that_recovered_has_an_rto_without_a_reason() {
        let post = PostResult {
            impact_observed: Some(true),
            ..full_post()
        };

        let out = evaluate(&with_rto(2.0), &watching(&post, true)).expect("an rto target");

        assert_eq!(rto(&out).reason, None);
        assert_eq!(rto(&out).recovered, Some(true));
    }

    #[test]
    fn a_run_that_never_recovered_breaches_its_rto_from_the_outage_seen() {
        // The probes saw the outage 5 s before the method ended, and it
        // still failed 29.1 s after.
        let post = PostResult {
            impact_observed: Some(true),
            outage_started_at_ns: Some(-5_000_000_000),
            ..never_recovered()
        };

        let out = evaluate(&with_rto(2.0), &watching(&post, true)).expect("an rto target");

        let verdict = rto(&out);
        assert_eq!(verdict.state, NfrState::Breached);
        let observed = verdict.observed.expect("a lower bound");
        assert!((observed - 34.1).abs() < 1e-9, "{observed}");
        assert_eq!(verdict.margin, None);
        assert_eq!(verdict.recovered, Some(false));
        assert_eq!(verdict.reason, None);
        assert_eq!(out.rto_measured_s, None);
    }

    /// A file of the examples folder of the repository.
    fn example(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples")
            .join(name)
    }

    /// The journal of `postgres-failover` that 3.0.3 recorded: its fault
    /// killed no connection, and it said the RTO was met in 0 s.
    #[test]
    fn the_recorded_postgres_failover_run_saw_no_outage_when_read_again() {
        let journal =
            crate::journal::read_journal(&example("recorded/postgres-failover.journal.toon"))
                .expect("the recorded journal");
        let definition =
            std::fs::read_to_string(example("postgres-failover.toon")).expect("the experiment");
        let experiment = crate::engine::parse_experiment(&definition).expect("it parses");
        let stored = journal.post_result.clone().expect("the run watched");
        assert_eq!(stored.mttr_s, Some(0.0));
        assert_eq!(stored.impact_observed, None);

        let post = super::super::phases::timed_from_outage(stored, &journal.probe_samples);
        let m = Measurements {
            during: journal.during_result.as_ref(),
            post: Some(&post),
            load: None,
            samples: &journal.probe_samples,
            watched_in_full: true,
        };
        let out = evaluate(&experiment, &m).expect("an rto target");

        assert_eq!(post.impact_observed, Some(false));
        assert_eq!(post.mttr_s, None);
        let verdict = rto(&out);
        assert_eq!(verdict.state, NfrState::Inconclusive);
        assert_eq!(
            verdict.reason.as_deref(),
            Some(NfrVerdict::NO_OUTAGE_OBSERVED)
        );
        assert_eq!(out.rto_measured_s, None);
        let availability = out
            .verdicts
            .iter()
            .find(|v| v.nfr_id == "post-availability")
            .expect("the declared objective");
        assert_eq!(availability.state, NfrState::Met);
    }
}
