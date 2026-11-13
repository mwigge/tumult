//! `tumult perf <target>` — ask the daemon for a target's performance
//! regression verdict (`GET /api/perf/{target}`) and turn it into an exit
//! code a pipeline can act on, like `tumult gate`.

use anyhow::{bail, Context, Result};
use serde_json::Value;

/// Env var naming the daemon base URL (shared with `tumult gate`).
const DAEMON_URL_ENV: &str = "TUMULT_DAEMON_URL";
/// Env var with the daemon API token (`kro_...`).
const DAEMON_TOKEN_ENV: &str = "TUMULT_DAEMON_TOKEN";

/// Which verdicts make the command exit non-zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PerfFailOn {
    /// Exit non-zero on `regressed` only (default).
    Regressed,
    /// Exit non-zero on `regressed` or `inconclusive`.
    Inconclusive,
    /// Always exit zero; print the verdict only.
    Never,
}

/// Arguments of `tumult perf`.
#[derive(Debug, Clone)]
pub struct PerfArgs {
    pub target: String,
    pub probe: Option<String>,
    pub quantile: Option<String>,
    pub min_samples: Option<u32>,
    pub margin_pct: Option<f64>,
    pub env: Option<String>,
    pub daemon_url: Option<String>,
    pub fail_on: PerfFailOn,
    pub json: bool,
}

/// One side of the daemon's answer, only what the CLI prints.
#[derive(Debug, Clone, Default)]
pub struct PerfSide {
    pub version: String,
    pub samples: u64,
    pub quantile_value: Option<f64>,
}

/// The daemon's answer (`GET /api/perf/{target}`); unknown fields are
/// ignored so the daemon can grow the payload.
#[derive(Debug, Clone)]
pub struct PerfResponse {
    pub target: String,
    pub probe: String,
    pub verdict: String,
    pub reasons: Vec<String>,
    pub quantile: String,
    pub baseline: Option<PerfSide>,
    pub candidate: PerfSide,
    pub delta: Option<f64>,
    pub delta_pct: Option<f64>,
    pub ci: Option<(f64, f64)>,
    pub margin: Option<f64>,
    pub stored: bool,
}

fn side(v: &Value, quantile: &str) -> Option<PerfSide> {
    let version = v.get("version")?.as_str()?.to_owned();
    Some(PerfSide {
        version,
        samples: v.get("samples").and_then(Value::as_u64).unwrap_or_default(),
        quantile_value: v.get(quantile).and_then(Value::as_f64),
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
            stored: v.get("stored").and_then(Value::as_bool).unwrap_or(false),
            quantile,
        })
    }
}

/// Whether a verdict passes under the `fail_on` policy. Pure.
#[must_use]
pub fn passes(verdict: &str, fail_on: PerfFailOn) -> bool {
    match fail_on {
        PerfFailOn::Never => true,
        PerfFailOn::Regressed => matches!(verdict, "improved" | "unchanged" | "inconclusive"),
        PerfFailOn::Inconclusive => matches!(verdict, "improved" | "unchanged"),
    }
}

fn fmt_opt(v: Option<f64>) -> String {
    v.map_or_else(|| "n/a".to_string(), |v| format!("{v:.1}"))
}

/// Human-readable rendering of the perf answer.
#[must_use]
pub fn render(resp: &PerfResponse) -> String {
    let mut out = format!(
        "perf {}: {} ({} of {})\n",
        resp.target,
        resp.verdict.to_uppercase(),
        resp.quantile,
        resp.probe
    );
    if let Some(b) = &resp.baseline {
        out.push_str(&format!(
            "  baseline  {}: {} ms over {} sample(s)\n",
            b.version,
            fmt_opt(b.quantile_value),
            b.samples
        ));
    }
    out.push_str(&format!(
        "  candidate {}: {} ms over {} sample(s)\n",
        resp.candidate.version,
        fmt_opt(resp.candidate.quantile_value),
        resp.candidate.samples
    ));
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
        out.push_str(&format!("  delta {delta:+.1} ms{pct}{ci}{margin}\n"));
    }
    for reason in &resp.reasons {
        out.push_str(&format!("  - {reason}\n"));
    }
    if resp.stored {
        out.push_str("  verdict stored in perf_verdicts\n");
    }
    out
}

/// Query the daemon and print the verdict. Returns `Ok(true)` when the
/// verdict passes under `fail_on`; the caller turns `false` into a
/// non-zero exit.
///
/// # Errors
///
/// Returns an error when no daemon URL is configured, the daemon is
/// unreachable, or it answers with an error status.
pub async fn cmd_perf(args: &PerfArgs) -> Result<bool> {
    let base = args
        .daemon_url
        .clone()
        .or_else(|| std::env::var(DAEMON_URL_ENV).ok())
        .filter(|s| !s.is_empty())
        .with_context(|| format!("no daemon URL: pass --daemon-url or set {DAEMON_URL_ENV}"))?;
    let url = format!(
        "{}/api/perf/{}",
        base.trim_end_matches('/'),
        super::gate::encode_segment(&args.target)
    );
    let mut query: Vec<(&str, String)> = Vec::new();
    if let Some(p) = &args.probe {
        query.push(("probe", p.clone()));
    }
    if let Some(q) = &args.quantile {
        query.push(("quantile", q.clone()));
    }
    if let Some(m) = args.min_samples {
        query.push(("min_samples", m.to_string()));
    }
    if let Some(m) = args.margin_pct {
        query.push(("margin_pct", m.to_string()));
    }
    if let Some(env) = &args.env {
        query.push(("env", env.clone()));
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
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
        .with_context(|| format!("perf request to {url} failed"))?;
    let status = resp.status();
    let body: Value = resp
        .json()
        .await
        .with_context(|| format!("perf answered HTTP {status} with an unreadable body"))?;
    if !status.is_success() {
        let msg = body
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        bail!("perf answered HTTP {status}: {msg}");
    }
    let parsed = PerfResponse::from_value(&body)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&body)?);
    } else {
        print!("{}", render(&parsed));
    }
    Ok(passes(&parsed.verdict, args.fail_on))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fail_on_policy_maps_verdicts_to_exit_outcomes() {
        assert!(passes("improved", PerfFailOn::Regressed));
        assert!(passes("unchanged", PerfFailOn::Regressed));
        assert!(passes("inconclusive", PerfFailOn::Regressed));
        assert!(!passes("regressed", PerfFailOn::Regressed));
        assert!(!passes("inconclusive", PerfFailOn::Inconclusive));
        assert!(!passes("regressed", PerfFailOn::Inconclusive));
        assert!(passes("unchanged", PerfFailOn::Inconclusive));
        assert!(passes("regressed", PerfFailOn::Never));
        assert!(!passes("bogus", PerfFailOn::Regressed));
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

    #[tokio::test]
    async fn cmd_perf_requires_a_daemon_url() {
        std::env::remove_var("TUMULT_DAEMON_URL");
        let args = PerfArgs {
            target: "checkout".into(),
            probe: None,
            quantile: None,
            min_samples: None,
            margin_pct: None,
            env: None,
            daemon_url: None,
            fail_on: PerfFailOn::Regressed,
            json: false,
        };
        let err = cmd_perf(&args).await.unwrap_err();
        assert!(err.to_string().contains("no daemon URL"), "{err}");
    }
}
