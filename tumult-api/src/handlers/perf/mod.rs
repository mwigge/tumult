//! The performance regression verdict: `GET /api/perf/{target}`,
//! `POST /api/perf/{target}/verdicts`, `GET /api/perf` and
//! `GET /api/perf/{target}/version`.
//!
//! The verdict compares the latency samples a target produced under two
//! of its versions inside one environment, and before that what became
//! of the requests: a version that fails to answer is worse, not unknown,
//! and the verdict is never better than the evidence. The samples are a
//! run's `during`-phase `probe_samples` rows for one series (default
//! `load.latency_p95_ms`, the per-window p95 a streaming load adapter
//! writes).
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
//!    counted as `declared` or `observed`. A run whose runner saw two
//!    versions share its window recorded `mixed` and has no version.
//! 2. A run that recorded none is read from the target's own spans
//!    (`service.name` is the target) and counted as `inferred`: the
//!    version that served at least 95 % of the requests inside the run's
//!    window, widened by two seconds of clock skew; a request is a span
//!    the service received or began, so a chatty build weighs no more
//!    than a quiet one. When no version served that much the run has no
//!    version (`mixed_versions`). With no request inside the window, the
//!    version of the last span before the run's start, when that span is
//!    no more than an hour old.
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
//! The environment's versions are every version the target ever reported
//! there or a run ever recorded; `range` bounds the samples and nothing
//! else. They are ordered by their latest deployment, latest first, then
//! by the greater version number: a version's latest deployment is its
//! first sight, or, when it came back after another version had taken
//! over (a rollback), the start of its latest return (`redeployed`). The
//! candidate is the first and the baseline the one after it, unless
//! `candidate=` and `baseline=` name them; `selection` and
//! `selection_rule` say which rule chose the pair. When the candidate
//! came back and the baseline kept reporting beside it, the order cannot
//! be told from two builds running side by side and the answer is a 400
//! that asks for both names. While an old lake's version history is
//! still being read back (`backfill: running`) a pair the request did
//! not name in full is `inconclusive`.
//!
//! # What is compared
//!
//! Each side holds the samples of the runs that measured its version. A
//! run whose load was aborted or truncated is counted in `partial` and
//! left out unless `include=partial`. In order:
//!
//! 1. Windows in which nothing answered leave no latency behind. A
//!    baseline that lost more than a tenth of its windows that way is no
//!    yardstick: `inconclusive` (`baseline_unanswered`, or
//!    `both_unanswered`). A candidate that did while the baseline did
//!    not is `regressed` (`candidate_unanswered`).
//! 2. A candidate whose share of failed requests (sent and not answered
//!    well) is above the baseline's by more than one percentage point,
//!    and by more than chance (one-sided two-proportion test, z ≥ 1.645),
//!    is `regressed` (`failure_share`) whatever its latencies: the
//!    survivors of a build that hangs are fast. Runs whose tool counted
//!    no requests are named (`failure_data_missing`); the answer does not
//!    read their silence as zero failures.
//! 3. Otherwise `tumult_perf::compare_runs` decides on the latencies
//!    (`latency`, or `insufficient_evidence`): percentiles weigh a window
//!    by its seconds of load and the interval resamples whole runs. A
//!    sample that is a request given up on at its timeout is censored:
//!    the truth is that or more. A verdict that would rest on a censored
//!    quantile being no more than it reads — a candidate improved or
//!    unchanged, a baseline the candidate is slower than or level with —
//!    is `inconclusive` (`censored`).
//!
//! `reason_codes` lists the codes, the deciding one first (`reason`).
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
use tumult_core::types::{check_version_text, TargetVersion, VersionShare, VersionSource};
use tumult_ingest::Batch;
use tumult_lake::{PerfVerdictRow, Reader, Writer};
use tumult_perf::{compare_runs, CompareOptions, Comparison, Quantile, SampleSummary, Verdict};

use self::answer::Answer;
use self::select::{Env, NoEnvironment, Requests, Side, VersionSeen};
use crate::auth::Principal;
use crate::error::{bad_request, conflict, forbidden, not_found, unavailable, ApiError};
use crate::sql_util::{internal, now_ns, with_reader};
use crate::ApiState;

/// The series compared when `probe` is not given.
const DEFAULT_PROBE: &str = "load.latency_p95_ms";
/// The lookback when `range` is not given.
const DEFAULT_RANGE: &str = "30d";
const NS_PER_S: i64 = 1_000_000_000;
/// How many percentage points, as a fraction, a candidate's share of
/// failed requests may be above the baseline's before it is regressed.
pub(crate) const FAILURE_SHARE_MARGIN: f64 = 0.01;
/// The least z of the one-sided two-proportion test for that difference
/// to be more than chance (95 %).
const FAILURE_SHARE_Z: f64 = 1.645;
/// The widest window `GET /api/perf/{target}/version` reads.
const MAX_VERSION_WINDOW_NS: i64 = 24 * 3_600 * NS_PER_S;
/// The longest `service.namespace` a request may name, in bytes.
const MAX_NAMESPACE_LEN: usize = 128;

#[derive(Deserialize, Default)]
pub(crate) struct PerfQuery {
    probe: Option<String>,
    quantile: Option<String>,
    confidence: Option<f64>,
    min_samples: Option<usize>,
    min_runs: Option<usize>,
    seed: Option<u64>,
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
    let min_runs = q.min_runs.unwrap_or(defaults.min_runs);
    if min_runs < 1 {
        return Err(bad_request("min_runs must be at least 1"));
    }
    Ok(CompareOptions {
        quantile,
        confidence,
        min_samples,
        min_runs,
        seed: q.seed.unwrap_or(defaults.seed),
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

/// A version a request names, held to the bounds every version is held
/// to. What was sent is not repeated in the refusal.
fn named_version(name: &str, value: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.trim().is_empty() {
        return Err(bad_request(format!(
            "{name} must name a version; leave it out to follow the deployment order"
        )));
    }
    if let Err(e) = check_version_text(value) {
        return Err(bad_request(format!("{name} is not a version: {e}")));
    }
    Ok(Some(value.to_string()))
}

/// The `service.namespace` a request names.
fn named_namespace(value: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let bounded = !value.trim().is_empty()
        && value.len() <= MAX_NAMESPACE_LEN
        && !value.chars().any(char::is_control);
    if !bounded {
        return Err(bad_request(format!(
            "namespace must name a service.namespace of at most {MAX_NAMESPACE_LEN} bytes \
             without control characters"
        )));
    }
    Ok(Some(value.to_string()))
}

/// The window a version is asked for: both instants, in order, and no
/// more than a day apart.
fn version_window(from_ns: Option<i64>, to_ns: Option<i64>) -> Result<(i64, i64), String> {
    let (Some(from_ns), Some(to_ns)) = (from_ns, to_ns) else {
        return Err("from_ns and to_ns are required".into());
    };
    if from_ns > to_ns {
        return Err("from_ns must not be later than to_ns".into());
    }
    let within = to_ns
        .checked_sub(from_ns)
        .is_some_and(|width| width <= MAX_VERSION_WINDOW_NS);
    if !within {
        return Err("the window from from_ns to to_ns must be no wider than 24 h".into());
    }
    Ok((from_ns, to_ns))
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

    /// Whether the request names both versions, so no order is needed.
    fn names_the_pair(&self) -> bool {
        self.baseline.is_some() && self.candidate.is_some()
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
        "version {unknown} of {} was not seen in environment {}; seen there: {}",
        req.target,
        env_name(env),
        seen.join(", ")
    )
}

/// The candidate came back while the baseline kept reporting: ask for
/// the pair.
fn redeploy_message(target: &str, env: &Env, candidate: &str, baseline: &str) -> String {
    format!(
        "version {candidate} of {target} was deployed again in environment {} while {baseline} \
         kept reporting beside it, so which of the two was deployed latest cannot be told; \
         name the pair with baseline= and candidate=",
        env_name(env)
    )
}

/// Which rule chose the pair, in words.
fn selection_rule(
    env: &Env,
    explicit: bool,
    candidate: &VersionSeen,
    baseline: Option<&VersionSeen>,
) -> String {
    let env = env_name(env);
    let c = &candidate.version;
    if explicit {
        let against = baseline.map_or_else(
            || "no version was deployed before it".to_string(),
            |b| format!("it is compared against {}", b.version),
        );
        return format!(
            "the request named the pair in environment {env}: the candidate is {c} and {against}"
        );
    }
    let again = if candidate.redeployed {
        ", where it was deployed again after another version had taken over"
    } else {
        ""
    };
    let before = baseline.map_or_else(
        || "no version was deployed before it".to_string(),
        |b| format!("{} the one deployed before it", b.version),
    );
    format!("{c} is the version deployed latest in environment {env}{again}, and {before}")
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

/// The share of their requests each side failed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct FailureShares {
    pub baseline: f64,
    pub candidate: f64,
}

/// A verdict with the codes of its reasons, the deciding one first.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Judged {
    pub comparison: Comparison,
    pub codes: Vec<&'static str>,
    /// `None` when a side counted no requests.
    pub failure: Option<FailureShares>,
}

/// A verdict that does not come from the latencies, or that overrules
/// what they say.
struct Ruling {
    verdict: Verdict,
    code: &'static str,
    reasons: Vec<String>,
}

impl Ruling {
    fn inconclusive(code: &'static str, reasons: Vec<String>) -> Option<Self> {
        Some(Self {
            verdict: Verdict::Inconclusive,
            code,
            reasons,
        })
    }

    fn regressed(code: &'static str, reason: String) -> Option<Self> {
        Some(Self {
            verdict: Verdict::Regressed,
            code,
            reasons: vec![reason],
        })
    }
}

fn failure_shares(candidate: &Side, baseline: &Side) -> Option<FailureShares> {
    Some(FailureShares {
        baseline: baseline.requests.failure_share()?,
        candidate: candidate.requests.failure_share()?,
    })
}

/// Whether the candidate failed a greater share of its requests than the
/// baseline by more than [`FAILURE_SHARE_MARGIN`] and by more than
/// chance: the pooled two-proportion z is at least [`FAILURE_SHARE_Z`].
fn fails_more(candidate: &Requests, baseline: &Requests) -> bool {
    let (Some(c), Some(b)) = (candidate.failure_share(), baseline.failure_share()) else {
        return false;
    };
    if c - b <= FAILURE_SHARE_MARGIN {
        return false;
    }
    #[allow(clippy::cast_precision_loss)]
    let (sent_c, sent_b) = (candidate.sent as f64, baseline.sent as f64);
    #[allow(clippy::cast_precision_loss)]
    let pooled = (candidate.failed + baseline.failed) as f64 / (sent_c + sent_b);
    let spread = (pooled * (1.0 - pooled) * (1.0 / sent_c + 1.0 / sent_b)).sqrt();
    c - b >= FAILURE_SHARE_Z * spread
}

fn failure_reason(candidate: &Side, baseline: &Side) -> String {
    let share = |side: &Side| side.requests.failure_share().unwrap_or(0.0) * 100.0;
    format!(
        "candidate {}: {} of {} requests failed ({:.1}%) against {} of {} ({:.1}%) for \
         baseline {}, more than the {:.0} percentage point(s) a candidate may be above its \
         baseline and more than chance; the latencies leave out the requests that were not \
         answered",
        candidate.version,
        candidate.requests.failed,
        candidate.requests.sent,
        share(candidate),
        baseline.requests.failed,
        baseline.requests.sent,
        share(baseline),
        baseline.version,
        FAILURE_SHARE_MARGIN * 100.0
    )
}

/// What decides before the latencies are looked at: no baseline, a
/// history still being read back, silence, failed requests.
fn ruling_before_latency(
    target: &str,
    candidate: &Side,
    baseline: Option<&Side>,
    versions: usize,
    history_incomplete: bool,
) -> Option<Ruling> {
    let Some(baseline) = baseline else {
        let reason = lone_reason(target, &candidate.version, versions);
        return Ruling::inconclusive("no_baseline", vec![reason]);
    };
    if history_incomplete {
        let reason = format!(
            "the version history of {target} is still being read back from this lake's spans, \
             so which version was deployed latest is not known yet; name the pair with \
             baseline= and candidate=, or ask again when backfill is done"
        );
        return Ruling::inconclusive("history_incomplete", vec![reason]);
    }
    let silent_baseline = select::unanswered_reason("baseline", baseline);
    let silent_candidate = select::unanswered_reason("candidate", candidate);
    match (silent_baseline, silent_candidate) {
        (Some(b), Some(c)) => return Ruling::inconclusive("both_unanswered", vec![b, c]),
        (Some(b), None) => return Ruling::inconclusive("baseline_unanswered", vec![b]),
        (None, Some(c)) => return Ruling::regressed("candidate_unanswered", c),
        (None, None) => {}
    }
    if fails_more(&candidate.requests, &baseline.requests) {
        return Ruling::regressed("failure_share", failure_reason(candidate, baseline));
    }
    None
}

fn quantile_of(summary: &SampleSummary, quantile: Quantile) -> Option<f64> {
    match quantile {
        Quantile::P50 => summary.p50,
        Quantile::P95 => summary.p95,
        Quantile::P99 => summary.p99,
    }
}

/// Why `side`'s quantile is a floor rather than a measurement: it is at
/// or above a sample that is a request given up on at its timeout.
fn censored_reason(
    role: &str,
    side: &Side,
    summary: &SampleSummary,
    c: &Comparison,
) -> Option<String> {
    let floor = side.censored_floor?;
    let at = quantile_of(summary, c.quantile)?;
    if at < floor {
        return None;
    }
    Some(format!(
        "{role} {}: {} of {} windows report a request that was given up on at its timeout, \
         and the {} is among them: it is at least {floor} ms, how much more is not known",
        side.version,
        side.windows.censored,
        side.samples.len(),
        c.quantile
    ))
}

/// The latencies' verdict rests on a censored quantile being no more
/// than it reads: a candidate that is improved or unchanged, or a
/// baseline the candidate is slower than or level with.
fn ruling_on_censoring(c: &Comparison, candidate: &Side, baseline: &Side) -> Option<Ruling> {
    let better = matches!(c.verdict, Verdict::Improved | Verdict::Unchanged);
    let worse = matches!(c.verdict, Verdict::Regressed | Verdict::Unchanged);
    let reasons: Vec<String> = [
        censored_reason("candidate", candidate, &c.candidate, c).filter(|_| better),
        censored_reason("baseline", baseline, &c.baseline, c).filter(|_| worse),
    ]
    .into_iter()
    .flatten()
    .collect();
    if reasons.is_empty() {
        return None;
    }
    Ruling::inconclusive("censored", reasons)
}

/// What a side whose requests were not all counted says about it.
fn uncounted_reason(role: &str, side: &Side) -> Option<String> {
    let requests = &side.requests;
    if requests.runs_without_counts > 0 {
        return Some(format!(
            "{role} {}: {} run(s) counted no requests, so the share that failed is {}",
            side.version,
            requests.runs_without_counts,
            if requests.sent == 0 {
                "unknown"
            } else {
                "known for its other runs only"
            }
        ));
    }
    if requests.sent > 0 {
        return None;
    }
    Some(format!(
        "{role} {}: no requests were counted, so the share that failed is unknown",
        side.version
    ))
}

/// The verdict and why. Silence and failed requests decide before the
/// latencies do; the latencies are `compare_runs`'s, reported as measured
/// whatever decided.
fn judge(
    target: &str,
    candidate: &Side,
    baseline: Option<&Side>,
    versions: usize,
    opts: &CompareOptions,
    history_incomplete: bool,
) -> Judged {
    let none = Vec::new();
    let baseline_runs = baseline.map_or(&none, |b| &b.by_run);
    let mut c = compare_runs(baseline_runs, &candidate.by_run, opts);
    let ruling = ruling_before_latency(target, candidate, baseline, versions, history_incomplete)
        .or_else(|| baseline.and_then(|b| ruling_on_censoring(&c, candidate, b)));
    let mut codes = Vec::new();
    match ruling {
        Some(mut ruling) => {
            c.verdict = ruling.verdict;
            codes.push(ruling.code);
            ruling.reasons.append(&mut c.reasons);
            c.reasons = ruling.reasons;
        }
        None if c.verdict == Verdict::Inconclusive => codes.push("insufficient_evidence"),
        None => codes.push("latency"),
    }
    let Some(baseline) = baseline else {
        return Judged {
            comparison: c,
            codes,
            failure: None,
        };
    };
    let mut uncounted: Vec<String> = [
        uncounted_reason("baseline", baseline),
        uncounted_reason("candidate", candidate),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !uncounted.is_empty() {
        codes.push("failure_data_missing");
        c.reasons.append(&mut uncounted);
    }
    Judged {
        comparison: c,
        codes,
        failure: failure_shares(candidate, baseline),
    }
}

/// Read the evidence and decide the answer, or why there is none.
fn decide(reader: &Reader, req: &Request) -> Result<Result<Answer, Refusal>, String> {
    let found = evidence::load(reader, &req.target, &req.probe, &req.scopes, req.since_ns)?;
    let runs = select::attribute(&found.sightings, found.runs);
    let by_env = select::versions_by_env(&found.sightings, &found.recorded, &runs);
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
    if let Some(b) = baseline.filter(|b| select::redeploy_is_ambiguous(candidate, b)) {
        if !req.names_the_pair() {
            let message =
                redeploy_message(&req.target, &environment, &candidate.version, &b.version);
            return Ok(Err(Refusal::BadRequest(message)));
        }
    }
    let explicit = !wanted.is_empty();
    let selection_rule = selection_rule(&environment, explicit, candidate, baseline);
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
    let judged = judge(
        &req.target,
        &candidate,
        baseline.as_ref(),
        versions.len(),
        &req.opts,
        found.backfill_running && !req.names_the_pair(),
    );
    Ok(Ok(Answer {
        target: req.target.clone(),
        probe: req.probe.clone(),
        environments_seen: select::sorted_envs(by_env.keys()),
        range: req.range.clone(),
        since_ns: req.since_ns,
        as_of_ns: req.as_of_ns,
        explicit,
        selection_rule,
        backfill_running: found.backfill_running,
        include_partial: req.include_partial,
        min_samples: req.opts.min_samples,
        min_runs: req.opts.min_runs,
        unattributed: answer::unattributed(&runs, |env| req.in_scope(env)),
        runs_on_other_versions,
        history: evidence::history(reader, &req.target, &environment)?,
        environment,
        versions,
        candidate,
        baseline,
        comparison: judged.comparison,
        reason_codes: judged.codes,
        failure: judged.failure,
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

/// `GET /api/perf/{target}?probe=&quantile=&confidence=&min_samples=&min_runs=&seed=&margin_pct=&env=&baseline=&candidate=&range=&include=`
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
    namespace: Option<String>,
    from_ns: Option<i64>,
    to_ns: Option<i64>,
}

fn shares_json(shares: &[VersionShare]) -> Vec<Value> {
    let all: u64 = shares.iter().map(|s| s.requests).sum();
    shares
        .iter()
        .map(|s| {
            #[allow(clippy::cast_precision_loss)]
            let share = s.requests as f64 / all as f64;
            json!({"version": s.version, "requests": s.requests, "share": share})
        })
        .collect()
}

/// `GET /api/perf/{target}/version?env=&namespace=&from_ns=&to_ns=` —
/// what served a window: the requests each version of the target served
/// between the two instants in exactly that environment (without `env`,
/// the spans that name none; with `namespace`, only the service of that
/// `service.namespace`), most first. `version` is the one that served at
/// least 95 % of them (`version_source: observed`); when none did it is
/// `null` and the source `mixed`; when the target reported nothing
/// versioned there then, both are `null`. The window is at most 24 h
/// wide. A runner asks this when its run ends, and asks again until the
/// count has settled.
pub(crate) async fn version(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(target): Path<String>,
    Query(q): Query<VersionQuery>,
) -> Result<Json<Value>, ApiError> {
    let (from_ns, to_ns) = version_window(q.from_ns, q.to_ns).map_err(bad_request)?;
    let namespace = named_namespace(q.namespace.as_deref())?;
    let env = requested_env(&principal, q.env.as_deref())?;
    if env.is_none() && !principal.env_scopes.is_empty() {
        return Err(forbidden(
            "telemetry without an environment is outside the principal's scopes; name one with env=",
        ));
    }
    let shares = {
        let (target, env, namespace) = (target.clone(), env.clone(), namespace.clone());
        with_reader(&state.db_path, move |reader| {
            reader
                .observed_versions(
                    &target,
                    env.as_deref(),
                    namespace.as_deref(),
                    from_ns,
                    to_ns,
                )
                .map_err(|e| e.to_string())
        })
        .await?
    };
    let versions = shares_json(&shares);
    let observed = TargetVersion::observed(shares);
    let source = observed.as_ref().map(|v| v.version_source.to_string());
    let version = observed
        .filter(|v| v.version_source == VersionSource::Observed)
        .map(|v| v.version);
    Ok(Json(json!({
        "target": target,
        "environment": env,
        "namespace": namespace,
        "from_ns": from_ns,
        "to_ns": to_ns,
        "version": version,
        "version_source": source,
        "versions": versions,
    })))
}

#[cfg(test)]
mod tests {
    use self::select::{Requests, RunDetail, Windows};
    use super::*;
    use tumult_perf::Window;

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
        assert_eq!(opts.min_runs, 3);
        let runs = |min_runs: usize, seed: Option<u64>| PerfQuery {
            min_runs: Some(min_runs),
            seed,
            ..PerfQuery::default()
        };
        let opts = options_from_query(&runs(1, Some(7))).expect("one run, seeded");
        assert_eq!((opts.min_runs, opts.seed), (1, 7));
        assert!(options_from_query(&runs(0, None)).is_err());
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

    /// A side of four runs, the samples dealt to them in turn.
    fn side_with(version: &str, samples: Vec<f64>, windows: Windows) -> Side {
        let mut by_run = vec![Vec::new(); 4];
        for (i, value) in samples.iter().enumerate() {
            by_run[i % 4].push(Window::whole(*value));
        }
        by_run.retain(|run| !run.is_empty());
        Side {
            version: version.into(),
            samples,
            by_run,
            windows,
            ..Side::default()
        }
    }

    fn steady(around: f64) -> Vec<f64> {
        (0..40).map(|i| around + f64::from(i % 5)).collect()
    }

    const WHOLE: Windows = Windows {
        planned: 40,
        completed: 40,
        unanswered: 0,
        lost: 0,
        censored: 0,
    };

    fn counted(mut side: Side, sent: u64, failed: u64) -> Side {
        side.requests = Requests {
            sent,
            failed,
            runs_without_counts: 0,
        };
        side
    }

    fn judged(candidate: &Side, baseline: &Side) -> Judged {
        let opts = CompareOptions::default();
        judge("checkout", candidate, Some(baseline), 2, &opts, false)
    }

    #[test]
    fn a_silent_candidate_is_regressed_and_a_silent_baseline_is_no_yardstick() {
        let baseline = side_with("1.0.0", steady(100.0), WHOLE);
        let same = side_with("1.1.0", steady(100.0), WHOLE);
        let j = judged(&same, &baseline);
        assert_eq!(j.comparison.verdict, Verdict::Unchanged, "{:?}", j.codes);
        assert_eq!(j.codes[0], "latency");

        let silent = Windows {
            unanswered: 12,
            ..WHOLE
        };
        // A candidate that fails to answer is worse, not unknown, however
        // good the latencies of what it did answer.
        let quiet = side_with("1.1.0", steady(50.0), silent);
        let j = judged(&quiet, &baseline);
        assert_eq!(j.comparison.verdict, Verdict::Regressed);
        assert_eq!(j.codes[0], "candidate_unanswered");
        assert!(j.comparison.reasons[0].contains("candidate 1.1.0: 12 of 40"));
        assert!(
            j.comparison.reasons.len() > 1,
            "compare's own reasons are kept"
        );
        // The yardstick went silent: nothing can be said against it.
        let j = judged(&baseline, &quiet);
        assert_eq!(j.comparison.verdict, Verdict::Inconclusive);
        assert_eq!(j.codes[0], "baseline_unanswered");
        assert!(j.comparison.reasons[0].contains("baseline 1.1.0"));
        // Both did: nothing can be said at all, and both say why.
        let also = side_with("1.0.0", steady(100.0), silent);
        let j = judged(&quiet, &also);
        assert_eq!(j.comparison.verdict, Verdict::Inconclusive);
        assert_eq!(j.codes[0], "both_unanswered");
        assert!(j.comparison.reasons[0].contains("baseline 1.0.0"));
        assert!(j.comparison.reasons[1].contains("candidate 1.1.0"));
        // One window in ten is the limit, not over it.
        let edge = Windows {
            unanswered: 4,
            ..WHOLE
        };
        let j = judged(&side_with("1.1.0", steady(100.0), edge), &baseline);
        assert_eq!(j.codes[0], "latency");
    }

    #[test]
    fn a_candidate_that_fails_more_of_its_requests_is_regressed_whatever_its_latency() {
        let baseline = counted(side_with("1.0.0", steady(100.0), WHOLE), 4000, 0);
        // Four in ten hang; the rest are five times faster.
        let hanging = counted(side_with("1.1.0", steady(20.0), WHOLE), 4000, 1600);
        let j = judged(&hanging, &baseline);
        assert_eq!(j.comparison.verdict, Verdict::Regressed);
        assert_eq!(j.codes, vec!["failure_share"]);
        let reason = &j.comparison.reasons[0];
        assert!(reason.contains("1600 of 4000"), "{reason}");
        assert!(reason.contains("candidate 1.1.0"), "{reason}");
        assert_eq!(
            j.failure,
            Some(FailureShares {
                baseline: 0.0,
                candidate: 0.4,
            })
        );
        // The latencies are still reported as measured.
        assert!(j.comparison.delta.is_some_and(|d| d < -70.0));

        // The same share on both sides says nothing against the candidate.
        let failing = counted(side_with("1.0.0", steady(100.0), WHOLE), 4000, 200);
        let equal = counted(side_with("1.1.0", steady(100.0), WHOLE), 4000, 200);
        let j = judged(&equal, &failing);
        assert_eq!(j.comparison.verdict, Verdict::Unchanged);
        assert_eq!(j.codes, vec!["latency"]);
        // Nor does a difference inside the margin: one point is the limit.
        let slightly = counted(side_with("1.1.0", steady(100.0), WHOLE), 4000, 240);
        assert_eq!(judged(&slightly, &failing).codes, vec!["latency"]);
        let over = counted(side_with("1.1.0", steady(100.0), WHOLE), 4000, 300);
        assert_eq!(judged(&over, &failing).codes, vec!["failure_share"]);
        // A candidate that fails less is not regressed by it.
        assert_eq!(judged(&failing, &over).codes, vec!["latency"]);
        // One failure among fifty requests is over the margin and still
        // no more than chance.
        let few = counted(side_with("1.0.0", steady(100.0), WHOLE), 50, 0);
        let one = counted(side_with("1.1.0", steady(100.0), WHOLE), 50, 1);
        let j = judged(&one, &few);
        assert_eq!(j.comparison.verdict, Verdict::Unchanged);
        assert_eq!(j.codes, vec!["latency"]);
    }

    #[test]
    fn a_side_whose_requests_nobody_counted_says_the_failure_share_is_unknown() {
        let baseline = counted(side_with("1.0.0", steady(100.0), WHOLE), 4000, 0);
        let mut uncounted = side_with("1.1.0", steady(100.0), WHOLE);
        uncounted.requests.runs_without_counts = 3;
        let j = judged(&uncounted, &baseline);
        // The latencies still decide; the answer says what it lacks.
        assert_eq!(j.comparison.verdict, Verdict::Unchanged);
        assert_eq!(j.codes, vec!["latency", "failure_data_missing"]);
        assert_eq!(j.failure, None);
        let last = j.comparison.reasons.last().expect("a reason");
        assert!(last.contains("candidate 1.1.0"), "{last}");
        assert!(last.contains("3 run(s)"), "{last}");
        // Counted in part: compared on what was counted, and said so.
        let mut partly = counted(side_with("1.1.0", steady(20.0), WHOLE), 4000, 1600);
        partly.requests.runs_without_counts = 1;
        let j = judged(&partly, &baseline);
        assert_eq!(j.codes, vec!["failure_share", "failure_data_missing"]);
    }

    #[test]
    fn a_verdict_that_rests_on_a_censored_quantile_is_not_given() {
        let censored_at = |side: Side, floor: f64, windows: u64| Side {
            censored_floor: Some(floor),
            windows: Windows {
                censored: windows,
                ..WHOLE
            },
            ..side
        };
        let baseline = side_with("1.0.0", steady(100.0), WHOLE);
        // The candidate's p95 is a request given up on at 50 ms: at least
        // 50 ms, which is not known to be less than 100.
        let mut fast: Vec<f64> = steady(20.0);
        for value in fast.iter_mut().step_by(5) {
            *value = 50.0;
        }
        let candidate = censored_at(side_with("1.1.0", fast, WHOLE), 50.0, 8);
        let j = judged(&candidate, &baseline);
        assert_eq!(j.comparison.verdict, Verdict::Inconclusive);
        assert_eq!(j.codes[0], "censored");
        let reason = &j.comparison.reasons[0];
        assert!(reason.contains("candidate 1.1.0"), "{reason}");
        assert!(reason.contains("8 of 40"), "{reason}");
        assert!(reason.contains("at least 50"), "{reason}");
        // "At least 50 ms" as the yardstick: slower than it is not known
        // either.
        let j = judged(&baseline, &candidate);
        assert_eq!(j.comparison.verdict, Verdict::Inconclusive);
        assert_eq!(j.codes[0], "censored");
        assert!(j.comparison.reasons[0].contains("baseline 1.1.0"));
        // A candidate at least as slow as its timeout and slower than the
        // baseline all the same is regressed: the truth is no better.
        let quick = side_with("0.9.0", steady(10.0), WHOLE);
        let j = judged(&candidate, &quick);
        assert_eq!(j.comparison.verdict, Verdict::Regressed);
        assert_eq!(j.codes[0], "latency");
        // Censored windows below the quantile do not touch it.
        let below = censored_at(side_with("1.1.0", steady(20.0), WHOLE), 1000.0, 1);
        let j = judged(&below, &baseline);
        assert_eq!(j.comparison.verdict, Verdict::Improved);
        assert_eq!(j.codes[0], "latency");
    }

    #[test]
    fn a_default_pair_chosen_while_the_history_is_read_back_is_inconclusive() {
        let opts = CompareOptions::default();
        let baseline = side_with("1.0.0", steady(100.0), WHOLE);
        let slower = side_with("1.1.0", steady(150.0), WHOLE);
        let j = judge("checkout", &slower, Some(&baseline), 2, &opts, true);
        assert_eq!(j.comparison.verdict, Verdict::Inconclusive);
        assert_eq!(j.codes[0], "history_incomplete");
        assert!(j.comparison.reasons[0].contains("baseline= and candidate="));
        let j = judge("checkout", &slower, Some(&baseline), 2, &opts, false);
        assert_eq!(j.comparison.verdict, Verdict::Regressed);
    }

    #[test]
    fn a_version_in_a_request_is_held_to_the_bounds() {
        let named = |value: &str| named_version("candidate", Some(value));
        assert_eq!(named("1.5.0").ok(), Some(Some("1.5.0".to_string())));
        let most = named(&"9".repeat(128)).ok().flatten();
        assert_eq!(most.map(|v| v.len()), Some(128));
        assert!(named(&"9".repeat(129)).is_err());
        assert!(named("1.5.0\n").is_err());
        assert!(named("1.5.0\u{1b}[31m").is_err());
        assert!(named(" ").is_err());
        assert_eq!(named_version("candidate", None).ok(), Some(None));
    }

    #[test]
    fn a_version_window_is_at_most_a_day_wide() {
        let day = 86_400 * NS_PER_S;
        assert!(version_window(Some(0), Some(day)).is_ok());
        assert_eq!(version_window(Some(5), Some(5)).ok(), Some((5, 5)));
        let wide = version_window(Some(0), Some(day + 1)).expect_err("a day and a nanosecond");
        assert!(wide.contains("24 h"), "{wide}");
        // The widest pair of instants there is: refused, not wrapped.
        assert!(version_window(Some(i64::MIN), Some(i64::MAX)).is_err());
        assert!(version_window(Some(2), Some(1)).is_err());
        assert!(version_window(None, Some(1)).is_err());
        assert!(version_window(Some(1), None).is_err());
    }

    #[test]
    fn the_selection_rule_names_the_pair_and_how_it_was_chosen() {
        let seen = |version: &str, redeployed: bool| select::VersionSeen {
            version: version.into(),
            first_seen_ns: 0,
            last_seen_ns: 0,
            last_deployed_ns: 0,
            redeployed,
        };
        let env = Some("demo".to_string());
        let (new, old) = (seen("1.5.0", false), seen("1.4.0", false));
        let plain = selection_rule(&env, false, &new, Some(&old));
        assert!(plain.contains("1.5.0 is the version deployed latest in environment demo"));
        assert!(
            plain.contains("1.4.0 the one deployed before it"),
            "{plain}"
        );
        assert!(!plain.contains("again"), "{plain}");
        let back = selection_rule(&env, false, &seen("1.4.0", true), Some(&new));
        assert!(back.contains("deployed again"), "{back}");
        let named = selection_rule(&env, true, &seen("1.4.0", true), Some(&new));
        assert!(named.contains("the request named"), "{named}");
        let alone = selection_rule(&None, false, &old, None);
        assert!(
            alone.contains("(none)") && alone.contains("no version"),
            "{alone}"
        );
    }

    #[test]
    fn a_candidate_without_a_baseline_is_inconclusive_and_says_which_kind() {
        let opts = CompareOptions::default();
        let candidate = side_with("1.0.0", steady(100.0), Windows::default());
        let alone = judge("checkout", &candidate, None, 1, &opts, false);
        assert_eq!(alone.comparison.verdict, Verdict::Inconclusive);
        assert_eq!(alone.codes[0], "no_baseline");
        assert!(alone.comparison.reasons[0].contains("only one service.version (1.0.0)"));
        let oldest = judge("checkout", &candidate, None, 3, &opts, false);
        assert_eq!(oldest.comparison.verdict, Verdict::Inconclusive);
        assert!(oldest.comparison.reasons[0].contains("deployed before 1.0.0"));
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
        let j = judge("checkout", &candidate, Some(&baseline), 2, &opts, false);
        let mut a = Answer {
            target: "checkout".into(),
            probe: DEFAULT_PROBE.into(),
            environment: Some("demo".into()),
            environments_seen: vec![Some("demo".into())],
            range: DEFAULT_RANGE.into(),
            since_ns: 0,
            as_of_ns: 7,
            explicit: false,
            selection_rule: "the rule".into(),
            backfill_running: false,
            include_partial: false,
            min_samples: opts.min_samples,
            min_runs: opts.min_runs,
            versions: Vec::new(),
            comparison: j.comparison,
            reason_codes: j.codes,
            failure: j.failure,
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
        assert_eq!(body["selection"], "latest_deployment");
        assert_eq!(body["selection_rule"], "the rule");
        assert_eq!(body["backfill"], "done");
        assert_eq!(body["reason"], "latency");
        assert_eq!(
            body["reason_codes"],
            json!(["latency", "failure_data_missing"])
        );
        assert_eq!(body["comparison"]["failure_share"], Value::Null);
        assert_eq!(body["comparison"]["min_runs"], 3);
        assert_eq!(body["candidate"]["windows"]["lost"], 0);
        assert_eq!(body["candidate"]["windows"]["censored"], 0);
        assert_eq!(body["candidate"]["attribution"]["declared"], 1);
        assert_eq!(
            body["candidate"]["windows"]["unanswered_share"],
            Value::Null
        );
        a.environment = None;
        a.explicit = true;
        assert_eq!(a.row(&baseline).envs, None);
        a.backfill_running = true;
        let body = a.json(false, false);
        assert_eq!(body["selection"], "explicit");
        assert_eq!(body["backfill"], "running");
    }
}
