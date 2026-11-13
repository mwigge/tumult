//! The answer of the verdict routes, and its JSON.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use tumult_lake::PerfVerdictRow;
use tumult_perf::{percentile_sorted, Comparison, SampleSummary};

use super::select::{Attributed, Attribution, Env, Reason, Side, VersionSeen};

/// Unattributed runs listed one by one; the count covers all of them.
const UNATTRIBUTED_LISTED: usize = 50;

/// Everything both routes compute before answering.
pub(crate) struct Answer {
    pub target: String,
    pub probe: String,
    /// The one environment the answer is for.
    pub environment: Env,
    /// Every environment in which the principal could see versions.
    pub environments_seen: Vec<Env>,
    pub range: String,
    pub since_ns: i64,
    pub as_of_ns: i64,
    /// Whether the request named a version.
    pub explicit: bool,
    pub include_partial: bool,
    pub min_samples: usize,
    /// The environment's versions in deployment order, latest first.
    pub versions: Vec<VersionSeen>,
    pub candidate: Side,
    pub baseline: Option<Side>,
    pub comparison: Comparison,
    /// Runs inside the request's scope that no version could be given.
    pub unattributed: Vec<(String, Env, Reason)>,
    /// Runs of the environment that measured neither side's version.
    pub runs_on_other_versions: usize,
    pub history: Vec<Value>,
}

/// The runs of `runs` without a version, for the environments `in_scope`
/// lets through.
pub(crate) fn unattributed(
    runs: &[Attributed],
    in_scope: impl Fn(&Env) -> bool,
) -> Vec<(String, Env, Reason)> {
    runs.iter()
        .filter(|a| in_scope(&a.run.environment))
        .filter_map(|a| {
            let reason = a.owner.as_ref().err()?;
            Some((
                a.run.experiment_id.clone(),
                a.run.environment.clone(),
                *reason,
            ))
        })
        .collect()
}

/// The percentiles 0, 5, … 100 of the finite samples: enough to draw the
/// distribution without shipping every sample.
fn distribution(samples: &[f64]) -> Vec<Value> {
    let mut sorted: Vec<f64> = samples.iter().copied().filter(|v| v.is_finite()).collect();
    sorted.sort_by(f64::total_cmp);
    (0..=20u8)
        .map(|step| step * 5)
        .filter_map(|p| {
            let value = percentile_sorted(&sorted, f64::from(p))?;
            Some(json!({"p": p, "value": value}))
        })
        .collect()
}

fn side_json(side: &Side, summary: &SampleSummary) -> Value {
    let details: Vec<Value> = side
        .details
        .iter()
        .map(|d| {
            json!({
                "experiment_id": d.experiment_id,
                "attribution": d.attribution.as_str(),
                "status": d.status,
                "truncated": d.truncated,
                "aborted": d.aborted,
                "included": d.included,
            })
        })
        .collect();
    json!({
        "version": side.version,
        "runs": side.runs(),
        "samples": summary.n,
        "dropped": summary.dropped,
        "p50": summary.p50,
        "p95": summary.p95,
        "p99": summary.p99,
        "attribution": {
            "declared": side.attributed(Attribution::Declared),
            "observed": side.attributed(Attribution::Observed),
            "inferred": side.attributed(Attribution::Inferred),
        },
        "run_details": details,
        "partial": {
            "truncated": side.partial.truncated,
            "aborted": side.partial.aborted,
            "halted": side.partial.halted,
            "deviated": side.partial.deviated,
            "excluded": side.partial.excluded,
        },
        "windows": {
            "planned": side.windows.planned,
            "completed": side.windows.completed,
            "unanswered": side.windows.unanswered,
            "unanswered_share": side.windows.unanswered_share(),
        },
        "distribution": distribution(&side.samples),
    })
}

fn unattributed_json(runs: &[(String, Env, Reason)]) -> Value {
    let mut reasons: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, _, reason) in runs {
        *reasons.entry(reason.as_str()).or_default() += 1;
    }
    let listed: Vec<Value> = runs
        .iter()
        .take(UNATTRIBUTED_LISTED)
        .map(|(id, env, reason)| {
            json!({"experiment_id": id, "environment": env, "reason": reason.as_str()})
        })
        .collect();
    json!({"count": runs.len(), "reasons": reasons, "runs": listed})
}

impl Answer {
    pub(crate) fn json(&self, stored: bool, duplicate: bool) -> Value {
        let c = &self.comparison;
        let baseline = self
            .baseline
            .as_ref()
            .map_or(Value::Null, |b| side_json(b, &c.baseline));
        let selection = if self.explicit {
            "explicit"
        } else {
            "deployment_order"
        };
        json!({
            "target": self.target,
            "probe": self.probe,
            "environment": self.environment,
            "environments_seen": self.environments_seen,
            "range": self.range,
            "since_ns": self.since_ns,
            "selection": selection,
            "include_partial": self.include_partial,
            "verdict": c.verdict,
            "reasons": c.reasons,
            "baseline": baseline,
            "candidate": side_json(&self.candidate, &c.candidate),
            "comparison": {
                "quantile": c.quantile,
                "delta": c.delta,
                "delta_pct": c.delta_pct,
                "ci_low": c.interval.map(|i| i.low),
                "ci_high": c.interval.map(|i| i.high),
                "confidence": c.confidence,
                "margin": c.margin,
                "iterations": c.iterations,
                "seed": c.seed,
                "min_samples": self.min_samples,
            },
            "versions_seen": self.versions.iter().map(|v| json!({
                "version": v.version,
                "first_seen_ns": v.first_seen_ns,
                "last_seen_ns": v.last_seen_ns,
            })).collect::<Vec<_>>(),
            "unattributed_runs": unattributed_json(&self.unattributed),
            "runs_on_other_versions": self.runs_on_other_versions,
            "stored": stored,
            "duplicate": duplicate,
            "history": self.history,
            "as_of_ns": self.as_of_ns,
        })
    }

    /// The row a stored verdict is, for the baseline it was computed
    /// against. The writer stamps the time.
    pub(crate) fn row(&self, baseline: &Side) -> PerfVerdictRow {
        let c = &self.comparison;
        let n = |n: usize| i64::try_from(n).unwrap_or(i64::MAX);
        PerfVerdictRow {
            target: self.target.clone(),
            probe: self.probe.clone(),
            baseline_version: baseline.version.clone(),
            candidate_version: self.candidate.version.clone(),
            quantile: c.quantile.to_string(),
            baseline_n: n(c.baseline.n),
            candidate_n: n(c.candidate.n),
            baseline_p50: c.baseline.p50,
            baseline_p95: c.baseline.p95,
            baseline_p99: c.baseline.p99,
            candidate_p50: c.candidate.p50,
            candidate_p95: c.candidate.p95,
            candidate_p99: c.candidate.p99,
            delta: c.delta,
            delta_pct: c.delta_pct,
            ci_low: c.interval.map(|i| i.low),
            ci_high: c.interval.map(|i| i.high),
            confidence: c.confidence,
            margin: c.margin,
            verdict: c.verdict.to_string(),
            reasons: c.reasons.clone(),
            computed_at_ns: self.as_of_ns,
            envs: self.environment.clone().map(|env| vec![env]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_distribution_is_twenty_one_ordered_points_of_the_finite_samples() {
        assert!(distribution(&[]).is_empty());
        assert!(distribution(&[f64::NAN, f64::INFINITY]).is_empty());
        let samples: Vec<f64> = (1..=200).rev().map(f64::from).collect();
        let mut with_noise = samples.clone();
        with_noise.push(f64::NAN);
        let points = distribution(&with_noise);
        assert_eq!(points.len(), 21);
        assert_eq!(points[0], json!({"p": 0, "value": 1.0}));
        assert_eq!(points[10], json!({"p": 50, "value": 100.0}));
        assert_eq!(points[20], json!({"p": 100, "value": 200.0}));
        let values: Vec<f64> = points.iter().filter_map(|p| p["value"].as_f64()).collect();
        assert!(
            values.windows(2).all(|pair| pair[0] <= pair[1]),
            "{values:?}"
        );
    }

    #[test]
    fn unattributed_runs_are_counted_by_reason_and_listed_up_to_a_bound() {
        let runs: Vec<(String, Env, Reason)> = (0..60)
            .map(|i| {
                let reason = if i % 3 == 0 {
                    Reason::NoSpansNearRun
                } else {
                    Reason::NoTelemetryInEnvironment
                };
                (
                    format!("run-{i}"),
                    (i % 2 == 0).then(|| "demo".to_string()),
                    reason,
                )
            })
            .collect();
        let body = unattributed_json(&runs);
        assert_eq!(body["count"], 60);
        assert_eq!(
            body["reasons"],
            json!({"no_target_spans_near_run": 20, "no_target_telemetry_in_environment": 40})
        );
        assert_eq!(body["runs"].as_array().map(Vec::len), Some(50));
        assert_eq!(
            body["runs"][1],
            json!({
                "experiment_id": "run-1", "environment": null,
                "reason": "no_target_telemetry_in_environment",
            })
        );
        assert_eq!(unattributed_json(&[])["reasons"], json!({}));
    }
}
