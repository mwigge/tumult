//! `tumult gate <target>` — ask the daemon's CI gate and turn its verdict
//! into an exit code a pipeline can act on.

use std::fmt::Write as _;

use anyhow::{bail, Context, Result};
use serde_json::Value;

/// Env var naming the daemon base URL (shared with auto-ingest).
const DAEMON_URL_ENV: &str = "TUMULT_DAEMON_URL";
/// Env var with the daemon API token (`kro_...`).
const DAEMON_TOKEN_ENV: &str = "TUMULT_DAEMON_TOKEN";

/// Which verdicts make the command exit non-zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailOn {
    /// Exit non-zero on `fail` only (default).
    Fail,
    /// Exit non-zero on `warn` or `fail`.
    Warn,
    /// Always exit zero; print the verdict only.
    Never,
}

/// Arguments of `tumult gate`.
#[derive(Debug, Clone)]
pub struct GateArgs {
    pub target: String,
    pub min_score: Option<f64>,
    pub max_age_days: Option<u32>,
    pub env: Option<String>,
    pub daemon_url: Option<String>,
    pub fail_on: FailOn,
    pub json: bool,
}

/// The daemon's answer (`GET /api/gate/{target}`), only the fields the CLI
/// prints; unknown fields are ignored so the daemon can grow the payload.
#[derive(Debug, Clone)]
pub struct GateResponse {
    pub target: String,
    pub score: f64,
    pub band: String,
    pub verdict: String,
    pub reasons: Vec<String>,
    pub evidence: GateEvidence,
}

#[derive(Debug, Clone, Default)]
pub struct GateEvidence {
    pub runs: u64,
    pub last_run_id: Option<String>,
    pub last_run_at_ns: Option<i64>,
}

impl GateResponse {
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
                .with_context(|| format!("gate answer is missing `{k}`"))
        };
        let evidence = v.get("evidence").cloned().unwrap_or(Value::Null);
        Ok(Self {
            target: str_field("target")?,
            score: v
                .get("score")
                .and_then(Value::as_f64)
                .context("gate answer is missing `score`")?,
            band: str_field("band")?,
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
            evidence: GateEvidence {
                runs: evidence
                    .get("runs")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
                last_run_id: evidence
                    .get("last_run_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                last_run_at_ns: evidence.get("last_run_at_ns").and_then(Value::as_i64),
            },
        })
    }
}

/// Percent-encode one URL path segment (everything outside the unreserved
/// set), so a target name like `payments api` or `a/b` cannot escape the
/// gate route.
#[must_use]
pub fn encode_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// Whether a verdict passes under the `fail_on` policy. Pure.
#[must_use]
pub fn passes(verdict: &str, fail_on: FailOn) -> bool {
    matches!(
        (verdict, fail_on),
        (_, FailOn::Never) | ("pass", _) | ("warn", FailOn::Fail)
    )
}

/// Human-readable rendering of the gate answer.
#[must_use]
pub fn render(resp: &GateResponse) -> String {
    let mut out = format!(
        "gate {}: {} (score {:.1}, band {}, runs {})\n",
        resp.target,
        resp.verdict.to_uppercase(),
        resp.score,
        resp.band,
        resp.evidence.runs
    );
    if let Some(id) = &resp.evidence.last_run_id {
        let when = resp
            .evidence
            .last_run_at_ns
            .map(|ns| format!(" at {ns} ns"))
            .unwrap_or_default();
        let _ = writeln!(out, "  latest evidence: {id}{when}");
    }
    for reason in &resp.reasons {
        let _ = writeln!(out, "  - {reason}");
    }
    out
}

/// Query the gate and print the verdict. Returns `Ok(true)` when the gate
/// passes under `fail_on`; the caller turns `false` into a non-zero exit.
///
/// # Errors
///
/// Returns an error when no daemon URL is configured, the daemon is
/// unreachable, or it answers with an error status.
pub async fn cmd_gate(args: &GateArgs) -> Result<bool> {
    let base = args
        .daemon_url
        .clone()
        .or_else(|| std::env::var(DAEMON_URL_ENV).ok())
        .filter(|s| !s.is_empty())
        .with_context(|| format!("no daemon URL: pass --daemon-url or set {DAEMON_URL_ENV}"))?;
    let url = format!(
        "{}/api/gate/{}",
        base.trim_end_matches('/'),
        encode_segment(&args.target)
    );
    let mut query: Vec<(&str, String)> = Vec::new();
    if let Some(m) = args.min_score {
        query.push(("min_score", m.to_string()));
    }
    if let Some(d) = args.max_age_days {
        query.push(("max_age_days", d.to_string()));
    }
    if let Some(env) = &args.env {
        query.push(("env", env.clone()));
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .context("failed to build HTTP client")?;
    let mut request = client.get(&url).query(&query);
    if let Ok(token) = std::env::var(DAEMON_TOKEN_ENV) {
        if !token.is_empty() {
            request = request.bearer_auth(token);
        }
    }
    let resp = request
        .send()
        .await
        .with_context(|| format!("gate request to {url} failed"))?;
    let status = resp.status();
    let body: Value = resp
        .json()
        .await
        .with_context(|| format!("gate answered HTTP {status} with an unreadable body"))?;
    if !status.is_success() {
        let msg = body
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        bail!("gate answered HTTP {status}: {msg}");
    }
    let parsed = GateResponse::from_value(&body)?;
    if args.json {
        crate::outln!("{}", serde_json::to_string_pretty(&body)?);
    } else {
        crate::out!("{}", render(&parsed));
    }
    Ok(passes(&parsed.verdict, args.fail_on))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fail_on_policy_maps_verdicts_to_exit_outcomes() {
        assert!(passes("pass", FailOn::Fail));
        assert!(passes("warn", FailOn::Fail));
        assert!(!passes("fail", FailOn::Fail));
        assert!(passes("pass", FailOn::Warn));
        assert!(!passes("warn", FailOn::Warn));
        assert!(!passes("fail", FailOn::Warn));
        assert!(passes("fail", FailOn::Never));
        assert!(!passes("bogus", FailOn::Fail));
    }

    #[test]
    fn encode_segment_escapes_reserved_bytes() {
        assert_eq!(encode_segment("payments-api"), "payments-api");
        assert_eq!(encode_segment("a b/c?d"), "a%20b%2Fc%3Fd");
        assert_eq!(encode_segment("ö"), "%C3%B6");
    }

    #[test]
    fn render_lists_verdict_evidence_and_reasons() {
        let resp = GateResponse::from_value(&serde_json::json!({
            "target": "database",
            "score": 75.0,
            "band": "good",
            "verdict": "warn",
            "reasons": ["1 experiment(s) failed on the latest run: cache-stampede"],
            "evidence": {"runs": 2, "last_run_id": "exp-fail", "last_run_at_ns": 1},
            "thresholds": {"min_score": 70.0, "max_age_days": 30}
        }))
        .unwrap();
        assert_eq!(resp.evidence.last_run_at_ns, Some(1));
        let text = render(&resp);
        assert!(text.contains("gate database: WARN (score 75.0, band good, runs 2)"));
        assert!(text.contains("latest evidence: exp-fail at 1 ns"));
        assert!(text.contains("- 1 experiment(s) failed"));
    }

    #[tokio::test]
    async fn cmd_gate_requires_a_daemon_url() {
        let mut env = tumult_test_utils::EnvGuard::lock();
        env.remove("TUMULT_DAEMON_URL");
        let args = GateArgs {
            target: "db".into(),
            min_score: None,
            max_age_days: None,
            env: None,
            daemon_url: None,
            fail_on: FailOn::Fail,
            json: false,
        };
        let err = cmd_gate(&args).await.unwrap_err();
        assert!(err.to_string().contains("no daemon URL"), "{err}");
    }
}
