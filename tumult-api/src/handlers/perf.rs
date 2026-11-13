//! `GET /api/perf/{target}` and `POST /api/perf/{target}/verdicts` — the
//! performance regression verdict.
//!
//! Compares the latency samples the target produced under two of its
//! `service.version`s. A target's versions are the `service.version`
//! values on the telemetry whose `service.name` is the target (the
//! service's own spans). The samples are a run's `during`-phase
//! `probe_samples` rows for one series (default `load.latency_p95_ms`, the
//! per-window p95 a streaming load adapter writes). The verdict is
//! `tumult_perf::compare`'s.
//!
//! # Which version a run measured
//!
//! Each experiment run against the target (root spans with
//! `resilience.target.system`) is attributed from the target's spans in
//! the run's own environment only, so a busy production deployment never
//! claims a run made in another environment:
//!
//! 1. the version carried by most of the target's spans inside the run's
//!    own window (start to end of the root span) — the version that
//!    actually served the load, also when it was idle until the load
//!    began. When several versions tie (a canary taking half the
//!    traffic), the one deployed latest — first seen latest in that
//!    environment — wins, then the greater version string;
//! 2. with no span of the target inside the window, the version of its
//!    last span at or before the run's start: the version live at that
//!    moment, which after a rollback is the older one again;
//! 3. otherwise the run is left out.
//!
//! A run that names no environment is matched against the target's spans
//! from every environment in scope.
//!
//! # Candidate and baseline
//!
//! A version is *live* while it still reports: its last span in some
//! environment is no more than [`LIVE_WINDOW_NS`] (one minute) behind the
//! latest span of the target there. The candidate is the live version
//! deployed latest (first seen latest); the baseline is the next live
//! one, or else the version that went silent most recently. Versions
//! running side by side — in two environments, or as a canary in one —
//! therefore keep a fixed order however their spans interleave, and a
//! version that was rolled back stops being the candidate once it has
//! been silent for a minute of the other's telemetry.
//!
//! # Reading and storing
//!
//! `GET` is a pure read and never writes; a `store` parameter on it is a
//! 400 that names the `POST` route. `POST …/verdicts` computes the same
//! answer and appends it to `perf_verdicts`; the route table puts it at
//! the operator role. Stored rows carry the environment list they were
//! computed under. Storing is idempotent: when the latest stored row for
//! the same target, probe, versions, quantile and environments holds the
//! same evidence, nothing is appended and the answer is 200 with
//! `duplicate: true` instead of 201.
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

use std::collections::{BTreeMap, HashMap, HashSet};
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
/// How far behind an environment's latest span of the target a version's
/// last span may lie for the version to count as still live there.
pub(crate) const LIVE_WINDOW_NS: i64 = 60 * 1_000_000_000;

#[derive(Deserialize)]
pub(crate) struct PerfQuery {
    probe: Option<String>,
    quantile: Option<String>,
    confidence: Option<f64>,
    min_samples: Option<usize>,
    margin_pct: Option<f64>,
    env: Option<String>,
    /// Not a parameter: `GET ?store=` is refused rather than ignored.
    store: Option<String>,
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

/// A `service.version` in one environment and when the target's
/// telemetry there first and last carried it. `environment` is `None`
/// for spans that name none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Sighting {
    pub environment: Option<String>,
    pub version: String,
    pub first_seen_ns: i64,
    pub last_seen_ns: i64,
}

/// A `service.version` across the environments in scope: when it was
/// first and last seen, and whether it still reports (see [`rank`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VersionSeen {
    pub version: String,
    pub first_seen_ns: i64,
    pub last_seen_ns: i64,
    pub live: bool,
}

/// A run against the target, with what the target's own spans say about
/// the version that served it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct RunSeen {
    pub experiment_id: String,
    pub started_at_ns: i64,
    pub ended_at_ns: i64,
    /// The environment the run named, if any.
    pub environment: Option<String>,
    /// `(version, spans)`: the target's spans inside the run's window, in
    /// the run's environment.
    pub inside: Vec<(String, u64)>,
    /// The version of the target's last span at or before the run's
    /// start, in the run's environment.
    pub before: Option<String>,
}

/// What the lake knows about the target, before attribution. Every part
/// is aggregated in SQL: its size follows the number of versions,
/// environments and runs, never the number of spans.
#[derive(Debug, Default)]
pub(crate) struct Evidence {
    pub sightings: Vec<Sighting>,
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

/// The target's versions, candidate first: the live ones by first sight,
/// latest first, then the silent ones by last sight, latest first. A
/// version is live when, in some environment, its last span is within
/// [`LIVE_WINDOW_NS`] of that environment's latest span of the target.
/// The order depends on first sights and on silences longer than the
/// window only, so it does not change with which of two concurrent
/// versions happened to speak last.
pub(crate) fn rank(sightings: &[Sighting]) -> Vec<VersionSeen> {
    let mut latest: HashMap<Option<&str>, i64> = HashMap::new();
    for s in sightings {
        let last = latest.entry(s.environment.as_deref()).or_insert(i64::MIN);
        *last = (*last).max(s.last_seen_ns);
    }
    let mut versions: BTreeMap<&str, VersionSeen> = BTreeMap::new();
    for s in sightings {
        let live = s.last_seen_ns >= latest[&s.environment.as_deref()] - LIVE_WINDOW_NS;
        let seen = versions
            .entry(s.version.as_str())
            .or_insert_with(|| VersionSeen {
                version: s.version.clone(),
                first_seen_ns: s.first_seen_ns,
                last_seen_ns: s.last_seen_ns,
                live: false,
            });
        seen.first_seen_ns = seen.first_seen_ns.min(s.first_seen_ns);
        seen.last_seen_ns = seen.last_seen_ns.max(s.last_seen_ns);
        seen.live |= live;
    }
    let mut ranked: Vec<VersionSeen> = versions.into_values().collect();
    ranked.sort_by(|a, b| {
        let recency = |v: &VersionSeen| {
            if v.live {
                v.first_seen_ns
            } else {
                v.last_seen_ns
            }
        };
        b.live
            .cmp(&a.live)
            .then(recency(b).cmp(&recency(a)))
            .then(b.version.cmp(&a.version))
    });
    ranked
}

/// When `version` was deployed where the run ran: its first sight in the
/// run's environment, or anywhere for a run that names none.
fn deployed_at(sightings: &[Sighting], run: &RunSeen, version: &str) -> i64 {
    sightings
        .iter()
        .filter(|s| s.version == version)
        .filter(|s| run.environment.is_none() || s.environment == run.environment)
        .map(|s| s.first_seen_ns)
        .min()
        .unwrap_or(i64::MIN)
}

/// The version a run measured (the module docs give the rule): most of
/// the target's spans inside the run's window, ties to the version
/// deployed latest and then to the greater version string; with no span
/// inside the window, the version live when the run started. `None` when
/// the target's telemetry in the run's environment says neither.
fn owner_of<'a>(sightings: &[Sighting], run: &'a RunSeen) -> Option<&'a str> {
    let key = |(version, spans): &(String, u64)| {
        (
            *spans,
            deployed_at(sightings, run, version),
            version.clone(),
        )
    };
    let served = run.inside.iter().max_by_key(|entry| key(entry));
    match served {
        Some((version, _)) => Some(version.as_str()),
        None => run.before.as_deref(),
    }
}

/// Attribute every run to the version it measured (see [`owner_of`]) and
/// return `(candidate, baseline)`: the first two of `versions`, which
/// [`rank`] ordered. `None` when the target has no versions at all.
pub(crate) fn split(versions: &[VersionSeen], evidence: &Evidence) -> Option<(Side, Option<Side>)> {
    let candidate = versions.first()?;
    let baseline = versions.get(1);

    let mut runs_of: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for run in &evidence.runs {
        if let Some(version) = owner_of(&evidence.sightings, run) {
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
    Some((side(candidate), baseline.map(side)))
}

/// The CTEs every attribution query starts from: `runs`, one row per run
/// against the target with its window and environment, and `svc`, the
/// target's own versioned spans.
fn attribution_ctes(t: &str, scope: &str) -> String {
    format!(
        "WITH runs AS (\
         SELECT experiment_id AS id, min(ts_ns) AS started, \
         max(ts_ns + duration_ns) AS ended, min(target_environment) AS env \
         FROM spans WHERE span_name = 'resilience.experiment' AND target_system = {t} \
         AND experiment_id IS NOT NULL{scope} GROUP BY experiment_id), \
         svc AS (\
         SELECT ts_ns, service_version AS v, target_environment AS env \
         FROM spans WHERE service_name = {t} AND service_version IS NOT NULL{scope})"
    )
}

/// The target's versions per environment.
fn load_sightings(reader: &Reader, t: &str, scope: &str) -> Result<Vec<Sighting>, String> {
    let rows = reader
        .query_json_rows(&format!(
            "SELECT target_environment AS env, service_version AS v, \
             min(ts_ns) AS first_seen, max(ts_ns) AS last_seen \
             FROM spans WHERE service_name = {t} AND service_version IS NOT NULL{scope} \
             GROUP BY target_environment, service_version \
             ORDER BY first_seen, service_version"
        ))
        .map_err(|e| e.to_string())?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            Some(Sighting {
                environment: r.get("env").and_then(Value::as_str).map(str::to_string),
                version: r.get("v")?.as_str()?.to_string(),
                first_seen_ns: r.get("first_seen")?.as_i64()?,
                last_seen_ns: r.get("last_seen")?.as_i64()?,
            })
        })
        .collect())
}

/// Per run, how many of the target's spans each version has inside the
/// run's window in the run's environment: at most runs × versions rows.
fn load_inside(reader: &Reader, ctes: &str) -> Result<HashMap<String, Vec<(String, u64)>>, String> {
    let rows = reader
        .query_json_rows(&format!(
            "{ctes} SELECT r.id AS id, s.v AS v, count(*) AS n \
             FROM runs AS r INNER JOIN svc AS s \
             ON (r.env IS NULL OR s.env = r.env) \
             AND s.ts_ns >= r.started AND s.ts_ns <= r.ended \
             GROUP BY r.id, s.v ORDER BY r.id, s.v"
        ))
        .map_err(|e| e.to_string())?;
    let mut inside: HashMap<String, Vec<(String, u64)>> = HashMap::new();
    for r in &rows {
        let (Some(id), Some(v), Some(n)) = (
            r.get("id").and_then(Value::as_str),
            r.get("v").and_then(Value::as_str),
            r.get("n").and_then(Value::as_u64),
        ) else {
            continue;
        };
        inside
            .entry(id.to_string())
            .or_default()
            .push((v.to_string(), n));
    }
    Ok(inside)
}

/// Per run, the version of the target's last span at or before the run's
/// start, in the run's environment (any environment in scope for a run
/// that names none): one row per run, found by an as-of join. Spans at
/// the same nanosecond resolve to the greater version string.
fn load_before(reader: &Reader, ctes: &str) -> Result<HashMap<String, String>, String> {
    let rows = reader
        .query_json_rows(&format!(
            "{ctes}, \
             in_env AS (SELECT env, ts_ns, max(v) AS v FROM svc GROUP BY env, ts_ns), \
             anywhere AS (SELECT ts_ns, max(v) AS v FROM svc GROUP BY ts_ns) \
             SELECT r.id AS id, s.v AS v FROM runs AS r \
             ASOF INNER JOIN in_env AS s ON r.env = s.env AND r.started >= s.ts_ns \
             UNION ALL \
             SELECT r.id AS id, s.v AS v FROM (SELECT id, started FROM runs WHERE env IS NULL) AS r \
             ASOF INNER JOIN anywhere AS s ON r.started >= s.ts_ns"
        ))
        .map_err(|e| e.to_string())?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            Some((
                r.get("id")?.as_str()?.to_string(),
                r.get("v")?.as_str()?.to_string(),
            ))
        })
        .collect())
}

/// Every run against the target with its window, its environment and
/// what the target's spans say about the version that served it.
fn load_runs(reader: &Reader, t: &str, scope: &str) -> Result<Vec<RunSeen>, String> {
    let ctes = attribution_ctes(t, scope);
    let mut inside = load_inside(reader, &ctes)?;
    let mut before = load_before(reader, &ctes)?;
    let rows = reader
        .query_json_rows(&format!(
            "{ctes} SELECT id, started, ended, env FROM runs ORDER BY started, id"
        ))
        .map_err(|e| e.to_string())?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            let id = r.get("id")?.as_str()?.to_string();
            Some(RunSeen {
                started_at_ns: r.get("started")?.as_i64()?,
                ended_at_ns: r.get("ended")?.as_i64()?,
                environment: r.get("env").and_then(Value::as_str).map(str::to_string),
                inside: inside.remove(&id).unwrap_or_default(),
                before: before.remove(&id),
                experiment_id: id,
            })
        })
        .collect())
}

/// The requested probe series of `runs`, as `(experiment_id, value)`.
fn load_samples(
    reader: &Reader,
    probe: &str,
    runs: &[RunSeen],
) -> Result<Vec<(String, f64)>, String> {
    if runs.is_empty() {
        return Ok(Vec::new());
    }
    let ids = runs
        .iter()
        .map(|r| sql_string(&r.experiment_id))
        .collect::<Vec<_>>()
        .join(", ");
    let rows = reader
        .query_json_rows(&format!(
            "SELECT experiment_id AS id, value FROM probe_samples \
             WHERE probe_name = {} AND phase = 'during' AND experiment_id IN ({ids}) \
             ORDER BY ts_ns",
            sql_string(probe)
        ))
        .map_err(|e| e.to_string())?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            Some((
                r.get("id")?.as_str()?.to_string(),
                r.get("value")?.as_f64()?,
            ))
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
    let sightings = load_sightings(reader, &t, &scope)?;
    let runs = load_runs(reader, &t, &scope)?;
    let samples = load_samples(reader, probe, &runs)?;
    let history = reader
        .perf_verdicts(target, envs, HISTORY_LIMIT)
        .map_err(|e| e.to_string())?;
    Ok(Evidence {
        sightings,
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

/// What the writer did with a verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stored {
    /// A new row was appended.
    Appended,
    /// The latest stored row already holds this verdict.
    Duplicate,
    /// The API has no ingest handle (read-only deployment).
    NoWriter,
}

/// Append the verdict through the single-writer channel unless the latest
/// stored row for the same comparison already holds it.
async fn store_verdict(state: &ApiState, row: PerfVerdictRow) -> Result<Stored, ApiError> {
    let Some(ingest) = state.ingest_handle() else {
        return Ok(Stored::NoWriter);
    };
    let slot = Arc::new(Mutex::new(None));
    let slot2 = Arc::clone(&slot);
    ingest
        .write(Batch::Exec(Box::new(move |writer: &Writer| {
            *slot2.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(writer.append_perf_verdict(&row));
            Ok(())
        })))
        .await
        .map_err(|e| internal(e.to_string()))?;
    let result = slot.lock().unwrap_or_else(|e| e.into_inner()).take();
    match result {
        Some(Ok(true)) => Ok(Stored::Appended),
        Some(Ok(false)) => Ok(Stored::Duplicate),
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
    versions: Vec<VersionSeen>,
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
    let versions = rank(&evidence.sightings);
    let Some((candidate, baseline)) = split(&versions, &evidence) else {
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
        versions,
        evidence,
        candidate,
        baseline,
        comparison,
    })
}

fn answer_json(a: &Answer, stored: bool, duplicate: bool) -> Value {
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
        "versions_seen": a.versions.iter().map(|v| json!({
            "version": v.version,
            "first_seen_ns": v.first_seen_ns,
            "last_seen_ns": v.last_seen_ns,
            "live": v.live,
        })).collect::<Vec<_>>(),
        "stored": stored,
        "duplicate": duplicate,
        "history": a.evidence.history,
        "as_of_ns": a.as_of_ns,
    })
}

/// `GET /api/perf/{target}?probe=&quantile=&confidence=&min_samples=&margin_pct=&env=`
/// — a pure read; `stored` is always `false`. A `store` parameter is a
/// 400: this route never writes, and ignoring the flag would let a caller
/// believe it had stored something.
pub(crate) async fn perf(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(target): Path<String>,
    Query(q): Query<PerfQuery>,
) -> Result<Json<Value>, ApiError> {
    if q.store.is_some() {
        return Err(bad_request(format!(
            "GET /api/perf/{target} never stores; store the verdict with \
             POST /api/perf/{target}/verdicts"
        )));
    }
    let a = answer(&state, &principal, target, &q).await?;
    Ok(Json(answer_json(&a, false, false)))
}

/// `POST /api/perf/{target}/verdicts` with the same query string: compute
/// the verdict and append it to `perf_verdicts`. 201 with the answer
/// (`stored: true`, `history` as it was before this row); 200 with
/// `duplicate: true` when the latest stored row for the same comparison
/// already holds this evidence, so a retry appends nothing; 409 when
/// there is no second version to compare against; 503 when this daemon
/// cannot write. The operator role is required by the route table.
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
    match store_verdict(&state, row).await? {
        Stored::Appended => Ok((StatusCode::CREATED, Json(answer_json(&a, true, false)))),
        Stored::Duplicate => Ok((StatusCode::OK, Json(answer_json(&a, true, true)))),
        Stored::NoWriter => Err(unavailable(
            "this daemon has no writer; the verdict was not stored",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: i64 = 1_000_000_000;

    fn sighting(env: Option<&str>, version: &str, first: i64, last: i64) -> Sighting {
        Sighting {
            environment: env.map(str::to_string),
            version: version.into(),
            first_seen_ns: first,
            last_seen_ns: last,
        }
    }

    fn demo(version: &str, first: i64, last: i64) -> Sighting {
        sighting(Some("demo"), version, first, last)
    }

    /// A demo run that found no span of the target inside its window.
    fn run(id: &str, before: Option<&str>) -> RunSeen {
        RunSeen {
            experiment_id: id.into(),
            environment: Some("demo".into()),
            before: before.map(str::to_string),
            ..RunSeen::default()
        }
    }

    /// A demo run with the target's spans per version inside its window.
    fn serving(id: &str, inside: &[(&str, u64)], before: Option<&str>) -> RunSeen {
        RunSeen {
            inside: inside.iter().map(|(v, n)| ((*v).to_string(), *n)).collect(),
            ..run(id, before)
        }
    }

    fn evidence(sightings: Vec<Sighting>, runs: Vec<RunSeen>) -> Evidence {
        Evidence {
            sightings,
            runs,
            ..Evidence::default()
        }
    }

    fn sides(evidence: &Evidence) -> (Side, Option<Side>) {
        split(&rank(&evidence.sightings), evidence).expect("versions")
    }

    fn order(sightings: &[Sighting]) -> Vec<String> {
        rank(sightings).into_iter().map(|v| v.version).collect()
    }

    #[test]
    fn a_quiet_run_goes_to_the_version_live_when_it_started() {
        let mut evidence = evidence(
            vec![
                demo("1.4.0", 100 * S, 400 * S),
                demo("1.5.0", 500 * S, 900 * S),
            ],
            vec![
                run("old", Some("1.4.0")),
                run("mid", Some("1.4.0")),
                run("new", Some("1.5.0")),
                run("ancient", None),
            ],
        );
        evidence.samples = vec![
            ("old".into(), 10.0),
            ("mid".into(), 11.0),
            ("new".into(), 20.0),
            ("ancient".into(), 99.0),
        ];
        let (candidate, baseline) = sides(&evidence);
        assert_eq!(candidate.version, "1.5.0");
        assert_eq!(candidate.runs, vec!["new"]);
        assert_eq!(candidate.samples, vec![20.0]);
        let baseline = baseline.expect("baseline");
        assert_eq!(baseline.version, "1.4.0");
        assert_eq!(baseline.runs, vec!["old", "mid"]);
        // A run older than all of the target's telemetry belongs to nobody.
        assert_eq!(baseline.samples, vec![10.0, 11.0]);
    }

    #[test]
    fn single_version_has_no_baseline_and_none_is_none() {
        let evidence = evidence(vec![demo("1.0.0", S, 2 * S)], vec![run("r", Some("1.0.0"))]);
        let (candidate, baseline) = sides(&evidence);
        assert_eq!(candidate.runs, vec!["r"]);
        assert!(baseline.is_none());
        assert!(split(&[], &Evidence::default()).is_none());
    }

    #[test]
    fn the_spans_inside_the_window_decide_before_the_version_live_at_the_start() {
        // The candidate was idle until the load began: its first span is
        // after the run's start, where 1.0.0 had spoken last.
        let idle = serving("idle-candidate", &[("1.1.0", 40)], Some("1.0.0"));
        let sightings = [demo("1.0.0", S, 50 * S), demo("1.1.0", 100 * S, 130 * S)];
        assert_eq!(owner_of(&sightings, &idle), Some("1.1.0"));
        // Nothing before the start at all, and still attributed.
        let fresh = serving("fresh", &[("1.1.0", 3)], None);
        assert_eq!(owner_of(&sightings, &fresh), Some("1.1.0"));
        assert_eq!(owner_of(&sightings, &run("silent", None)), None);
    }

    #[test]
    fn overlapping_versions_resolve_by_share_then_by_latest_deploy() {
        let sightings = [demo("1.0.0", S, 900 * S), demo("1.1.0", 500 * S, 900 * S)];
        // A 10 % canary: the old version served the load.
        let canary = serving("canary", &[("1.0.0", 90), ("1.1.0", 10)], Some("1.1.0"));
        assert_eq!(owner_of(&sightings, &canary), Some("1.0.0"));
        // An even split goes to the version deployed latest, whatever the
        // order of the rows and whoever spoke last before the run.
        let even = serving("even", &[("1.1.0", 50), ("1.0.0", 50)], Some("1.0.0"));
        assert_eq!(owner_of(&sightings, &even), Some("1.1.0"));
        let flipped = serving("even", &[("1.0.0", 50), ("1.1.0", 50)], Some("1.0.0"));
        assert_eq!(owner_of(&sightings, &flipped), Some("1.1.0"));
    }

    #[test]
    fn the_latest_deploy_is_judged_in_the_runs_own_environment() {
        // 2.0.0 reached prod first and demo last; 2.1.0 the other way.
        let sightings = [
            sighting(Some("prod"), "2.0.0", S, 900 * S),
            sighting(Some("prod"), "2.1.0", 800 * S, 900 * S),
            demo("2.1.0", 10 * S, 900 * S),
            demo("2.0.0", 700 * S, 900 * S),
        ];
        let tie = [("2.0.0", 5), ("2.1.0", 5)];
        assert_eq!(
            owner_of(&sightings, &serving("d", &tie, None)),
            Some("2.0.0")
        );
        let prod = RunSeen {
            environment: Some("prod".into()),
            ..serving("p", &tie, None)
        };
        assert_eq!(owner_of(&sightings, &prod), Some("2.1.0"));
        // A run that names no environment looks at all of them: both
        // versions were first seen there at different times, 2.1.0 later.
        let anywhere = RunSeen {
            environment: None,
            ..serving("a", &tie, None)
        };
        assert_eq!(owner_of(&sightings, &anywhere), Some("2.1.0"));
        // With every key equal the greater version string wins.
        let same = [demo("a", S, 2 * S), demo("b", S, 2 * S)];
        let even = serving("e", &[("b", 1), ("a", 1)], None);
        assert_eq!(owner_of(&same, &even), Some("b"));
    }

    #[test]
    fn concurrent_versions_rank_by_first_sight_whoever_spoke_last() {
        // Busy 1.0.0 in prod, 1.1.0 in demo: prod spoke last.
        let across = [
            sighting(Some("prod"), "1.0.0", S, 1000 * S),
            demo("1.1.0", 500 * S, 990 * S),
        ];
        assert_eq!(order(&across), vec!["1.1.0", "1.0.0"]);
        // A canary inside one environment, either one speaking last.
        let old_last = [demo("1.0.0", S, 1000 * S), demo("1.1.0", 500 * S, 999 * S)];
        let new_last = [demo("1.0.0", S, 999 * S), demo("1.1.0", 500 * S, 1000 * S)];
        assert_eq!(order(&old_last), vec!["1.1.0", "1.0.0"]);
        assert_eq!(order(&new_last), order(&old_last));
        assert!(rank(&old_last).iter().all(|v| v.live));
    }

    #[test]
    fn a_version_silent_for_longer_than_the_window_is_no_longer_the_candidate() {
        // 1.6.0 replaced 1.5.0 at 100 s and was rolled back at 300 s;
        // 1.5.0 has been reporting since.
        let rolled_back = [
            demo("1.5.0", 50 * S, 900 * S),
            demo("1.6.0", 100 * S, 300 * S),
        ];
        let ranked = rank(&rolled_back);
        assert_eq!(ranked[0].version, "1.5.0");
        assert!(ranked[0].live);
        assert_eq!(ranked[1].version, "1.6.0");
        assert!(!ranked[1].live);
        assert_eq!(ranked[0].first_seen_ns, 50 * S);
        assert_eq!(ranked[1].last_seen_ns, 300 * S);
        // Inside the window the rollback is not told from a canary yet.
        let just_now = [
            demo("1.5.0", 50 * S, 330 * S),
            demo("1.6.0", 100 * S, 300 * S),
        ];
        assert_eq!(order(&just_now), vec!["1.6.0", "1.5.0"]);
        // Silent versions follow the live ones, most recently seen first.
        let history = [
            demo("1.0.0", S, 100 * S),
            demo("1.1.0", 200 * S, 300 * S),
            demo("1.2.0", 400 * S, 900 * S),
        ];
        assert_eq!(order(&history), vec!["1.2.0", "1.1.0", "1.0.0"]);
    }

    #[test]
    fn a_version_live_in_any_environment_is_live() {
        // 1.0.0 is long gone from demo and still serving prod.
        let sightings = [
            demo("1.0.0", S, 100 * S),
            demo("1.1.0", 200 * S, 900 * S),
            sighting(Some("prod"), "1.0.0", S, 950 * S),
            sighting(None, "0.9.0", S, 2 * S),
        ];
        let ranked = rank(&sightings);
        let live: Vec<(&str, bool)> = ranked
            .iter()
            .map(|v| (v.version.as_str(), v.live))
            .collect();
        // Untagged spans are an environment of their own: 0.9.0 is the
        // latest there, so it is live, and the oldest deploy of the three.
        assert_eq!(
            live,
            vec![("1.1.0", true), ("1.0.0", true), ("0.9.0", true)]
        );
        assert_eq!(ranked[1].last_seen_ns, 950 * S);
    }

    #[test]
    fn runs_after_a_rollback_belong_to_the_version_rolled_back_to() {
        let evidence = evidence(
            vec![
                demo("1.5.0", 50 * S, 900 * S),
                demo("1.6.0", 100 * S, 300 * S),
            ],
            vec![
                run("before-any-telemetry", None),
                run("first-1.5", Some("1.5.0")),
                serving("during-1.6", &[("1.6.0", 30)], Some("1.5.0")),
                run("in-the-gap", Some("1.6.0")),
                serving("after-rollback", &[("1.5.0", 30)], Some("1.6.0")),
                run("after-the-last-span", Some("1.5.0")),
            ],
        );
        let (candidate, baseline) = sides(&evidence);
        assert_eq!(candidate.version, "1.5.0");
        assert_eq!(
            candidate.runs,
            vec!["first-1.5", "after-rollback", "after-the-last-span"]
        );
        let baseline = baseline.expect("baseline");
        assert_eq!(baseline.version, "1.6.0");
        // 1.6.0 owns what it served, and the quiet stretch before the
        // rollback was observed.
        assert_eq!(baseline.runs, vec!["during-1.6", "in-the-gap"]);
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
