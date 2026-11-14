//! `tumult perf <target>` — ask the daemon for a target's performance
//! regression verdict (`GET /api/perf/{target}`) and turn it into an exit
//! code a pipeline can act on, like `tumult gate`. With `--store` the
//! verdict is also appended to the daemon's history
//! (`POST /api/perf/{target}/verdicts`), and a verdict the daemon did not
//! store is an error.
//!
//! Exit codes: 0 the verdict passes; 1 an error (no daemon, a refusal, a
//! verdict that was not stored); 2 a usage error; 3 `regressed`; 4
//! `inconclusive`. By default both `regressed` and `inconclusive` fail:
//! a release is not passed on evidence that cannot tell. A pair the
//! daemon cannot put in deployment order (its 400 with
//! `reason: unorderable`) is inconclusive too: a gate names what it
//! tests with `--candidate`.

// Writing to a `String` cannot fail, so the results of `writeln!` are dropped.
use std::fmt::Write as _;

use anyhow::{bail, Context, Result};
use serde_json::Value;

/// Env var naming the daemon base URL (shared with `tumult gate`).
const DAEMON_URL_ENV: &str = "TUMULT_DAEMON_URL";
/// Env var with the daemon API token (`kro_...`).
const DAEMON_TOKEN_ENV: &str = "TUMULT_DAEMON_TOKEN";

/// The exit code of a `regressed` verdict that fails the gate.
pub const EXIT_REGRESSED: i32 = 3;
/// The exit code of an `inconclusive` verdict that fails the gate.
pub const EXIT_INCONCLUSIVE: i32 = 4;

/// Which verdicts make the command exit non-zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PerfFailOn {
    /// Exit non-zero on `regressed` only.
    Regressed,
    /// Exit non-zero on `regressed` or `inconclusive` (default).
    Inconclusive,
    /// Always exit zero; print the verdict only.
    Never,
}

impl PerfFailOn {
    /// The policy for the verdicts `--fail-on` lists: the strictest of
    /// them. `never` stands alone.
    ///
    /// # Errors
    ///
    /// Returns an error when nothing is listed or `never` is listed with
    /// another.
    pub fn of(listed: &[Self]) -> Result<Self, String> {
        if listed.is_empty() {
            return Err("--fail-on must name regressed, inconclusive or never".into());
        }
        if listed.contains(&Self::Never) {
            if listed.iter().any(|one| *one != Self::Never) {
                return Err("--fail-on never cannot be combined with another verdict".into());
            }
            return Ok(Self::Never);
        }
        if listed.contains(&Self::Inconclusive) {
            return Ok(Self::Inconclusive);
        }
        Ok(Self::Regressed)
    }
}

/// The gate did not pass: the verdict is one `--fail-on` fails on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerfDidNotPass {
    pub target: String,
    pub verdict: String,
    /// What the process exits with.
    pub code: i32,
}

impl std::fmt::Display for PerfDidNotPass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "performance verdict for target {} is {}",
            self.target, self.verdict
        )
    }
}

impl std::error::Error for PerfDidNotPass {}

/// The exit code an error asks for, when it is a gate that did not pass.
#[must_use]
pub fn exit_code_of(error: &anyhow::Error) -> Option<i32> {
    error.downcast_ref::<PerfDidNotPass>().map(|e| e.code)
}

/// Arguments of `tumult perf`.
#[derive(Debug, Clone)]
pub struct PerfArgs {
    pub target: String,
    pub probe: Option<String>,
    pub quantile: Option<String>,
    pub min_samples: Option<u32>,
    /// Fewer runs with samples on either side is inconclusive.
    pub min_runs: Option<u32>,
    /// Seed of the resampling, for an interval that can be reproduced.
    pub seed: Option<u64>,
    pub margin_pct: Option<f64>,
    pub env: Option<String>,
    /// The version to compare against, instead of the one deployed just
    /// before the candidate.
    pub baseline: Option<String>,
    /// The version to judge, instead of the one deployed last.
    pub candidate: Option<String>,
    /// How far back samples are read (`30d`, `12h`); the daemon's default
    /// when not given.
    pub range: Option<String>,
    /// Count the samples of aborted and truncated runs too.
    pub include_partial: bool,
    /// Have the daemon append the verdict to `perf_verdicts` (operator
    /// role); without it the command only reads. The command fails when
    /// the verdict was not stored.
    pub store: bool,
    pub daemon_url: Option<String>,
    /// The policy `--fail-on` named; `None` when it was left out, which
    /// keeps the default policy without the caller having asked for a gate.
    pub fail_on: Option<PerfFailOn>,
    pub json: bool,
}

impl PerfArgs {
    /// The policy the exit code follows: the one named, else the default.
    #[must_use]
    pub fn policy(&self) -> PerfFailOn {
        self.fail_on.unwrap_or(PerfFailOn::Inconclusive)
    }
}

/// One side of the daemon's answer, only what the CLI prints.
#[derive(Debug, Clone, Default)]
pub struct PerfSide {
    pub version: String,
    pub samples: u64,
    pub quantile_value: Option<f64>,
    /// Runs whose samples are in the comparison.
    pub runs: u64,
    /// Runs whose version was read from the target's spans because the
    /// run recorded none.
    pub inferred: u64,
    /// Aborted or truncated runs left out of the samples.
    pub excluded: u64,
    /// Load windows planned, and how many of them nothing answered in.
    pub planned: u64,
    pub unanswered: u64,
    /// Windows that hold a request given up on at its timeout.
    pub censored: u64,
    /// Requests sent, and how many of them were not answered well.
    pub sent: u64,
    pub failed: u64,
}

/// The daemon's answer (`GET /api/perf/{target}`); unknown fields are
/// ignored so the daemon can grow the payload.
#[derive(Debug, Clone)]
pub struct PerfResponse {
    pub target: String,
    pub probe: String,
    /// The one environment the answer is for; `None` for telemetry that
    /// names none.
    pub environment: Option<String>,
    /// `explicit` or `latest_deployment`.
    pub selection: Option<String>,
    /// Which rule chose the pair, in the daemon's words.
    pub selection_rule: Option<String>,
    /// Runs no version could be given.
    pub unattributed: u64,
    pub verdict: String,
    pub reasons: Vec<String>,
    pub quantile: String,
    pub baseline: Option<PerfSide>,
    pub candidate: PerfSide,
    pub delta: Option<f64>,
    pub delta_pct: Option<f64>,
    pub ci: Option<(f64, f64)>,
    pub margin: Option<f64>,
    /// The interval rests on fewer than five runs on a side.
    pub low_run_count: bool,
    pub stored: bool,
    /// The daemon already held this verdict and appended nothing.
    pub duplicate: bool,
}

/// A count at a JSON pointer; absent (an older daemon) is zero.
fn count(v: &Value, pointer: &str) -> u64 {
    v.pointer(pointer).and_then(Value::as_u64).unwrap_or(0)
}

fn side(v: &Value, quantile: &str) -> Option<PerfSide> {
    let version = v.get("version")?.as_str()?.to_owned();
    Some(PerfSide {
        version,
        samples: v.get("samples").and_then(Value::as_u64).unwrap_or_default(),
        quantile_value: v.get(quantile).and_then(Value::as_f64),
        runs: v
            .get("runs")
            .and_then(Value::as_array)
            .map_or(0, |runs| runs.len() as u64),
        inferred: count(v, "/attribution/inferred"),
        excluded: count(v, "/partial/excluded"),
        planned: count(v, "/windows/planned"),
        unanswered: count(v, "/windows/unanswered"),
        censored: count(v, "/windows/censored"),
        sent: count(v, "/requests/sent"),
        failed: count(v, "/requests/failed"),
    })
}

impl PerfResponse {
    /// Read the fields the CLI needs out of the daemon's JSON.
    ///
    /// # Errors
    ///
    /// Returns an error when a required field is missing or mistyped.
    pub fn from_value(v: &Value) -> Result<Self> {
        let str_field = |k: &str| -> Result<String> {
            v.get(k)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .with_context(|| format!("perf answer is missing `{k}`"))
        };
        let comparison = v.get("comparison").cloned().unwrap_or(Value::Null);
        let quantile = comparison
            .get("quantile")
            .and_then(Value::as_str)
            .unwrap_or("p95")
            .to_owned();
        let candidate = v
            .get("candidate")
            .and_then(|c| side(c, &quantile))
            .context("perf answer is missing `candidate`")?;
        let num = |k: &str| comparison.get(k).and_then(Value::as_f64);
        Ok(Self {
            target: str_field("target")?,
            probe: str_field("probe")?,
            environment: v
                .get("environment")
                .and_then(Value::as_str)
                .map(str::to_owned),
            selection: v
                .get("selection")
                .and_then(Value::as_str)
                .map(str::to_owned),
            selection_rule: v
                .get("selection_rule")
                .and_then(Value::as_str)
                .map(str::to_owned),
            unattributed: count(v, "/unattributed_runs/count"),
            verdict: str_field("verdict")?,
            reasons: v
                .get("reasons")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            baseline: v.get("baseline").and_then(|b| side(b, &quantile)),
            candidate,
            delta: num("delta"),
            delta_pct: num("delta_pct"),
            ci: match (num("ci_low"), num("ci_high")) {
                (Some(low), Some(high)) => Some((low, high)),
                _ => None,
            },
            margin: num("margin"),
            low_run_count: comparison
                .get("low_run_count")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            stored: v.get("stored").and_then(Value::as_bool).unwrap_or(false),
            duplicate: v.get("duplicate").and_then(Value::as_bool).unwrap_or(false),
            quantile,
        })
    }
}

/// What the command exits with for a verdict under the `fail_on` policy:
/// `None` when it passes. A verdict this build does not know never
/// passes. Pure.
#[must_use]
pub fn exit_code(verdict: &str, fail_on: PerfFailOn) -> Option<i32> {
    if fail_on == PerfFailOn::Never {
        return None;
    }
    match verdict {
        "regressed" => Some(EXIT_REGRESSED),
        "inconclusive" if fail_on == PerfFailOn::Inconclusive => Some(EXIT_INCONCLUSIVE),
        "improved" | "unchanged" | "inconclusive" => None,
        _ => Some(1),
    }
}

fn fmt_opt(v: Option<f64>) -> String {
    v.map_or_else(|| "n/a".to_string(), |v| format!("{v:.1}"))
}

/// How the two versions were chosen and where; nothing for a daemon that
/// does not say.
fn selection_line(resp: &PerfResponse) -> Option<String> {
    if let Some(rule) = &resp.selection_rule {
        return Some(format!("  versions: {rule}\n"));
    }
    let how = match resp.selection.as_deref()? {
        "explicit" => "named on the request",
        _ => "the two deployed last",
    };
    let place = resp.environment.as_deref().map_or_else(
        || "telemetry without an environment".to_string(),
        |env| format!("environment {env}"),
    );
    Some(format!("  versions: {how}, in {place}\n"))
}

/// One side: its quantile, then what weakens its evidence, if anything.
fn side_lines(role: &str, side: &PerfSide) -> String {
    let mut out = format!(
        "  {role:<9} {}: {} ms over {} sample(s) in {} run(s)\n",
        side.version,
        fmt_opt(side.quantile_value),
        side.samples,
        side.runs
    );
    let mut runs = Vec::new();
    if side.inferred > 0 {
        runs.push(format!("{} run(s) attributed from spans", side.inferred));
    }
    if side.excluded > 0 {
        runs.push(format!("{} partial run(s) left out", side.excluded));
    }
    if !runs.is_empty() {
        let _ = writeln!(out, "      {}", runs.join("; "));
    }
    if side.unanswered > 0 {
        let _ = writeln!(
            out,
            "      {} of {} load window(s) went unanswered",
            side.unanswered, side.planned
        );
    }
    if side.failed > 0 {
        #[allow(clippy::cast_precision_loss)]
        let share = side.failed as f64 / side.sent.max(1) as f64 * 100.0;
        let _ = writeln!(
            out,
            "      {} of {} request(s) failed ({share:.1}%)",
            side.failed, side.sent
        );
    }
    if side.censored > 0 {
        let _ = writeln!(
            out,
            "      {} load window(s) hold a request given up on at its timeout",
            side.censored
        );
    }
    out
}

/// What a gate is told when no `--candidate` names the build under test:
/// the daemon then judges whichever version was deployed last, which is
/// not the build a pipeline means once another has been deployed since.
/// Nothing when `--fail-on` was not given, the candidate is named or no
/// verdict fails the command.
#[must_use]
pub fn gate_warning(args: &PerfArgs) -> Option<String> {
    if matches!(args.fail_on, None | Some(PerfFailOn::Never)) || args.candidate.is_some() {
        return None;
    }
    Some(format!(
        "warning: perf {} gates on the version deployed latest, whichever that is; \
         pass --candidate to gate on the build under test",
        args.target
    ))
}

/// Human-readable rendering of the perf answer.
#[must_use]
pub fn render(resp: &PerfResponse) -> String {
    let few_runs = if resp.low_run_count {
        "; fewer than 5 runs on a side"
    } else {
        ""
    };
    let mut out = format!(
        "perf {}: {} ({} of {}{few_runs})\n",
        resp.target,
        resp.verdict.to_uppercase(),
        resp.quantile,
        resp.probe
    );
    out.push_str(&selection_line(resp).unwrap_or_default());
    if let Some(b) = &resp.baseline {
        out.push_str(&side_lines("baseline", b));
    }
    out.push_str(&side_lines("candidate", &resp.candidate));
    if let Some(delta) = resp.delta {
        let pct = resp
            .delta_pct
            .map(|p| format!(" ({p:+.1}%)"))
            .unwrap_or_default();
        let ci = resp
            .ci
            .map(|(lo, hi)| format!(", CI [{lo:.1}, {hi:.1}]"))
            .unwrap_or_default();
        let margin = resp
            .margin
            .map(|m| format!(", margin ±{m:.1}"))
            .unwrap_or_default();
        let _ = writeln!(out, "  delta {delta:+.1} ms{pct}{ci}{margin}");
    }
    for reason in &resp.reasons {
        let _ = writeln!(out, "  - {reason}");
    }
    if resp.unattributed > 0 {
        let _ = writeln!(
            out,
            "  {} run(s) could not be attributed to a version",
            resp.unattributed
        );
    }
    if resp.stored && resp.duplicate {
        out.push_str(
            "  verdict stored in perf_verdicts (already there with the same evidence; nothing appended)\n",
        );
    } else if resp.stored {
        out.push_str("  verdict stored in perf_verdicts\n");
    }
    out
}

/// The request for the daemon: a read of the verdict, or with `--store`
/// the POST that also appends it. Pure.
#[must_use]
pub fn endpoint(base: &str, args: &PerfArgs) -> (reqwest::Method, String) {
    let read = format!(
        "{}/api/perf/{}",
        base.trim_end_matches('/'),
        super::gate::encode_segment(&args.target)
    );
    if args.store {
        return (reqwest::Method::POST, format!("{read}/verdicts"));
    }
    (reqwest::Method::GET, read)
}

/// `--store` asked for a stored verdict: anything else is a failure, never
/// a quiet read.
///
/// # Errors
///
/// Returns an error when `--store` was given and the daemon's answer does
/// not say the verdict was stored.
pub fn ensure_stored(args: &PerfArgs, resp: &PerfResponse) -> Result<()> {
    if args.store && !resp.stored {
        bail!(
            "the daemon did not store the {} verdict for {}",
            resp.verdict,
            resp.target
        );
    }
    Ok(())
}

/// The query string for the daemon: only what the caller set, so the
/// daemon's defaults apply to the rest. Pure.
#[must_use]
pub fn query_params(args: &PerfArgs) -> Vec<(&'static str, String)> {
    let mut query = Vec::new();
    if let Some(p) = &args.probe {
        query.push(("probe", p.clone()));
    }
    if let Some(q) = &args.quantile {
        query.push(("quantile", q.clone()));
    }
    if let Some(m) = args.min_samples {
        query.push(("min_samples", m.to_string()));
    }
    if let Some(m) = args.min_runs {
        query.push(("min_runs", m.to_string()));
    }
    if let Some(seed) = args.seed {
        query.push(("seed", seed.to_string()));
    }
    if let Some(m) = args.margin_pct {
        query.push(("margin_pct", m.to_string()));
    }
    if let Some(env) = &args.env {
        query.push(("env", env.clone()));
    }
    if let Some(baseline) = &args.baseline {
        query.push(("baseline", baseline.clone()));
    }
    if let Some(candidate) = &args.candidate {
        query.push(("candidate", candidate.clone()));
    }
    if let Some(range) = &args.range {
        query.push(("range", range.clone()));
    }
    if args.include_partial {
        query.push(("include", "partial".to_string()));
    }
    query
}

/// The daemon's reason when it refused because it cannot tell which of
/// two versions was deployed latest (HTTP 400, `reason: unorderable`).
/// Pure.
#[must_use]
pub fn unorderable(status: u16, body: &Value) -> Option<String> {
    let reason = body.get("reason").and_then(Value::as_str);
    if status != 400 || reason != Some("unorderable") {
        return None;
    }
    let why = body.get("error").and_then(Value::as_str);
    Some(
        why.unwrap_or("the deployment order cannot be told")
            .to_owned(),
    )
}

/// What is printed for a pair the daemon cannot order.
#[must_use]
pub fn render_unorderable(target: &str, why: &str) -> String {
    format!(
        "perf {target}: INCONCLUSIVE (which version was deployed latest cannot be told)\n  \
         - {why}\n  name the version under test with --candidate\n"
    )
}

/// A gate that could not be judged: inconclusive under `fail_on`.
fn inconclusive(target: &str, fail_on: PerfFailOn) -> Result<()> {
    let Some(code) = exit_code("inconclusive", fail_on) else {
        return Ok(());
    };
    Err(PerfDidNotPass {
        target: target.to_owned(),
        verdict: "inconclusive".to_owned(),
        code,
    }
    .into())
}

/// Query the daemon and print the verdict.
///
/// # Errors
///
/// Returns [`PerfDidNotPass`] when the verdict fails under `fail_on`, and
/// any other error when no daemon URL is configured, the daemon is
/// unreachable, it answers with an error status, or `--store` was given
/// and the verdict was not stored.
pub async fn cmd_perf(args: &PerfArgs) -> Result<()> {
    let base = args
        .daemon_url
        .clone()
        .or_else(|| std::env::var(DAEMON_URL_ENV).ok())
        .filter(|s| !s.is_empty())
        .with_context(|| format!("no daemon URL: pass --daemon-url or set {DAEMON_URL_ENV}"))?;
    if let Some(warning) = gate_warning(args) {
        crate::errln!("{warning}");
    }
    let (method, url) = endpoint(&base, args);
    let query = query_params(args);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("failed to build HTTP client")?;
    let mut request = client.request(method, &url).query(&query);
    if let Ok(token) = std::env::var(DAEMON_TOKEN_ENV) {
        if !token.is_empty() {
            request = request.bearer_auth(token);
        }
    }
    let resp = request
        .send()
        .await
        .with_context(|| format!("perf request to {url} failed"))?;
    let status = resp.status();
    let body: Value = resp
        .json()
        .await
        .with_context(|| format!("perf answered HTTP {status} with an unreadable body"))?;
    if let Some(why) = unorderable(status.as_u16(), &body) {
        if args.json {
            crate::outln!("{}", serde_json::to_string_pretty(&body)?);
        } else {
            crate::out!("{}", render_unorderable(&args.target, &why));
        }
        return inconclusive(&args.target, args.policy());
    }
    if !status.is_success() {
        let msg = body
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        bail!("perf answered HTTP {status}: {msg}");
    }
    let parsed = PerfResponse::from_value(&body)?;
    ensure_stored(args, &parsed)?;
    if args.json {
        crate::outln!("{}", serde_json::to_string_pretty(&body)?);
    } else {
        crate::out!("{}", render(&parsed));
    }
    let Some(code) = exit_code(&parsed.verdict, args.policy()) else {
        return Ok(());
    };
    Err(PerfDidNotPass {
        target: parsed.target,
        verdict: parsed.verdict,
        code,
    }
    .into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_verdict_that_fails_the_gate_has_an_exit_code_of_its_own() {
        for policy in [PerfFailOn::Regressed, PerfFailOn::Inconclusive] {
            assert_eq!(exit_code("improved", policy), None);
            assert_eq!(exit_code("unchanged", policy), None);
            assert_eq!(exit_code("regressed", policy), Some(EXIT_REGRESSED));
            // A verdict this build does not know never passes.
            assert_eq!(exit_code("bogus", policy), Some(1));
        }
        assert_eq!(
            exit_code("inconclusive", PerfFailOn::Inconclusive),
            Some(EXIT_INCONCLUSIVE)
        );
        assert_eq!(exit_code("inconclusive", PerfFailOn::Regressed), None);
        for verdict in ["improved", "regressed", "inconclusive", "bogus"] {
            assert_eq!(exit_code(verdict, PerfFailOn::Never), None);
        }
        // Not the code of an error (1) or of a usage error (2).
        assert_eq!((EXIT_REGRESSED, EXIT_INCONCLUSIVE), (3, 4));
    }

    #[test]
    fn the_strictest_of_the_named_verdicts_is_the_policy() {
        use PerfFailOn::{Inconclusive, Never, Regressed};
        assert_eq!(PerfFailOn::of(&[Regressed, Inconclusive]), Ok(Inconclusive));
        assert_eq!(PerfFailOn::of(&[Inconclusive]), Ok(Inconclusive));
        assert_eq!(PerfFailOn::of(&[Regressed]), Ok(Regressed));
        assert_eq!(PerfFailOn::of(&[Never]), Ok(Never));
        let both = PerfFailOn::of(&[Never, Regressed]).unwrap_err();
        assert!(both.contains("never"), "{both}");
        assert!(PerfFailOn::of(&[]).is_err());
    }

    #[test]
    fn a_failed_gate_is_an_error_that_carries_its_exit_code() {
        let failed: anyhow::Error = PerfDidNotPass {
            target: "checkout".into(),
            verdict: "inconclusive".into(),
            code: EXIT_INCONCLUSIVE,
        }
        .into();
        assert_eq!(exit_code_of(&failed), Some(EXIT_INCONCLUSIVE));
        assert_eq!(
            failed.to_string(),
            "performance verdict for target checkout is inconclusive"
        );
        assert_eq!(exit_code_of(&anyhow::anyhow!("daemon unreachable")), None);
    }

    #[test]
    fn an_order_the_daemon_cannot_tell_is_inconclusive_at_the_gate() {
        let refused = serde_json::json!({
            "error": "version 1.4.0 of checkout reported again after 1.5.0 had taken over",
            "reason": "unorderable"
        });
        let why = unorderable(400, &refused).expect("an unorderable pair");
        assert!(why.contains("1.4.0") && why.contains("1.5.0"), "{why}");
        let text = render_unorderable("checkout", &why);
        assert!(text.starts_with("perf checkout: INCONCLUSIVE"), "{text}");
        assert!(text.contains("  - version 1.4.0 of checkout"), "{text}");
        assert!(text.contains("--candidate"), "{text}");
        // Any other refusal stays an error.
        let other = serde_json::json!({"error": "quantile must be p50, p95 or p99"});
        assert_eq!(unorderable(400, &other), None);
        assert_eq!(unorderable(404, &refused), None);
    }

    fn answer() -> Value {
        serde_json::json!({
            "target": "checkout",
            "probe": "load.latency_p95_ms",
            "verdict": "regressed",
            "reasons": ["p95 difference interval [31.0, 70.0] lies entirely above the margin"],
            "baseline": {"version": "1.4.0", "runs": ["old"], "samples": 40, "p50": 50.0, "p95": 100.0, "p99": 150.0},
            "candidate": {"version": "1.5.0", "runs": ["new"], "samples": 40, "p50": 75.0, "p95": 150.0, "p99": 225.0},
            "comparison": {"quantile": "p95", "delta": 50.0, "delta_pct": 50.0, "ci_low": 31.0, "ci_high": 70.0, "confidence": 0.95, "margin": 5.0},
            "stored": true
        })
    }

    /// Slower beyond doubt, with a few more failed requests than there
    /// is evidence to judge: the daemon answers regressed, and the gate
    /// fails on it.
    #[test]
    fn a_regression_beside_an_unsettled_failure_share_fails_the_gate() {
        let mut body = answer();
        body["reason"] = serde_json::json!("latency");
        body["reason_codes"] = serde_json::json!(["latency"]);
        body["reasons"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!(
                "candidate 1.5.0: 3 of 150 requests failed (2.0%) against 0 of 150 (0.0%) for \
             baseline 1.4.0, but on too little to count as a regression by itself"
            ));
        let parsed = PerfResponse::from_value(&body).unwrap();
        assert_eq!(
            exit_code(&parsed.verdict, PerfFailOn::Regressed),
            Some(EXIT_REGRESSED)
        );
        let text = render(&parsed);
        assert!(text.starts_with("perf checkout: REGRESSED"), "{text}");
        assert!(text.contains("  - candidate 1.5.0: 3 of 150"), "{text}");
    }

    #[test]
    fn render_lists_sides_delta_and_reasons() {
        let resp = PerfResponse::from_value(&answer()).unwrap();
        assert_eq!(resp.ci, Some((31.0, 70.0)));
        assert_eq!(resp.baseline.as_ref().unwrap().quantile_value, Some(100.0));
        let text = render(&resp);
        assert!(text.contains("perf checkout: REGRESSED (p95 of load.latency_p95_ms)"));
        assert!(text.contains("baseline  1.4.0: 100.0 ms over 40 sample(s)"));
        assert!(text.contains("candidate 1.5.0: 150.0 ms over 40 sample(s)"));
        assert!(text.contains("delta +50.0 ms (+50.0%), CI [31.0, 70.0], margin ±5.0"));
        assert!(text.contains("- p95 difference"));
        assert!(text.contains("verdict stored"));
    }

    #[test]
    fn a_verdict_the_daemon_already_held_counts_as_stored_and_says_so() {
        let mut v = answer();
        v["duplicate"] = true.into();
        let resp = PerfResponse::from_value(&v).unwrap();
        assert!(resp.stored && resp.duplicate);
        let asking = PerfArgs {
            store: true,
            ..args()
        };
        assert!(ensure_stored(&asking, &resp).is_ok());
        let text = render(&resp);
        assert!(
            text.contains("verdict stored in perf_verdicts (already there"),
            "{text}"
        );
        assert!(text.contains("nothing appended"), "{text}");
        let first = PerfResponse::from_value(&answer()).unwrap();
        assert!(!first.duplicate);
        assert!(!render(&first).contains("already there"));
    }

    #[test]
    fn render_without_a_baseline_is_candidate_only() {
        let mut v = answer();
        v["baseline"] = Value::Null;
        v["verdict"] = "inconclusive".into();
        v["comparison"]["delta"] = Value::Null;
        v["stored"] = false.into();
        let resp = PerfResponse::from_value(&v).unwrap();
        assert!(resp.baseline.is_none());
        let text = render(&resp);
        assert!(text.contains("INCONCLUSIVE"));
        assert!(!text.contains("baseline"));
        assert!(!text.contains("delta"));
        assert!(!text.contains("stored"));
    }

    #[test]
    fn from_value_requires_the_candidate() {
        let mut v = answer();
        v["candidate"] = Value::Null;
        assert!(PerfResponse::from_value(&v).is_err());
    }

    fn args() -> PerfArgs {
        PerfArgs {
            target: "checkout".into(),
            probe: None,
            quantile: None,
            min_samples: None,
            min_runs: None,
            seed: None,
            margin_pct: None,
            env: None,
            baseline: None,
            candidate: None,
            range: None,
            include_partial: false,
            store: false,
            daemon_url: None,
            fail_on: None,
            json: false,
        }
    }

    #[test]
    fn the_runs_a_side_needs_and_the_seed_go_to_the_daemon_when_asked_for() {
        let asked = PerfArgs {
            min_runs: Some(5),
            seed: Some(42),
            ..args()
        };
        assert_eq!(
            query_params(&asked),
            vec![("min_runs", "5".to_string()), ("seed", "42".to_string())]
        );
    }

    #[test]
    fn render_says_when_the_interval_rests_on_few_runs() {
        let mut v = answer();
        v["comparison"]["low_run_count"] = true.into();
        let resp = PerfResponse::from_value(&v).unwrap();
        assert!(resp.low_run_count);
        let text = render(&resp);
        // In the headline, where it cannot be missed.
        assert!(
            text.starts_with(
                "perf checkout: REGRESSED (p95 of load.latency_p95_ms; fewer than 5 runs on a side)\n"
            ),
            "{text}"
        );
        // An older daemon does not say; nothing is printed.
        let plain = PerfResponse::from_value(&answer()).unwrap();
        assert!(!plain.low_run_count);
        assert!(!render(&plain).contains("fewer than 5 runs"));
    }

    #[test]
    fn a_gate_asked_for_without_a_named_candidate_is_warned_of() {
        let gate = PerfArgs {
            fail_on: Some(PerfFailOn::Regressed),
            ..args()
        };
        let warning = gate_warning(&gate).expect("a warning");
        assert!(warning.contains("--candidate"), "{warning}");
        assert!(warning.contains("deployed latest"), "{warning}");
        // The build under test is named: nothing to warn of.
        let named = PerfArgs {
            candidate: Some("1.5.0".into()),
            ..gate.clone()
        };
        assert_eq!(gate_warning(&named), None);
        // No verdict fails the command: it is a read, not a gate.
        let read = PerfArgs {
            fail_on: Some(PerfFailOn::Never),
            ..gate
        };
        assert_eq!(gate_warning(&read), None);
    }

    #[test]
    fn a_run_that_did_not_ask_for_a_gate_is_not_warned() {
        // The default policy still fails the command, but nobody said gate.
        let plain = args();
        assert_eq!(plain.policy(), PerfFailOn::Inconclusive);
        assert_eq!(gate_warning(&plain), None);
    }

    #[test]
    fn the_range_and_partial_runs_go_to_the_daemon_when_asked_for() {
        let asked = PerfArgs {
            range: Some("60d".into()),
            include_partial: true,
            ..args()
        };
        assert_eq!(
            query_params(&asked),
            vec![
                ("range", "60d".to_string()),
                ("include", "partial".to_string())
            ]
        );
    }

    #[test]
    fn render_says_which_rule_chose_the_pair_and_what_became_of_the_requests() {
        let mut v = evidence();
        v["selection"] = "latest_deployment".into();
        v["selection_rule"] =
            "1.5.0 is the version deployed latest in environment demo, and 1.4.0 the one \
             deployed before it"
                .into();
        v["candidate"]["requests"] = serde_json::json!(
            {"sent": 12000, "failed": 4800, "failure_share": 0.4, "runs_without_counts": 0});
        v["candidate"]["windows"]["censored"] = 12.into();
        v["baseline"]["requests"] = serde_json::json!(
            {"sent": 12000, "failed": 0, "failure_share": 0.0, "runs_without_counts": 0});
        let resp = PerfResponse::from_value(&v).unwrap();
        assert_eq!((resp.candidate.sent, resp.candidate.failed), (12000, 4800));
        let text = render(&resp);
        assert!(
            text.contains("  versions: 1.5.0 is the version deployed latest in environment demo"),
            "{text}"
        );
        assert!(!text.contains("the two deployed last"), "{text}");
        assert!(
            text.contains("      4800 of 12000 request(s) failed (40.0%)\n"),
            "{text}"
        );
        assert!(
            text.contains("      12 load window(s) hold a request given up on at its timeout\n"),
            "{text}"
        );
        assert_eq!(text.matches("request(s) failed").count(), 1, "{text}");
    }

    #[test]
    fn store_is_a_post_to_the_verdicts_route_and_never_a_query_flag() {
        assert!(query_params(&args()).is_empty());
        let (method, url) = endpoint("http://d:1/", &args());
        assert_eq!(method, reqwest::Method::GET);
        assert_eq!(url, "http://d:1/api/perf/checkout");

        let stored = PerfArgs {
            store: true,
            env: Some("demo".into()),
            quantile: Some("p99".into()),
            ..args()
        };
        let (method, url) = endpoint("http://d:1", &stored);
        assert_eq!(method, reqwest::Method::POST);
        assert_eq!(url, "http://d:1/api/perf/checkout/verdicts");
        assert_eq!(
            query_params(&stored),
            vec![("quantile", "p99".to_string()), ("env", "demo".to_string())]
        );
    }

    #[test]
    fn named_versions_go_to_the_daemon_as_baseline_and_candidate() {
        let named = PerfArgs {
            baseline: Some("1.3.0".into()),
            candidate: Some("1.5.0".into()),
            ..args()
        };
        assert_eq!(
            query_params(&named),
            vec![
                ("baseline", "1.3.0".to_string()),
                ("candidate", "1.5.0".to_string())
            ]
        );
        let one = PerfArgs {
            candidate: Some("1.5.0".into()),
            ..args()
        };
        assert_eq!(query_params(&one), vec![("candidate", "1.5.0".to_string())]);
    }

    fn evidence() -> Value {
        let mut v = answer();
        v["environment"] = "demo".into();
        v["selection"] = "deployment_order".into();
        v["baseline"]["attribution"] =
            serde_json::json!({"declared": 0, "observed": 1, "inferred": 2});
        v["baseline"]["runs"] = serde_json::json!(["a", "b", "c"]);
        v["baseline"]["partial"] = serde_json::json!(
            {"truncated": 1, "aborted": 1, "halted": 0, "deviated": 1, "excluded": 2});
        v["candidate"]["windows"] = serde_json::json!(
            {"planned": 24, "completed": 24, "unanswered": 3, "unanswered_share": 0.125});
        v["unattributed_runs"] = serde_json::json!({"count": 4, "reasons": {}, "runs": []});
        v
    }

    #[test]
    fn render_says_how_the_versions_were_chosen_and_how_good_the_evidence_is() {
        let resp = PerfResponse::from_value(&evidence()).unwrap();
        assert_eq!(resp.environment.as_deref(), Some("demo"));
        assert_eq!(resp.unattributed, 4);
        let baseline = resp.baseline.as_ref().unwrap();
        assert_eq!(
            (baseline.runs, baseline.inferred, baseline.excluded),
            (3, 2, 2)
        );
        assert_eq!((resp.candidate.planned, resp.candidate.unanswered), (24, 3));
        let text = render(&resp);
        assert!(
            text.contains("  versions: the two deployed last, in environment demo\n"),
            "{text}"
        );
        assert!(
            text.contains("baseline  1.4.0: 100.0 ms over 40 sample(s) in 3 run(s)\n"),
            "{text}"
        );
        assert!(
            text.contains("      2 run(s) attributed from spans; 2 partial run(s) left out\n"),
            "{text}"
        );
        assert!(
            text.contains("      3 of 24 load window(s) went unanswered\n"),
            "{text}"
        );
        assert!(
            text.contains("  4 run(s) could not be attributed to a version\n"),
            "{text}"
        );

        let mut named = evidence();
        named["selection"] = "explicit".into();
        named["environment"] = Value::Null;
        let text = render(&PerfResponse::from_value(&named).unwrap());
        assert!(
            text.contains(
                "  versions: named on the request, in telemetry without an environment\n"
            ),
            "{text}"
        );
    }

    #[test]
    fn render_adds_no_notes_to_clean_evidence_or_to_an_older_daemon() {
        let text = render(&PerfResponse::from_value(&answer()).unwrap());
        assert!(!text.contains("versions:"), "{text}");
        assert!(!text.contains("attributed"), "{text}");
        assert!(!text.contains("unanswered"), "{text}");
        assert!(!text.contains("left out"), "{text}");
        assert!(
            text.contains("baseline  1.4.0: 100.0 ms over 40 sample(s) in 1 run(s)\n"),
            "{text}"
        );
    }

    #[test]
    fn store_fails_when_the_daemon_did_not_store() {
        let stored = PerfResponse::from_value(&answer()).unwrap();
        let mut v = answer();
        v["stored"] = false.into();
        let unstored = PerfResponse::from_value(&v).unwrap();
        let asking = PerfArgs {
            store: true,
            ..args()
        };
        assert!(ensure_stored(&asking, &stored).is_ok());
        let err = ensure_stored(&asking, &unstored).unwrap_err();
        assert!(err.to_string().contains("did not store"), "{err}");
        // A plain read never expects a stored verdict.
        assert!(ensure_stored(&args(), &unstored).is_ok());
    }

    #[tokio::test]
    async fn cmd_perf_requires_a_daemon_url() {
        // An explicit empty URL is rejected before the environment is
        // consulted, so the test does not touch process-wide state.
        let args = PerfArgs {
            daemon_url: Some(String::new()),
            ..args()
        };
        let err = cmd_perf(&args).await.unwrap_err();
        assert!(err.to_string().contains("no daemon URL"), "{err}");
    }
}
