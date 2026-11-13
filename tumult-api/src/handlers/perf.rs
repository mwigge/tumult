//! `GET /api/perf/{target}` — the performance regression verdict.
//!
//! Compares the latency samples the target produced under its latest two
//! `service.version`s. A target's versions are the `service.version`
//! values on the telemetry whose `service.name` is the target (the
//! service's own spans); each experiment run against the target (root
//! spans with `resilience.target.system`) is attributed to the most recent
//! version first seen before the run started. The samples are the run's
//! `during`-phase `probe_samples` rows for one series (default
//! `load.latency_p95_ms`, the per-window p95 a streaming load adapter
//! writes). The verdict is `tumult_perf::compare`'s. The request is a pure
//! read unless `store=true`, which appends the verdict to `perf_verdicts`
//! and needs the operator role; stored rows carry the environment list
//! they were computed under, and a scoped principal's versions, runs and
//! history are all restricted to its environments.

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, State};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use tumult_ingest::Batch;
use tumult_lake::{PerfVerdictRow, Reader, Writer};
use tumult_perf::{compare, CompareOptions, Comparison, Quantile, Verdict};

use crate::auth::Principal;
use crate::error::{bad_request, forbidden, not_found, ApiError};
use crate::sql_util::{and_pred, env_scope_where, internal, now_ns, sql_string, with_reader};
use crate::ApiState;
use tumult_auth::Role;

/// The series compared when `probe` is not given.
pub(crate) const DEFAULT_PROBE: &str = "load.latency_p95_ms";
/// Stored verdicts returned as history.
const HISTORY_LIMIT: usize = 10;

#[derive(Deserialize)]
pub(crate) struct PerfQuery {
    probe: Option<String>,
    quantile: Option<String>,
    confidence: Option<f64>,
    min_samples: Option<usize>,
    margin_pct: Option<f64>,
    env: Option<String>,
    /// `true` appends the verdict to `perf_verdicts` (operator and above).
    /// The default is a pure read.
    store: Option<bool>,
}

/// Validated comparison settings from the query string.
pub(crate) fn options_from_query(q: &PerfQuery) -> Result<CompareOptions, ApiError> {
    let defaults = CompareOptions::default();
    let quantile = match q.quantile.as_deref() {
        None => defaults.quantile,
        Some("p50") => Quantile::P50,
        Some("p95") => Quantile::P95,
        Some("p99") => Quantile::P99,
        Some(_) => return Err(bad_request("quantile must be p50, p95 or p99")),
    };
    let confidence = q.confidence.unwrap_or(defaults.confidence);
    if !(confidence > 0.0 && confidence < 1.0) {
        return Err(bad_request(
            "confidence must be between 0 and 1 (exclusive)",
        ));
    }
    let noise_margin_pct = q.margin_pct.unwrap_or(defaults.noise_margin_pct);
    if !(noise_margin_pct.is_finite() && noise_margin_pct >= 0.0) {
        return Err(bad_request("margin_pct must be a non-negative number"));
    }
    let min_samples = q.min_samples.unwrap_or(defaults.min_samples);
    if min_samples < 2 {
        return Err(bad_request("min_samples must be at least 2"));
    }
    Ok(CompareOptions {
        quantile,
        confidence,
        min_samples,
        noise_margin_pct,
        ..defaults
    })
}

/// A `service.version` and when the target's telemetry first and last
/// carried it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VersionSeen {
    pub version: String,
    pub first_seen_ns: i64,
    pub last_seen_ns: i64,
}

/// A run against the target: its id and start time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunSeen {
    pub experiment_id: String,
    pub started_at_ns: i64,
}

/// What the lake knows about the target, before attribution.
#[derive(Debug, Default)]
pub(crate) struct Evidence {
    pub versions: Vec<VersionSeen>,
    pub runs: Vec<RunSeen>,
    /// `(experiment_id, value)` for the requested probe series.
    pub samples: Vec<(String, f64)>,
    pub history: Vec<Value>,
}

/// One side of the comparison after attribution.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Side {
    pub version: String,
    pub runs: Vec<String>,
    pub samples: Vec<f64>,
}

/// The version a run started under. A version whose `[first_seen,
/// last_seen]` window contains the start wins (the latest-deployed one
/// when several overlap); a run in a telemetry gap falls back to the most
/// recent version first seen before it. `by_first` is sorted by
/// `first_seen_ns`. `None` for a run older than every version.
fn owner_of<'a>(by_first: &[&'a VersionSeen], started_at_ns: i64) -> Option<&'a VersionSeen> {
    let mut seen_before = by_first
        .iter()
        .rev()
        .filter(|v| v.first_seen_ns <= started_at_ns);
    let fallback = seen_before.clone().next();
    seen_before
        .find(|v| started_at_ns <= v.last_seen_ns)
        .or(fallback)
        .copied()
}

/// Attribute every run to the version live when it started (see
/// [`owner_of`]); runs older than every version are unattributed. Returns
/// `(candidate, baseline)`: the two versions most recently seen, newest
/// first. `None` when the target has no versions at all.
pub(crate) fn split(evidence: &Evidence) -> Option<(Side, Option<Side>)> {
    let mut by_first: Vec<&VersionSeen> = evidence.versions.iter().collect();
    by_first.sort_by_key(|v| v.first_seen_ns);
    let mut by_last: Vec<&VersionSeen> = evidence.versions.iter().collect();
    by_last.sort_by_key(|v| std::cmp::Reverse(v.last_seen_ns));
    let candidate = by_last.first()?;
    let baseline = by_last.get(1);

    let mut runs_of: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for run in &evidence.runs {
        if let Some(v) = owner_of(&by_first, run.started_at_ns) {
            runs_of
                .entry(v.version.as_str())
                .or_default()
                .push(run.experiment_id.clone());
        }
    }
    let side = |v: &VersionSeen| {
        let runs = runs_of.get(v.version.as_str()).cloned().unwrap_or_default();
        let ids: HashSet<&str> = runs.iter().map(String::as_str).collect();
        let samples = evidence
            .samples
            .iter()
            .filter(|(id, _)| ids.contains(id.as_str()))
            .map(|(_, value)| *value)
            .collect();
        Side {
            version: v.version.clone(),
            runs,
            samples,
        }
    };
    Some((side(candidate), baseline.map(|b| side(b))))
}

/// Read the target's versions, runs, samples and stored verdicts.
fn load_evidence(
    reader: &Reader,
    target: &str,
    probe: &str,
    envs: &[String],
) -> Result<Evidence, String> {
    let t = sql_string(target);
    let scope = and_pred(env_scope_where("target_environment", envs));
    let versions = reader
        .query_json_rows(&format!(
            "SELECT service_version AS v, min(ts_ns) AS first_seen, max(ts_ns) AS last_seen \
             FROM spans WHERE service_name = {t} AND service_version IS NOT NULL{scope} \
             GROUP BY service_version ORDER BY last_seen DESC"
        ))
        .map_err(|e| e.to_string())?
        .iter()
        .filter_map(|r| {
            Some(VersionSeen {
                version: r.get("v")?.as_str()?.to_string(),
                first_seen_ns: r.get("first_seen")?.as_i64()?,
                last_seen_ns: r.get("last_seen")?.as_i64()?,
            })
        })
        .collect();
    let runs: Vec<RunSeen> = reader
        .query_json_rows(&format!(
            "SELECT experiment_id AS id, min(ts_ns) AS started \
             FROM spans WHERE span_name = 'resilience.experiment' AND target_system = {t} \
             AND experiment_id IS NOT NULL{scope} GROUP BY experiment_id ORDER BY started"
        ))
        .map_err(|e| e.to_string())?
        .iter()
        .filter_map(|r| {
            Some(RunSeen {
                experiment_id: r.get("id")?.as_str()?.to_string(),
                started_at_ns: r.get("started")?.as_i64()?,
            })
        })
        .collect();
    let samples = if runs.is_empty() {
        Vec::new()
    } else {
        let ids = runs
            .iter()
            .map(|r| sql_string(&r.experiment_id))
            .collect::<Vec<_>>()
            .join(", ");
        reader
            .query_json_rows(&format!(
                "SELECT experiment_id AS id, value FROM probe_samples \
                 WHERE probe_name = {} AND phase = 'during' AND experiment_id IN ({ids}) \
                 ORDER BY ts_ns",
                sql_string(probe)
            ))
            .map_err(|e| e.to_string())?
            .iter()
            .filter_map(|r| {
                Some((
                    r.get("id")?.as_str()?.to_string(),
                    r.get("value")?.as_f64()?,
                ))
            })
            .collect()
    };
    let history = reader
        .perf_verdicts(target, envs, HISTORY_LIMIT)
        .map_err(|e| e.to_string())?;
    Ok(Evidence {
        versions,
        runs,
        samples,
        history,
    })
}

/// The stored row for a comparison.
fn verdict_row(
    target: &str,
    probe: &str,
    baseline: &str,
    candidate: &str,
    c: &Comparison,
    computed_at_ns: i64,
    envs: &[String],
) -> PerfVerdictRow {
    let n = |n: usize| i64::try_from(n).unwrap_or(i64::MAX);
    PerfVerdictRow {
        target: target.into(),
        probe: probe.into(),
        baseline_version: baseline.into(),
        candidate_version: candidate.into(),
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
        computed_at_ns,
        envs: (!envs.is_empty()).then(|| envs.to_vec()),
    }
}

/// Append the verdict through the single-writer channel. `Ok(false)` when
/// the API has no ingest handle (read-only deployment).
async fn store_verdict(state: &ApiState, row: PerfVerdictRow) -> Result<bool, ApiError> {
    let Some(ingest) = state.ingest_handle() else {
        return Ok(false);
    };
    let slot = Arc::new(Mutex::new(None));
    let slot2 = Arc::clone(&slot);
    ingest
        .write(Batch::Exec(Box::new(move |writer: &Writer| {
            *slot2.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(writer.insert_perf_verdict(&row));
            Ok(())
        })))
        .await
        .map_err(|e| internal(e.to_string()))?;
    let result = slot.lock().unwrap_or_else(|e| e.into_inner()).take();
    match result {
        Some(Ok(())) => Ok(true),
        Some(Err(e)) => Err(internal(e.to_string())),
        None => Err(internal("perf verdict was not stored".into())),
    }
}

fn side_json(side: &Side, summary: &tumult_perf::SampleSummary) -> Value {
    json!({
        "version": side.version,
        "runs": side.runs,
        "samples": summary.n,
        "dropped": summary.dropped,
        "p50": summary.p50,
        "p95": summary.p95,
        "p99": summary.p99,
    })
}

/// `GET /api/perf/{target}?probe=&quantile=&confidence=&min_samples=&margin_pct=&env=&store=`.
pub(crate) async fn perf(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(target): Path<String>,
    Query(q): Query<PerfQuery>,
) -> Result<Json<Value>, ApiError> {
    let opts = options_from_query(&q)?;
    let probe = q
        .probe
        .clone()
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_PROBE.to_string());
    let envs: Vec<String> = match q.env {
        Some(env) => {
            if !principal.env_allowed(&env) {
                return Err(forbidden(format!(
                    "environment {env} is outside the principal's scopes"
                )));
            }
            vec![env]
        }
        None => principal.env_scopes.clone(),
    };
    let store = q.store.unwrap_or(false);
    if store && principal.role < Role::Operator {
        return Err(forbidden(
            "storing a perf verdict needs the operator role".to_string(),
        ));
    }
    let as_of_ns = now_ns();
    let evidence = {
        let (target, probe, envs) = (target.clone(), probe.clone(), envs.clone());
        with_reader(&state.db_path, move |reader| {
            load_evidence(reader, &target, &probe, &envs)
        })
        .await?
    };
    let Some((candidate, baseline)) = split(&evidence) else {
        return Err(not_found(format!(
            "no service.version seen for target {target}"
        )));
    };
    let (comparison, baseline_json, stored) = match &baseline {
        Some(baseline) => {
            let c = compare(&baseline.samples, &candidate.samples, &opts);
            let stored = if store {
                let row = verdict_row(
                    &target,
                    &probe,
                    &baseline.version,
                    &candidate.version,
                    &c,
                    as_of_ns,
                    &envs,
                );
                store_verdict(&state, row).await?
            } else {
                false
            };
            let baseline_json = side_json(baseline, &c.baseline);
            (c, baseline_json, stored)
        }
        None => {
            let mut c = compare(&[], &candidate.samples, &opts);
            c.verdict = Verdict::Inconclusive;
            c.reasons.insert(
                0,
                format!(
                    "only one service.version ({}) seen for {target}; nothing to compare against",
                    candidate.version
                ),
            );
            (c, Value::Null, false)
        }
    };
    let body = json!({
        "target": target,
        "probe": probe,
        "verdict": comparison.verdict,
        "reasons": comparison.reasons,
        "baseline": baseline_json,
        "candidate": side_json(&candidate, &comparison.candidate),
        "comparison": {
            "quantile": comparison.quantile,
            "delta": comparison.delta,
            "delta_pct": comparison.delta_pct,
            "ci_low": comparison.interval.map(|i| i.low),
            "ci_high": comparison.interval.map(|i| i.high),
            "confidence": comparison.confidence,
            "margin": comparison.margin,
            "iterations": comparison.iterations,
            "seed": comparison.seed,
            "min_samples": opts.min_samples,
        },
        "versions_seen": evidence.versions.iter().map(|v| json!({
            "version": v.version,
            "first_seen_ns": v.first_seen_ns,
            "last_seen_ns": v.last_seen_ns,
        })).collect::<Vec<_>>(),
        "stored": stored,
        "history": evidence.history,
        "as_of_ns": as_of_ns,
    });
    Ok(Json(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(version: &str, first: i64, last: i64) -> VersionSeen {
        VersionSeen {
            version: version.into(),
            first_seen_ns: first,
            last_seen_ns: last,
        }
    }

    fn run(id: &str, started: i64) -> RunSeen {
        RunSeen {
            experiment_id: id.into(),
            started_at_ns: started,
        }
    }

    #[test]
    fn runs_are_attributed_to_the_version_live_when_they_started() {
        let evidence = Evidence {
            versions: vec![seen("1.4.0", 100, 400), seen("1.5.0", 500, 900)],
            runs: vec![
                run("old", 200),
                run("mid", 450),
                run("new", 600),
                run("ancient", 50),
            ],
            samples: vec![
                ("old".into(), 10.0),
                ("mid".into(), 11.0),
                ("new".into(), 20.0),
                ("ancient".into(), 99.0),
            ],
            history: vec![],
        };
        let (candidate, baseline) = split(&evidence).expect("versions");
        assert_eq!(candidate.version, "1.5.0");
        assert_eq!(candidate.runs, vec!["new"]);
        assert_eq!(candidate.samples, vec![20.0]);
        let baseline = baseline.expect("baseline");
        assert_eq!(baseline.version, "1.4.0");
        // `mid` started after 1.4.0 was first seen and before 1.5.0 was.
        assert_eq!(baseline.runs, vec!["old", "mid"]);
        assert_eq!(baseline.samples, vec![10.0, 11.0]);
    }

    #[test]
    fn single_version_has_no_baseline_and_none_is_none() {
        let evidence = Evidence {
            versions: vec![seen("1.0.0", 1, 2)],
            runs: vec![run("r", 5)],
            samples: vec![("r".into(), 1.0)],
            history: vec![],
        };
        let (candidate, baseline) = split(&evidence).expect("one version");
        assert_eq!(candidate.runs, vec!["r"]);
        assert!(baseline.is_none());
        assert!(split(&Evidence::default()).is_none());
    }

    #[test]
    fn candidate_is_the_most_recently_seen_version() {
        // 1.6.0 was seen first but 1.5.0 is still live later (a rollback).
        let evidence = Evidence {
            versions: vec![seen("1.6.0", 100, 300), seen("1.5.0", 50, 900)],
            runs: vec![
                run("both", 200),
                run("after-rollback", 600),
                run("late", 950),
            ],
            samples: vec![],
            history: vec![],
        };
        let (candidate, baseline) = split(&evidence).expect("versions");
        assert_eq!(candidate.version, "1.5.0");
        // Only 1.5.0 was live at 600: the rollback run is its evidence, not
        // 1.6.0's, although 1.6.0 was first seen later.
        assert_eq!(candidate.runs, vec!["after-rollback"]);
        let baseline = baseline.expect("baseline");
        assert_eq!(baseline.version, "1.6.0");
        // Both were live at 200 (the later-deployed one wins); nothing was
        // live at 950, which falls back to the latest first-seen version.
        assert_eq!(baseline.runs, vec!["both", "late"]);
    }

    #[test]
    fn query_options_are_validated() {
        let q = |quantile: Option<&str>,
                 confidence: Option<f64>,
                 margin: Option<f64>,
                 min: Option<usize>| {
            PerfQuery {
                probe: None,
                quantile: quantile.map(str::to_string),
                confidence,
                min_samples: min,
                margin_pct: margin,
                env: None,
                store: None,
            }
        };
        let opts = options_from_query(&q(None, None, None, None)).expect("defaults");
        assert_eq!(opts, CompareOptions::default());
        let opts =
            options_from_query(&q(Some("p99"), Some(0.9), Some(10.0), Some(5))).expect("custom");
        assert_eq!(opts.quantile, Quantile::P99);
        assert_eq!(opts.min_samples, 5);
        assert!(options_from_query(&q(Some("p42"), None, None, None)).is_err());
        assert!(options_from_query(&q(None, Some(1.0), None, None)).is_err());
        assert!(options_from_query(&q(None, None, Some(-1.0), None)).is_err());
        assert!(options_from_query(&q(None, None, None, Some(1))).is_err());
    }
}
