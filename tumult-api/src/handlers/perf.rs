//! `GET /api/perf/{target}` and `POST /api/perf/{target}/verdicts` — the
//! performance regression verdict.
//!
//! Compares the latency samples the target produced under its latest two
//! `service.version`s. A target's versions are the `service.version`
//! values on the telemetry whose `service.name` is the target (the
//! service's own spans); each experiment run against the target (root
//! spans with `resilience.target.system`) is attributed to the version the
//! target's last span before the run started carried — the version live
//! at that moment, which after a rollback is the older one again. The
//! samples are the run's `during`-phase `probe_samples` rows for one
//! series (default `load.latency_p95_ms`, the per-window p95 a streaming
//! load adapter writes). The verdict is `tumult_perf::compare`'s.
//!
//! `GET` is a pure read and never writes. `POST …/verdicts` computes the
//! same answer and appends it to `perf_verdicts`; the route table puts it
//! at the operator role. Stored rows carry the environment list they were
//! computed under.
//!
//! # Environments
//!
//! A span's environment is what ingest read from, in order,
//! `resilience.target.environment` on the span, then on the resource,
//! then the resource's `deployment.environment.name`, then its
//! `deployment.environment`. The value is matched exactly: `prod` and
//! `production` are different environments. With `env=` or for a scoped
//! principal only spans, runs and stored verdicts of those environments
//! exist; a service whose spans carry none of the attributes is outside
//! every environment, and the answer for it is a 404 that says so.

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
use crate::error::{bad_request, conflict, forbidden, not_found, unavailable, ApiError};
use crate::sql_util::{and_pred, env_scope_where, internal, now_ns, sql_string, with_reader};
use crate::ApiState;
use axum::http::StatusCode;

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

/// The moment the target's telemetry switched to a `service.version`: the
/// first span carrying it after spans that carried another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VersionChange {
    pub at_ns: i64,
    pub version: String,
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
    /// Every switch of version, oldest first. A rollback appears as a
    /// switch back to the older version.
    pub changes: Vec<VersionChange>,
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

/// The version live when a run started: the one the target's last span at
/// or before that moment carried. `changes` is sorted by `at_ns`. `None`
/// for a run older than all of the target's telemetry. While two versions
/// report side by side (a canary), the run goes to whichever spoke last.
fn owner_of(changes: &[VersionChange], started_at_ns: i64) -> Option<&str> {
    let later = changes.partition_point(|c| c.at_ns <= started_at_ns);
    let live = changes.get(later.checked_sub(1)?)?;
    Some(live.version.as_str())
}

/// Attribute every run to the version live when it started (see
/// [`owner_of`]); runs older than every version are unattributed. Returns
/// `(candidate, baseline)`: the two versions most recently seen, newest
/// first. `None` when the target has no versions at all.
pub(crate) fn split(evidence: &Evidence) -> Option<(Side, Option<Side>)> {
    let mut by_last: Vec<&VersionSeen> = evidence.versions.iter().collect();
    by_last.sort_by_key(|v| std::cmp::Reverse(v.last_seen_ns));
    let candidate = by_last.first()?;
    let baseline = by_last.get(1);

    let mut runs_of: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for run in &evidence.runs {
        if let Some(version) = owner_of(&evidence.changes, run.started_at_ns) {
            runs_of
                .entry(version)
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

/// Every switch of the target's `service.version`, oldest first.
fn load_changes(reader: &Reader, t: &str, scope: &str) -> Result<Vec<VersionChange>, String> {
    let rows = reader
        .query_json_rows(&format!(
            "SELECT seen.ts_ns AS at_ns, seen.v AS v FROM (              SELECT ts_ns, service_version AS v,              lag(service_version) OVER (ORDER BY ts_ns, span_id) AS prev              FROM spans WHERE service_name = {t} AND service_version IS NOT NULL{scope}              ) AS seen WHERE seen.prev IS NULL OR seen.prev <> seen.v ORDER BY seen.ts_ns"
        ))
        .map_err(|e| e.to_string())?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            Some(VersionChange {
                at_ns: r.get("at_ns")?.as_i64()?,
                version: r.get("v")?.as_str()?.to_string(),
            })
        })
        .collect())
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
    let changes = load_changes(reader, &t, &scope)?;
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
        changes,
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

/// The environments a request is answered for: the one it names, when the
/// principal may see it, or else the principal's own scopes (none for an
/// unscoped principal, which sees every environment).
fn envs_for(principal: &Principal, env: Option<&str>) -> Result<Vec<String>, ApiError> {
    let Some(env) = env else {
        return Ok(principal.env_scopes.clone());
    };
    if env.trim().is_empty() {
        return Err(bad_request(
            "env must name an environment; leave it out for all of them",
        ));
    }
    if !principal.env_allowed(env) {
        return Err(forbidden(format!(
            "environment {env} is outside the principal's scopes"
        )));
    }
    Ok(vec![env.to_string()])
}

/// Why the target has no versions. Under an environment filter the answer
/// names it and how a span gets an environment, without saying whether
/// the target exists elsewhere.
pub(crate) fn no_versions_message(target: &str, envs: &[String]) -> String {
    if envs.is_empty() {
        return format!(
            "no service.version seen for target {target}: no span with service.name {target}              carries one"
        );
    }
    let plural = if envs.len() == 1 { "" } else { "s" };
    format!(
        "no service.version seen for target {target} in environment{plural} {}: only spans          tagged with exactly that environment count (resilience.target.environment, or the          resource's deployment.environment.name or deployment.environment)",
        envs.join(", ")
    )
}

/// Everything both routes compute before answering.
struct Answer {
    target: String,
    probe: String,
    envs: Vec<String>,
    min_samples: usize,
    as_of_ns: i64,
    evidence: Evidence,
    candidate: Side,
    baseline: Option<Side>,
    comparison: Comparison,
}

/// With one version only there is nothing to compare against: the
/// candidate is summarised and the verdict is inconclusive.
fn lone_candidate(target: &str, candidate: &Side, opts: &CompareOptions) -> Comparison {
    let mut c = compare(&[], &candidate.samples, opts);
    c.verdict = Verdict::Inconclusive;
    c.reasons.insert(
        0,
        format!(
            "only one service.version ({}) seen for {target}; nothing to compare against",
            candidate.version
        ),
    );
    c
}

async fn answer(
    state: &ApiState,
    principal: &Principal,
    target: String,
    q: &PerfQuery,
) -> Result<Answer, ApiError> {
    let opts = options_from_query(q)?;
    let probe = q
        .probe
        .clone()
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_PROBE.to_string());
    let envs = envs_for(principal, q.env.as_deref())?;
    let as_of_ns = now_ns();
    let evidence = {
        let (target, probe, envs) = (target.clone(), probe.clone(), envs.clone());
        with_reader(&state.db_path, move |reader| {
            load_evidence(reader, &target, &probe, &envs)
        })
        .await?
    };
    let Some((candidate, baseline)) = split(&evidence) else {
        return Err(not_found(no_versions_message(&target, &envs)));
    };
    let comparison = match &baseline {
        Some(baseline) => compare(&baseline.samples, &candidate.samples, &opts),
        None => lone_candidate(&target, &candidate, &opts),
    };
    Ok(Answer {
        target,
        probe,
        envs,
        min_samples: opts.min_samples,
        as_of_ns,
        evidence,
        candidate,
        baseline,
        comparison,
    })
}

fn answer_json(a: &Answer, stored: bool) -> Value {
    let c = &a.comparison;
    let baseline = a
        .baseline
        .as_ref()
        .map_or(Value::Null, |b| side_json(b, &c.baseline));
    json!({
        "target": a.target,
        "probe": a.probe,
        "verdict": c.verdict,
        "reasons": c.reasons,
        "baseline": baseline,
        "candidate": side_json(&a.candidate, &c.candidate),
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
            "min_samples": a.min_samples,
        },
        "versions_seen": a.evidence.versions.iter().map(|v| json!({
            "version": v.version,
            "first_seen_ns": v.first_seen_ns,
            "last_seen_ns": v.last_seen_ns,
        })).collect::<Vec<_>>(),
        "stored": stored,
        "history": a.evidence.history,
        "as_of_ns": a.as_of_ns,
    })
}

/// `GET /api/perf/{target}?probe=&quantile=&confidence=&min_samples=&margin_pct=&env=`
/// — a pure read; `stored` is always `false`.
pub(crate) async fn perf(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(target): Path<String>,
    Query(q): Query<PerfQuery>,
) -> Result<Json<Value>, ApiError> {
    let a = answer(&state, &principal, target, &q).await?;
    Ok(Json(answer_json(&a, false)))
}

/// `POST /api/perf/{target}/verdicts` with the same query string: compute
/// the verdict and append it to `perf_verdicts`. 201 with the answer
/// (`stored: true`, `history` as it was before this row); 409 when there
/// is no second version to compare against; 503 when this daemon cannot
/// write. The operator role is required by the route table.
pub(crate) async fn store(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(target): Path<String>,
    Query(q): Query<PerfQuery>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let a = answer(&state, &principal, target, &q).await?;
    let Some(baseline) = &a.baseline else {
        return Err(conflict(format!(
            "only one service.version ({}) seen for {}; there is no verdict to store",
            a.candidate.version, a.target
        )));
    };
    let row = verdict_row(
        &a.target,
        &a.probe,
        &baseline.version,
        &a.candidate.version,
        &a.comparison,
        a.as_of_ns,
        &a.envs,
    );
    if !store_verdict(&state, row).await? {
        return Err(unavailable(
            "this daemon has no writer; the verdict was not stored",
        ));
    }
    state.starmap_cache.clear();
    Ok((StatusCode::CREATED, Json(answer_json(&a, true))))
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

    fn change(version: &str, at: i64) -> VersionChange {
        VersionChange {
            at_ns: at,
            version: version.into(),
        }
    }

    #[test]
    fn runs_are_attributed_to_the_version_live_when_they_started() {
        let evidence = Evidence {
            versions: vec![seen("1.4.0", 100, 400), seen("1.5.0", 500, 900)],
            changes: vec![change("1.4.0", 100), change("1.5.0", 500)],
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
        // `mid` started after 1.4.0 last spoke and before 1.5.0 first did:
        // 1.4.0 was still the live version.
        assert_eq!(baseline.runs, vec!["old", "mid"]);
        assert_eq!(baseline.samples, vec![10.0, 11.0]);
    }

    #[test]
    fn single_version_has_no_baseline_and_none_is_none() {
        let evidence = Evidence {
            versions: vec![seen("1.0.0", 1, 2)],
            changes: vec![change("1.0.0", 1)],
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
    fn after_a_rollback_runs_belong_to_the_version_rolled_back_to() {
        // 1.5.0 ran first, 1.6.0 replaced it at 100 and was rolled back at
        // 350; 1.5.0 has been live ever since, and fell silent at 900.
        let evidence = Evidence {
            versions: vec![seen("1.6.0", 100, 300), seen("1.5.0", 50, 900)],
            changes: vec![
                change("1.5.0", 50),
                change("1.6.0", 100),
                change("1.5.0", 350),
            ],
            runs: vec![
                run("before-any-telemetry", 10),
                run("first-1.5", 60),
                run("during-1.6", 200),
                run("in-the-gap", 320),
                run("after-rollback", 600),
                run("after-the-last-span", 950),
            ],
            samples: vec![],
            history: vec![],
        };
        let (candidate, baseline) = split(&evidence).expect("versions");
        assert_eq!(candidate.version, "1.5.0");
        // A run in a telemetry gap after the rollback is 1.5.0's evidence:
        // 1.5.0 was live, however much later 1.6.0 was first seen.
        assert_eq!(
            candidate.runs,
            vec!["first-1.5", "after-rollback", "after-the-last-span"]
        );
        let baseline = baseline.expect("baseline");
        assert_eq!(baseline.version, "1.6.0");
        // 1.6.0 owns only what started while it was the live version,
        // including the quiet stretch before the rollback was observed.
        assert_eq!(baseline.runs, vec!["during-1.6", "in-the-gap"]);
    }

    #[test]
    fn owner_is_the_last_version_to_speak_before_the_run() {
        let changes = vec![change("a", 10), change("b", 20)];
        assert_eq!(owner_of(&changes, 9), None);
        assert_eq!(owner_of(&changes, 10), Some("a"));
        assert_eq!(owner_of(&changes, 19), Some("a"));
        assert_eq!(owner_of(&changes, 20), Some("b"));
        assert_eq!(owner_of(&[], 20), None);
    }

    #[test]
    fn the_missing_target_message_names_the_environment_filter() {
        let plain = no_versions_message("checkout", &[]);
        assert!(
            plain.contains("no span with service.name checkout"),
            "{plain}"
        );
        assert!(!plain.contains("environment"), "{plain}");
        let one = no_versions_message("checkout", &["prod".into()]);
        assert!(one.contains("in environment prod:"), "{one}");
        for key in [
            "resilience.target.environment",
            "deployment.environment.name",
            "deployment.environment)",
        ] {
            assert!(one.contains(key), "{one}");
        }
        let two = no_versions_message("checkout", &["prod".into(), "demo".into()]);
        assert!(two.contains("in environments prod, demo:"), "{two}");
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
