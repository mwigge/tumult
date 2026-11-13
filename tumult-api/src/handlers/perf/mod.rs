//! The performance regression verdict: `GET /api/perf/{target}`,
//! `POST /api/perf/{target}/verdicts`, `GET /api/perf` and
//! `GET /api/perf/{target}/version`.
//!
//! The verdict compares the latency samples a target produced under two
//! of its versions inside one environment. The samples are a run's
//! `during`-phase `probe_samples` rows for one series (default
//! `load.latency_p95_ms`, the per-window p95 a streaming load adapter
//! writes). The verdict is `tumult_perf::compare`'s.
//!
//! # Which version a run measured
//!
//! A run is an experiment against the target (root spans with
//! `resilience.target.system`) that started inside the lookback `range`.
//!
//! 1. The version the run recorded is what it measured: the one the
//!    experiment or the command line declared, or the one the runner
//!    observed when the run ended. It is read from the journal, or from
//!    the run's root span when the journal has none. Such a run is
//!    counted as `declared` or `observed`.
//! 2. A run that recorded none is read from the target's own spans
//!    (`service.name` is the target) and counted as `inferred`: the
//!    version carried by most of them inside the run's window, ties to
//!    the version first seen latest and then to the greater version
//!    number; with no span inside the window, the version of the last
//!    span before the run's start, when that span is no more than an
//!    hour old.
//! 3. Otherwise the run has no version. It is counted under
//!    `unattributed_runs` with the reason and takes part in nothing.
//!
//! Only spans of exactly the run's environment are read for it; a run
//! that names no environment is read from the spans that name none.
//!
//! # One environment, two versions
//!
//! An answer is for one environment: the one `env=` names, else the only
//! one in which the target has versions, else the only one that can
//! answer (it has two versions, or every version the request names).
//! When several could, the answer is a 400 that asks for `env=` or for
//! explicit versions; evidence from two environments is never mixed.
//!
//! The environment's versions are ordered by deployment: first sight,
//! latest first, then the greater version number. The candidate is the
//! first and the baseline the one after it, unless `candidate=` and
//! `baseline=` name them (`selection` says which). The order does not
//! depend on which version reported last, so the same request against
//! the same runs gives the same verdict.
//!
//! # What is compared
//!
//! Each side holds the samples of the runs that measured its version. A
//! run whose load was aborted or truncated is counted in `partial` and
//! left out of the samples unless `include=partial`. Windows in which
//! nothing answered leave no latency behind; a side that lost more than a
//! tenth of its windows that way is not comparable and the verdict is
//! inconclusive, with the reason first.
//!
//! # Reading and storing
//!
//! `GET` is a pure read and never writes; `store=` is a 400 on both
//! routes. `POST …/verdicts` computes the same answer and appends it to
//! `perf_verdicts` with its environment; the route table puts it at the
//! operator role. Storing is idempotent: when the latest stored row for
//! the same comparison holds the same evidence nothing is appended and
//! the answer is 200 with `duplicate: true` instead of 201.
//!
//! # Environments
//!
//! A span's environment is what ingest read from, in order,
//! `resilience.target.environment` on the span, then on the resource,
//! then the resource's `deployment.environment.name`, then its
//! `deployment.environment`. The value is matched exactly: `prod` and
//! `production` are different environments. Telemetry that carries none
//! of them forms an environment of its own (`environment: null`), which
//! an unscoped principal can be answered for and no scoped one can.

mod answer;
mod evidence;
mod select;

use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use tumult_ingest::Batch;
use tumult_lake::{PerfVerdictRow, Reader, Writer};
use tumult_perf::{compare, CompareOptions, Comparison, Quantile, Verdict};

use self::answer::Answer;
use self::select::{Env, NoEnvironment, Side};
use crate::auth::Principal;
use crate::error::{bad_request, conflict, forbidden, not_found, unavailable, ApiError};
use crate::sql_util::{internal, now_ns, with_reader};
use crate::ApiState;

/// The series compared when `probe` is not given.
const DEFAULT_PROBE: &str = "load.latency_p95_ms";
/// The lookback when `range` is not given.
const DEFAULT_RANGE: &str = "30d";
const NS_PER_S: i64 = 1_000_000_000;

#[derive(Deserialize, Default)]
pub(crate) struct PerfQuery {
    probe: Option<String>,
    quantile: Option<String>,
    confidence: Option<f64>,
    min_samples: Option<usize>,
    margin_pct: Option<f64>,
    env: Option<String>,
    baseline: Option<String>,
    candidate: Option<String>,
    range: Option<String>,
    include: Option<String>,
    /// Not a parameter: `store=` is refused rather than ignored.
    store: Option<String>,
}

/// Validated comparison settings from the query string.
fn options_from_query(q: &PerfQuery) -> Result<CompareOptions, ApiError> {
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

/// The start of the lookback `range` ending at `as_of_ns`.
fn since(range: &str, as_of_ns: i64) -> Result<i64, ApiError> {
    select::parse_range(range)
        .and_then(|seconds| seconds.checked_mul(NS_PER_S))
        .map(|ns| as_of_ns.saturating_sub(ns))
        .ok_or_else(|| {
            bad_request("range must be a positive count of s, m, h, d or w, such as 12h or 30d")
        })
}

/// The environment a request names, when the principal may see it.
fn requested_env(principal: &Principal, env: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(env) = env else {
        return Ok(None);
    };
    if env.trim().is_empty() {
        return Err(bad_request(
            "env must name an environment; leave it out to have the only comparable one chosen",
        ));
    }
    if !principal.env_allowed(env) {
        return Err(forbidden(format!(
            "environment {env} is outside the principal's scopes"
        )));
    }
    Ok(Some(env.to_string()))
}

/// A version a request names.
fn named_version(name: &str, value: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.trim().is_empty() {
        return Err(bad_request(format!(
            "{name} must name a version; leave it out to follow the deployment order"
        )));
    }
    Ok(Some(value.to_string()))
}

fn include_partial(include: Option<&str>) -> Result<bool, ApiError> {
    match include {
        None => Ok(false),
        Some("partial") => Ok(true),
        Some(_) => Err(bad_request(
            "include must be partial (keep aborted and truncated runs) or be left out",
        )),
    }
}

/// A validated verdict request.
struct Request {
    target: String,
    probe: String,
    opts: CompareOptions,
    /// The environment the request names.
    env: Option<String>,
    /// The principal's scopes: empty for every environment.
    scopes: Vec<String>,
    baseline: Option<String>,
    candidate: Option<String>,
    range: String,
    since_ns: i64,
    as_of_ns: i64,
    include_partial: bool,
}

impl Request {
    fn new(principal: &Principal, target: String, q: &PerfQuery) -> Result<Self, ApiError> {
        if q.store.is_some() {
            return Err(bad_request(format!(
                "store is not a parameter: GET /api/perf/{target} never stores and \
                 POST /api/perf/{target}/verdicts always does"
            )));
        }
        let opts = options_from_query(q)?;
        let as_of_ns = now_ns();
        let range = q.range.clone().unwrap_or_else(|| DEFAULT_RANGE.to_string());
        let since_ns = since(&range, as_of_ns)?;
        let baseline = named_version("baseline", q.baseline.as_deref())?;
        let candidate = named_version("candidate", q.candidate.as_deref())?;
        if baseline.is_some() && baseline == candidate {
            return Err(bad_request(
                "baseline and candidate name the same version; there is nothing to compare",
            ));
        }
        let probe = q
            .probe
            .clone()
            .filter(|p| !p.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_PROBE.to_string());
        Ok(Self {
            env: requested_env(principal, q.env.as_deref())?,
            scopes: principal.env_scopes.clone(),
            include_partial: include_partial(q.include.as_deref())?,
            target,
            probe,
            opts,
            baseline,
            candidate,
            range,
            since_ns,
            as_of_ns,
        })
    }

    /// The named environments the request looks in: none for all of them.
    fn named_envs(&self) -> Vec<String> {
        match &self.env {
            Some(env) => vec![env.clone()],
            None => self.scopes.clone(),
        }
    }

    fn in_scope(&self, env: &Env) -> bool {
        self.env.is_none() || env == &self.env
    }
}

/// Why a request has no answer.
enum Refusal {
    NotFound(String),
    BadRequest(String),
}

impl From<Refusal> for ApiError {
    fn from(refusal: Refusal) -> Self {
        match refusal {
            Refusal::NotFound(message) => not_found(message),
            Refusal::BadRequest(message) => bad_request(message),
        }
    }
}

/// Why the target has no versions. Under an environment filter the answer
/// names it and how a span gets an environment, without saying whether
/// the target exists elsewhere.
fn no_versions_message(target: &str, envs: &[String]) -> String {
    if envs.is_empty() {
        return format!(
            "no service.version seen for target {target}: no span with service.name {target} \
             carries one"
        );
    }
    let plural = if envs.len() == 1 { "" } else { "s" };
    format!(
        "no service.version seen for target {target} in environment{plural} {}: only spans \
         tagged with exactly that environment count (resilience.target.environment, or the \
         resource's deployment.environment.name or deployment.environment)",
        envs.join(", ")
    )
}

fn env_name(env: &Env) -> &str {
    env.as_deref().unwrap_or("(none)")
}

/// Several environments could answer: ask which.
fn ambiguous_message(target: &str, envs: &[Env]) -> String {
    let names: Vec<&str> = envs.iter().map(env_name).collect();
    format!(
        "target {target} has versions in more than one environment ({}) and the request does \
         not single one out; name it with env=<environment>, or name versions only one of \
         them has seen with baseline= and candidate=",
        names.join(", ")
    )
}

fn unknown_version_message(req: &Request, env: &Env, unknown: &str, seen: &[&str]) -> String {
    format!(
        "version {unknown} of {} was not seen in environment {} within {}; seen there: {}",
        req.target,
        env_name(env),
        req.range,
        seen.join(", ")
    )
}

/// With no baseline there is nothing to compare against.
fn lone_reason(target: &str, candidate: &str, versions: usize) -> String {
    if versions == 1 {
        return format!(
            "only one service.version ({candidate}) seen for {target}; nothing to compare against"
        );
    }
    format!("no version of {target} was deployed before {candidate}; nothing to compare against")
}

/// `compare`'s verdict, made inconclusive when a side lost too many of
/// its windows to silence or when there is no baseline.
fn judge(
    target: &str,
    candidate: &Side,
    baseline: Option<&Side>,
    versions: usize,
    opts: &CompareOptions,
) -> Comparison {
    let baseline_samples = baseline.map_or(&[][..], |b| b.samples.as_slice());
    let mut c = compare(baseline_samples, &candidate.samples, opts);
    let mut reasons: Vec<String> = [
        baseline.and_then(|b| select::unanswered_reason("baseline", b)),
        select::unanswered_reason("candidate", candidate),
    ]
    .into_iter()
    .flatten()
    .collect();
    if baseline.is_none() {
        reasons.insert(0, lone_reason(target, &candidate.version, versions));
    }
    if reasons.is_empty() {
        return c;
    }
    c.verdict = Verdict::Inconclusive;
    reasons.append(&mut c.reasons);
    c.reasons = reasons;
    c
}

/// Read the evidence and decide the answer, or why there is none.
fn decide(reader: &Reader, req: &Request) -> Result<Result<Answer, Refusal>, String> {
    let found = evidence::load(reader, &req.target, &req.probe, &req.scopes, req.since_ns)?;
    let runs = select::attribute(&found.sightings, found.runs);
    let by_env = select::versions_by_env(&found.sightings, req.since_ns, &runs);
    let wanted: Vec<&str> = [req.baseline.as_deref(), req.candidate.as_deref()]
        .into_iter()
        .flatten()
        .collect();
    let environment = match select::choose_environment(req.env.as_deref(), &by_env, &wanted) {
        Ok(environment) => environment,
        Err(NoEnvironment::NoVersions) => {
            let message = no_versions_message(&req.target, &req.named_envs());
            return Ok(Err(Refusal::NotFound(message)));
        }
        Err(NoEnvironment::Ambiguous(envs)) => {
            let message = ambiguous_message(&req.target, &envs);
            return Ok(Err(Refusal::BadRequest(message)));
        }
    };
    let versions = by_env.get(&environment).cloned().unwrap_or_default();
    let picked = select::pick(&versions, req.baseline.as_deref(), req.candidate.as_deref());
    let (candidate, baseline) = match picked {
        Ok(picked) => picked,
        Err(unknown) => {
            let seen: Vec<&str> = versions.iter().map(|v| v.version.as_str()).collect();
            let message = unknown_version_message(req, &environment, &unknown, &seen);
            return Ok(Err(Refusal::BadRequest(message)));
        }
    };
    let side = |version: &str| {
        select::side(
            version,
            &environment,
            &runs,
            &found.samples,
            req.include_partial,
        )
    };
    let candidate = side(&candidate.version);
    let baseline = baseline.map(|b| side(&b.version));
    let compared = [
        Some(candidate.version.as_str()),
        baseline.as_ref().map(|b| b.version.as_str()),
    ];
    let runs_on_other_versions = runs
        .iter()
        .filter(|a| a.run.environment == environment)
        .filter_map(|a| a.owner.as_ref().ok())
        .filter(|(version, _)| !compared.contains(&Some(version.as_str())))
        .count();
    let comparison = judge(
        &req.target,
        &candidate,
        baseline.as_ref(),
        versions.len(),
        &req.opts,
    );
    Ok(Ok(Answer {
        target: req.target.clone(),
        probe: req.probe.clone(),
        environments_seen: select::sorted_envs(by_env.keys()),
        range: req.range.clone(),
        since_ns: req.since_ns,
        as_of_ns: req.as_of_ns,
        explicit: !wanted.is_empty(),
        include_partial: req.include_partial,
        min_samples: req.opts.min_samples,
        unattributed: answer::unattributed(&runs, |env| req.in_scope(env)),
        runs_on_other_versions,
        history: evidence::history(reader, &req.target, &environment)?,
        environment,
        versions,
        candidate,
        baseline,
        comparison,
    }))
}

async fn answer_for(
    state: &ApiState,
    principal: &Principal,
    target: String,
    q: &PerfQuery,
) -> Result<Answer, ApiError> {
    let req = Request::new(principal, target, q)?;
    let decided = with_reader(&state.db_path, move |reader| decide(reader, &req)).await?;
    decided.map_err(ApiError::from)
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

/// `GET /api/perf/{target}?probe=&quantile=&confidence=&min_samples=&margin_pct=&env=&baseline=&candidate=&range=&include=`
/// — a pure read; `stored` is always `false`. A `store` parameter is a
/// 400: this route never writes, and ignoring the flag would let a caller
/// believe it had stored something.
pub(crate) async fn perf(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(target): Path<String>,
    Query(q): Query<PerfQuery>,
) -> Result<Json<Value>, ApiError> {
    let a = answer_for(&state, &principal, target, &q).await?;
    Ok(Json(a.json(false, false)))
}

/// `POST /api/perf/{target}/verdicts` with the same query string: compute
/// the verdict and append it to `perf_verdicts`. 201 with the answer
/// (`stored: true`, `history` as it was before this row); 200 with
/// `duplicate: true` when the latest stored row for the same comparison
/// already holds this evidence, so a retry appends nothing; 409 when
/// there is no baseline to compare against; 503 when this daemon cannot
/// write. The operator role is required by the route table.
pub(crate) async fn store(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(target): Path<String>,
    Query(q): Query<PerfQuery>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let a = answer_for(&state, &principal, target, &q).await?;
    let Some(baseline) = &a.baseline else {
        return Err(conflict(format!(
            "no baseline for version {} of {}; there is no verdict to store",
            a.candidate.version, a.target
        )));
    };
    match store_verdict(&state, a.row(baseline)).await? {
        Stored::Appended => Ok((StatusCode::CREATED, Json(a.json(true, false)))),
        Stored::Duplicate => Ok((StatusCode::OK, Json(a.json(true, true)))),
        Stored::NoWriter => Err(unavailable(
            "this daemon has no writer; the verdict was not stored",
        )),
    }
}

#[derive(Deserialize)]
pub(crate) struct TargetsQuery {
    range: Option<String>,
}

/// `GET /api/perf?range=` — the targets experiments ran against inside
/// the lookback, with the environments of those runs, within the
/// principal's scopes.
pub(crate) async fn targets(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Query(q): Query<TargetsQuery>,
) -> Result<Json<Value>, ApiError> {
    let as_of_ns = now_ns();
    let range = q.range.unwrap_or_else(|| DEFAULT_RANGE.to_string());
    let since_ns = since(&range, as_of_ns)?;
    let scopes = principal.env_scopes.clone();
    let seen = with_reader(&state.db_path, move |reader| {
        evidence::targets(reader, &scopes, since_ns)
    })
    .await?;
    let targets: Vec<Value> = seen
        .iter()
        .map(|t| {
            json!({
                "target": t.target,
                "environments": t.environments,
                "runs": t.runs,
                "last_run_ns": t.last_run_ns,
            })
        })
        .collect();
    Ok(Json(json!({
        "targets": targets,
        "range": range,
        "as_of_ns": as_of_ns,
    })))
}

#[derive(Deserialize)]
pub(crate) struct VersionQuery {
    env: Option<String>,
    from_ns: Option<i64>,
    to_ns: Option<i64>,
}

/// `GET /api/perf/{target}/version?env=&from_ns=&to_ns=` — the version
/// that served a window: the one most of the target's spans carried
/// between the two instants in exactly that environment (without `env`,
/// the spans that name none), or `null` when the target reported nothing
/// versioned there then. A runner asks this once when its run ends.
pub(crate) async fn version(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(target): Path<String>,
    Query(q): Query<VersionQuery>,
) -> Result<Json<Value>, ApiError> {
    let (Some(from_ns), Some(to_ns)) = (q.from_ns, q.to_ns) else {
        return Err(bad_request("from_ns and to_ns are required"));
    };
    if from_ns > to_ns {
        return Err(bad_request("from_ns must not be later than to_ns"));
    }
    let env = requested_env(&principal, q.env.as_deref())?;
    if env.is_none() && !principal.env_scopes.is_empty() {
        return Err(forbidden(
            "telemetry without an environment is outside the principal's scopes; name one with env=",
        ));
    }
    let observed = {
        let (target, env) = (target.clone(), env.clone());
        with_reader(&state.db_path, move |reader| {
            reader
                .observed_version(&target, env.as_deref(), from_ns, to_ns)
                .map_err(|e| e.to_string())
        })
        .await?
    };
    Ok(Json(json!({
        "target": target,
        "environment": env,
        "from_ns": from_ns,
        "to_ns": to_ns,
        "version": observed,
    })))
}

#[cfg(test)]
mod tests {
    use self::select::{Execution, RunDetail, Windows};
    use super::*;

    #[test]
    fn the_missing_target_message_names_the_environment_filter() {
        let plain = no_versions_message("checkout", &[]);
        assert!(
            plain.contains("no span with service.name checkout carries one"),
            "{plain}"
        );
        assert!(!plain.contains("environment"), "{plain}");
        let one = no_versions_message("checkout", &["prod".into()]);
        assert!(
            one.contains("in environment prod: only spans tagged"),
            "{one}"
        );
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
    fn the_ambiguity_message_names_the_environments_and_the_ways_out() {
        let message = ambiguous_message("checkout", &[Some("demo".into()), None]);
        for part in ["demo", "(none)", "env=", "baseline=", "candidate="] {
            assert!(message.contains(part), "{part}: {message}");
        }
    }

    #[test]
    fn query_options_are_validated() {
        let q = |quantile: Option<&str>,
                 confidence: Option<f64>,
                 margin: Option<f64>,
                 min: Option<usize>| {
            PerfQuery {
                quantile: quantile.map(str::to_string),
                confidence,
                min_samples: min,
                margin_pct: margin,
                ..PerfQuery::default()
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

    #[test]
    fn the_lookback_starts_a_range_before_now() {
        let day = 86_400 * NS_PER_S;
        assert_eq!(since("30d", 100 * day).ok(), Some(70 * day));
        assert_eq!(since("1w", 100 * day).ok(), Some(93 * day));
        assert!(since("0d", 100 * day).is_err());
        assert!(since("soon", 100 * day).is_err());
        // Too long to count in nanoseconds: refused, not wrapped.
        assert!(since("9999999999999w", 100 * day).is_err());
    }

    fn side_with(version: &str, samples: Vec<f64>, windows: Windows) -> Side {
        Side {
            version: version.into(),
            samples,
            windows,
            ..Side::default()
        }
    }

    fn steady(around: f64) -> Vec<f64> {
        (0..40).map(|i| around + f64::from(i % 5)).collect()
    }

    #[test]
    fn a_side_that_went_silent_makes_the_verdict_inconclusive_with_the_reason_first() {
        let opts = CompareOptions::default();
        let whole = Windows {
            planned: 40,
            completed: 40,
            ..Windows::default()
        };
        let baseline = side_with("1.0.0", steady(100.0), whole);
        let same = side_with("1.1.0", steady(100.0), whole);
        let verdict = judge("checkout", &same, Some(&baseline), 2, &opts);
        assert_eq!(verdict.verdict, Verdict::Unchanged, "{:?}", verdict.reasons);

        let silent = Windows {
            unanswered: 12,
            ..whole
        };
        let quiet = side_with("1.1.0", steady(100.0), silent);
        let verdict = judge("checkout", &quiet, Some(&baseline), 2, &opts);
        assert_eq!(verdict.verdict, Verdict::Inconclusive);
        assert!(verdict.reasons[0].contains("candidate 1.1.0: 12 of 40"));
        assert!(verdict.reasons.len() > 1, "compare's own reasons are kept");
        let verdict = judge("checkout", &baseline, Some(&quiet), 2, &opts);
        assert!(verdict.reasons[0].contains("baseline 1.1.0"));
    }

    #[test]
    fn a_candidate_without_a_baseline_is_inconclusive_and_says_which_kind() {
        let opts = CompareOptions::default();
        let candidate = side_with("1.0.0", steady(100.0), Windows::default());
        let alone = judge("checkout", &candidate, None, 1, &opts);
        assert_eq!(alone.verdict, Verdict::Inconclusive);
        assert!(alone.reasons[0].contains("only one service.version (1.0.0)"));
        let oldest = judge("checkout", &candidate, None, 3, &opts);
        assert_eq!(oldest.verdict, Verdict::Inconclusive);
        assert!(oldest.reasons[0].contains("deployed before 1.0.0"));
    }

    #[test]
    fn a_stored_row_carries_the_one_environment_it_was_computed_for() {
        let detail = RunDetail {
            experiment_id: "r".into(),
            attribution: select::Attribution::Declared,
            status: "completed".into(),
            truncated: false,
            aborted: false,
            included: true,
        };
        let candidate = Side {
            details: vec![detail],
            ..side_with("1.1.0", steady(150.0), Windows::default())
        };
        let baseline = side_with("1.0.0", steady(100.0), Windows::default());
        let opts = CompareOptions::default();
        let mut a = Answer {
            target: "checkout".into(),
            probe: DEFAULT_PROBE.into(),
            environment: Some("demo".into()),
            environments_seen: vec![Some("demo".into())],
            range: DEFAULT_RANGE.into(),
            since_ns: 0,
            as_of_ns: 7,
            explicit: false,
            include_partial: false,
            min_samples: opts.min_samples,
            versions: Vec::new(),
            comparison: judge("checkout", &candidate, Some(&baseline), 2, &opts),
            candidate,
            baseline: Some(baseline.clone()),
            unattributed: Vec::new(),
            runs_on_other_versions: 0,
            history: Vec::new(),
        };
        let row = a.row(&baseline);
        assert_eq!(row.envs, Some(vec!["demo".to_string()]));
        assert_eq!(row.verdict, "regressed");
        assert_eq!((row.baseline_n, row.candidate_n), (40, 40));
        let body = a.json(false, false);
        assert_eq!(body["selection"], "deployment_order");
        assert_eq!(body["candidate"]["attribution"]["declared"], 1);
        assert_eq!(
            body["candidate"]["windows"]["unanswered_share"],
            Value::Null
        );
        a.environment = None;
        a.explicit = true;
        assert_eq!(a.row(&baseline).envs, None);
        assert_eq!(a.json(false, false)["selection"], "explicit");
        let _ = Execution::default();
    }
}
