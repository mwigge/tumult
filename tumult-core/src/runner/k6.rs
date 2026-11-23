//! K6 load executor — the concrete [`LoadExecutor`] implementation shared by
//! the CLI (`tumult run --load k6`, `tumult gameday run`) and the MCP
//! server's `tumult_gameday_run` tool.
//!
//! Spawns k6 as a background process, waits for it to complete, and parses
//! the JSON summary (with a text-output fallback) to produce a `LoadResult`.

use std::path::Path;

use super::{load_profile, LoadExecutor, LoadHandle};
use crate::types::{LoadConfig, LoadResult, LoadTool};

/// K6 load test executor.
///
/// The k6 binary is resolved from the `TUMULT_K6_BINARY` env var, falling
/// back to `k6` on `PATH`.
pub struct K6LoadExecutor;

/// Handle holding the k6 child process.
///
/// k6 is run with `--summary-export` so that `stop()` can read a stable JSON
/// summary; the human-readable stdout/stderr text is kept only as a fallback
/// for k6 binaries/versions that don't honor `--summary-export`.
struct K6Handle {
    child: std::process::Child,
    started_at_ns: i64,
    tool: LoadTool,
    vus: u32,
    summary_file: tempfile::NamedTempFile,
}

/// The statistics k6 writes for a trend metric. It leaves the 99th
/// percentile out of its summary unless it is named here.
const K6_TREND_STATS: &str = "avg,min,med,max,p(90),p(95),p(99)";

/// Upper bound on how long `stop()` waits for k6 to exit before killing it —
/// a hung k6 must not hang the whole run forever.
const K6_STOP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Wait for `child` to exit, collecting its piped output, but give up after
/// `timeout`: the child is killed and reaped, and an error is returned.
///
/// The piped streams are drained on reader threads while waiting so a chatty
/// child can't block on a full pipe and stall the wait.
fn wait_with_timeout(
    mut child: std::process::Child,
    timeout: std::time::Duration,
) -> Result<std::process::Output, String> {
    const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

    fn drain<R: std::io::Read + Send + 'static>(reader: R) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let mut reader = reader;
            let _ = reader.read_to_end(&mut buf);
            buf
        })
    }

    let stdout_thread = child.stdout.take().map(drain);
    let stderr_thread = child.stderr.take().map(drain);

    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    // Hung child — kill and reap so it doesn't outlive the run.
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(format!(
                        "process did not exit within {}s; killed",
                        timeout.as_secs()
                    ));
                }
                std::thread::sleep(
                    POLL_INTERVAL
                        .min(deadline.saturating_duration_since(std::time::Instant::now())),
                );
            }
            Err(e) => break Err(format!("k6 wait failed: {e}")),
        }
    };

    let stdout = stdout_thread.map_or_else(Vec::new, |t| t.join().unwrap_or_default());
    let stderr = stderr_thread.map_or_else(Vec::new, |t| t.join().unwrap_or_default());

    status.map(|status| std::process::Output {
        status,
        stdout,
        stderr,
    })
}

impl LoadExecutor for K6LoadExecutor {
    fn start(&self, config: &LoadConfig) -> Result<LoadHandle, String> {
        let vus = config.vus.unwrap_or(load_profile::DEFAULT_VUS);

        let k6_binary = std::env::var("TUMULT_K6_BINARY").unwrap_or_else(|_| "k6".to_string());

        let summary_file = tempfile::Builder::new()
            .prefix("tumult-k6-summary-")
            .suffix(".json")
            .tempfile()
            .map_err(|e| format!("failed to create k6 summary file: {e}"))?;

        let mut cmd = std::process::Command::new(&k6_binary);
        cmd.arg("run");
        for arg in k6_shape_args(config) {
            cmd.arg(arg);
        }
        cmd.arg("--summary-trend-stats")
            .arg(K6_TREND_STATS)
            .arg("--summary-export")
            .arg(summary_file.path())
            .arg(config.script.as_os_str())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        // Propagate OTel endpoint to k6 if available
        if let Ok(endpoint) = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT") {
            cmd.env("K6_OTEL_EXPORTER_OTLP_ENDPOINT", &endpoint);
        }

        let child =
            crate::spawn::alone(|| cmd.spawn()).map_err(|e| format!("failed to start k6: {e}"))?;

        let started_at_ns = super::epoch_nanos_now();

        Ok(LoadHandle {
            inner: Box::new(K6Handle {
                child,
                started_at_ns,
                tool: config.tool.clone(),
                vus,
                summary_file,
            }),
        })
    }

    fn stop(&self, handle: LoadHandle) -> Result<LoadResult, String> {
        let k6: K6Handle = *handle
            .inner
            .downcast::<K6Handle>()
            .map_err(|_| "invalid load handle")?;

        let output = wait_with_timeout(k6.child, K6_STOP_TIMEOUT)?;

        let ended_at_ns = super::epoch_nanos_now();
        let duration_ns = ended_at_ns - k6.started_at_ns;
        #[allow(clippy::cast_precision_loss)]
        let elapsed_s = duration_ns as f64 / 1_000_000_000.0;

        // k6's text summary is not a stable interface; prefer the JSON summary
        // written via --summary-export and fall back to the combined
        // stdout/stderr text only if it's missing or unparseable.
        let summary = read_k6_summary(k6.summary_file.path());

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{stdout}\n{stderr}");

        // k6 rate format: "iterations...: 300 29.82/s" — extract the rate after the count
        let throughput_rps = k6_metric_or_warn(
            k6_summary_metric(summary.as_ref(), "iterations", "rate")
                .or_else(|| parse_k6_rate(&combined, "iterations")),
            "iterations.rate",
        );
        let latency_p50 = k6_metric_or_warn(
            k6_summary_metric(summary.as_ref(), "iteration_duration", "med")
                .or_else(|| parse_k6_metric(&combined, "iteration_duration", "med")),
            "iteration_duration.med",
        );
        let latency_p95 = k6_metric_or_warn(
            k6_summary_metric(summary.as_ref(), "iteration_duration", "p(95)")
                .or_else(|| k6_summary_metric(summary.as_ref(), "pg_query_duration_ms", "p(95)"))
                .or_else(|| parse_k6_metric(&combined, "iteration_duration", "p(95)"))
                .or_else(|| parse_k6_metric(&combined, "pg_query_duration_ms", "p(95)")),
            "iteration_duration.p(95)",
        );
        let latency_p99 = k6_metric_or_warn(
            k6_summary_metric(summary.as_ref(), "iteration_duration", "p(99)")
                .or_else(|| parse_k6_metric(&combined, "iteration_duration", "p(99)")),
            "iteration_duration.p(99)",
        );

        // A script that sends HTTP requests is measured by k6's own
        // `http_reqs` and `http_req_failed`, whether or not it calls
        // `check()`. One that sends none (an xk6-sql script, say) keeps its
        // failed checks and its iterations, which a script may lack: a
        // missing value is treated as zero without a warning.
        let requests = k6_request_counts(summary.as_ref());
        let checks_total = k6_summary_count(summary.as_ref(), "checks_total")
            .or_else(|| parse_k6_counter(&combined, "checks_total"))
            .unwrap_or(0);
        let checks_failed = k6_summary_count(summary.as_ref(), "checks_failed")
            .or_else(|| parse_k6_counter(&combined, "checks_failed"))
            .unwrap_or(0);
        let check_error_rate = if checks_total > 0 {
            #[allow(clippy::cast_precision_loss)]
            {
                checks_failed as f64 / checks_total as f64
            }
        } else {
            0.0
        };
        let error_rate = requests
            .as_ref()
            .and_then(crate::types::RequestCounts::failure_share)
            .unwrap_or(check_error_rate);

        let iterations = k6_summary_count(summary.as_ref(), "iterations")
            .or_else(|| parse_k6_counter(&combined, "iterations"))
            .unwrap_or(0);
        let total_requests = requests.map_or(iterations, |counts| counts.sent);

        Ok(LoadResult {
            tool: k6.tool,
            started_at_ns: k6.started_at_ns,
            ended_at_ns,
            duration_s: elapsed_s,
            vus: k6.vus,
            throughput_rps,
            latency_p50_ms: latency_p50,
            latency_p95_ms: latency_p95,
            latency_p99_ms: latency_p99,
            error_rate,
            total_requests,
            thresholds_met: output.status.success(),
            // k6 reports one end-of-run summary; its per-request stream
            // (`--out json`) is far too heavy to keep, so no windows here.
            samples: vec![],
            execution: crate::types::LoadExecution {
                requests,
                ..crate::types::LoadExecution::default()
            },
        })
    }

    /// A k6 that has already exited is collected as on a normal stop; one
    /// still running is killed at once and leaves a result marked aborted
    /// with no measurements, since k6 writes its summary only on a clean
    /// exit.
    fn abort(&self, handle: LoadHandle) -> Result<LoadResult, String> {
        let mut k6 = handle
            .inner
            .downcast::<K6Handle>()
            .map_err(|_| "invalid load handle")?;
        match k6.child.try_wait() {
            Ok(Some(_)) => self.stop(LoadHandle { inner: k6 }),
            Ok(None) => Ok(Self::cut_short(*k6)),
            Err(e) => Err(format!("k6 wait failed: {e}")),
        }
    }
}

impl K6LoadExecutor {
    /// Kill a running k6 and say so in the result.
    fn cut_short(mut k6: K6Handle) -> LoadResult {
        // Both fail only when the process is already gone.
        let _ = k6.child.kill();
        let _ = k6.child.wait();
        tracing::warn!(tool = "k6", reason = "cut_before_summary", "load.aborted");
        let ended_at_ns = super::epoch_nanos_now();
        #[allow(clippy::cast_precision_loss)]
        let duration_s = (ended_at_ns - k6.started_at_ns) as f64 / 1_000_000_000.0;
        LoadResult {
            tool: k6.tool,
            started_at_ns: k6.started_at_ns,
            ended_at_ns,
            duration_s,
            vus: k6.vus,
            throughput_rps: 0.0,
            latency_p50_ms: 0.0,
            latency_p95_ms: 0.0,
            latency_p99_ms: 0.0,
            error_rate: 0.0,
            total_requests: 0,
            thresholds_met: false,
            samples: vec![],
            execution: crate::types::LoadExecution {
                aborted: true,
                reasons: vec![
                    "load aborted before k6 finished; k6 writes its summary only on a clean exit"
                        .into(),
                ],
                ..crate::types::LoadExecution::default()
            },
        }
    }
}

/// The k6 arguments that shape the traffic: a flat config is `--vus N
/// --duration Ds` as before; a profile becomes `--vus <start>` plus one
/// `--stage` per rendered plan stage (k6 reads `--vus` as the starting
/// level when stages are present).
#[must_use]
pub fn k6_shape_args(config: &LoadConfig) -> Vec<String> {
    let plan = load_profile::plan(config);
    if config.profile.is_none() {
        let duration = config
            .duration_s
            .map_or_else(|| "30s".to_string(), |s| format!("{s}s"));
        return vec![
            "--vus".into(),
            plan.start_vus.to_string(),
            "--duration".into(),
            duration,
        ];
    }
    let mut args = vec!["--vus".to_string(), plan.start_vus.to_string()];
    for stage in load_profile::k6_stages(&plan) {
        args.push("--stage".into());
        args.push(stage);
    }
    args
}

/// Reads and parses a k6 `--summary-export` JSON file.
///
/// Returns `None` (logging a warning) if the file is missing, empty, or not
/// valid JSON — callers fall back to text-based parsing in that case.
#[must_use]
pub fn read_k6_summary(path: &Path) -> Option<serde_json::Value> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) => {
            tracing::warn!(error = %e, "k6 --summary-export file not found; falling back to text parsing");
            return None;
        }
    };
    match serde_json::from_str(&text) {
        Ok(value) => Some(value),
        Err(e) => {
            tracing::warn!(error = %e, "failed to parse k6 --summary-export JSON; falling back to text parsing");
            None
        }
    }
}

/// Looks up `metrics.<metric>.<field>` in a k6 JSON summary.
#[must_use]
pub fn k6_summary_metric(
    summary: Option<&serde_json::Value>,
    metric: &str,
    field: &str,
) -> Option<f64> {
    summary?.get("metrics")?.get(metric)?.get(field)?.as_f64()
}

/// Looks up `metrics.<metric>.count` in a k6 JSON summary.
#[must_use]
pub fn k6_summary_count(summary: Option<&serde_json::Value>, metric: &str) -> Option<u64> {
    summary?.get("metrics")?.get(metric)?.get("count")?.as_u64()
}

/// What became of the requests, as far as a k6 summary says: `http_reqs`
/// sent, and of those the `http_req_failed` ones failed (k6 does not say
/// how, nor which timed out). `None` when the script made no HTTP
/// request or the summary lacks either metric.
#[must_use]
pub fn k6_request_counts(
    summary: Option<&serde_json::Value>,
) -> Option<crate::types::RequestCounts> {
    let sent = k6_summary_count(summary, "http_reqs").filter(|sent| *sent > 0)?;
    // A rate metric's `passes` are the samples that were non-zero: here,
    // the requests that failed.
    let failed = summary?
        .get("metrics")?
        .get("http_req_failed")?
        .get("passes")?
        .as_u64()?;
    Some(crate::types::RequestCounts {
        sent,
        ok: sent.saturating_sub(failed),
        unclassified: failed.min(sent),
        ..crate::types::RequestCounts::default()
    })
}

/// Returns `value`, or logs a warning and defaults to `0.0` if the metric
/// could not be parsed from either the JSON summary or the text fallback.
#[must_use]
pub fn k6_metric_or_warn(value: Option<f64>, metric: &str) -> f64 {
    value.unwrap_or_else(|| {
        tracing::warn!(metric, "k6: failed to parse metric; defaulting to 0");
        0.0
    })
}

/// Parses a k6 summary metric value from stdout.
///
/// k6 outputs lines like:
///   `iteration_duration...: avg=97.77ms min=55.75ms med=63.81ms max=201.09ms p(90)=67.34ms p(95)=148.01ms`
#[must_use]
pub fn parse_k6_metric(output: &str, metric_name: &str, stat: &str) -> Option<f64> {
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(metric_name) {
            // Find stat=value pattern
            let search = format!("{stat}=");
            if let Some(pos) = trimmed.find(&search) {
                let after = &trimmed[pos + search.len()..];
                // Extract number, stripping units like "ms", "s"
                let num_str: String = after
                    .chars()
                    .take_while(|c| c.is_ascii_digit() || *c == '.')
                    .collect();
                return num_str.parse().ok();
            }
        }
    }
    None
}

/// Parses a k6 counter value from stdout.
///
/// k6 outputs lines like:
///   `iterations...........: 1025 51.006998/s`
///   `checks_total.......: 1025    51.006998/s`
///   `checks_failed......: 2.52%   32 out of 1268`
///
/// A line of checks gives a share first: its count is the column after.
#[must_use]
pub fn parse_k6_counter(output: &str, counter_name: &str) -> Option<u64> {
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(counter_name) {
            // After the dots and colons, find the first number
            if let Some(colon_pos) = trimmed.find(':') {
                let mut columns = trimmed[colon_pos + 1..].split_whitespace();
                let first = columns.next()?;
                let count = if first.ends_with('%') {
                    columns.next()?
                } else {
                    first
                };
                let num_str: String = count.chars().take_while(char::is_ascii_digit).collect();
                return num_str.parse().ok();
            }
        }
    }
    None
}

/// Parses a k6 rate value (requests/s) from the counter line.
///
/// k6 outputs: `iterations...........: 300 29.82/s`
#[must_use]
pub fn parse_k6_rate(output: &str, counter_name: &str) -> Option<f64> {
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(counter_name) {
            // Find the rate: number followed by /s
            if let Some(slash_pos) = trimmed.find("/s") {
                let before = &trimmed[..slash_pos];
                // Walk backward to find the start of the number
                let num_str: String = before
                    .chars()
                    .rev()
                    .take_while(|c| c.is_ascii_digit() || *c == '.')
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect();
                return num_str.parse().ok();
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::LoadProfile;

    fn config(profile: Option<LoadProfile>) -> LoadConfig {
        LoadConfig {
            tool: LoadTool::K6,
            script: "load.js".into(),
            vus: Some(10),
            duration_s: Some(60.0),
            thresholds: std::collections::HashMap::new(),
            profile,
        }
    }

    #[test]
    fn request_counts_come_from_http_reqs_and_http_req_failed() {
        let summary = serde_json::json!({"metrics": {
            "http_reqs": {"count": 200, "rate": 20.0},
            "http_req_failed": {"passes": 30, "fails": 170, "value": 0.15},
        }});
        let counts = k6_request_counts(Some(&summary)).expect("both metrics are there");
        assert_eq!(
            counts,
            crate::types::RequestCounts {
                sent: 200,
                ok: 170,
                unclassified: 30,
                ..crate::types::RequestCounts::default()
            }
        );
        // A script that makes no HTTP request, or a summary that is
        // missing: nothing is claimed.
        let no_http = serde_json::json!({"metrics": {"iterations": {"count": 5}}});
        assert_eq!(k6_request_counts(Some(&no_http)), None);
        let half = serde_json::json!({"metrics": {"http_reqs": {"count": 200}}});
        assert_eq!(k6_request_counts(Some(&half)), None);
        assert_eq!(k6_request_counts(None), None);
    }

    #[test]
    fn flat_config_keeps_the_vus_duration_flags() {
        assert_eq!(
            k6_shape_args(&config(None)),
            vec!["--vus", "10", "--duration", "60s"]
        );
    }

    #[test]
    fn profile_becomes_stages_with_the_start_level() {
        let spike = LoadProfile::Spike {
            peak_vus: 100,
            after_s: 20.0,
            hold_s: 10.0,
            sample_interval_s: None,
        };
        let args = k6_shape_args(&config(Some(spike)));
        assert_eq!(&args[..2], &["--vus", "10"]);
        assert!(!args.contains(&"--duration".to_string()));
        assert_eq!(args.iter().filter(|a| *a == "--stage").count(), 5);
        assert!(args.contains(&"1s:100".to_string()));
    }

    fn spawn_piped(program: &str, args: &[&str]) -> std::process::Child {
        let mut command = std::process::Command::new(program);
        command
            .args(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        crate::spawn::alone(|| command.spawn()).expect("spawn test child")
    }

    #[test]
    fn wait_with_timeout_collects_output_of_fast_child() {
        let child = spawn_piped("echo", &["hello"]);
        let output = wait_with_timeout(child, std::time::Duration::from_secs(5))
            .expect("fast child must succeed");
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("hello"));
    }

    #[cfg(unix)]
    #[test]
    fn a_k6_cut_short_is_one_named_event_with_its_reason_as_a_field() {
        use crate::runner::tests::log_capture::logged;

        let handle = K6Handle {
            child: spawn_piped("sleep", &["300"]),
            started_at_ns: crate::runner::epoch_nanos_now(),
            tool: LoadTool::K6,
            vus: 2,
            summary_file: tempfile::NamedTempFile::new().expect("temp file"),
        };

        let events = logged(|| {
            let result = K6LoadExecutor::cut_short(handle);
            assert!(result.execution.aborted);
        });

        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].event, "load.aborted");
        assert_eq!(events[0].field("tool"), Some("k6"));
        assert_eq!(events[0].field("reason"), Some("cut_before_summary"));
    }

    #[cfg(unix)]
    #[test]
    fn wait_with_timeout_kills_hung_child() {
        let child = spawn_piped("sleep", &["300"]);
        let pid = child.id();

        let started = std::time::Instant::now();
        let result = wait_with_timeout(child, std::time::Duration::from_millis(200));
        let elapsed = started.elapsed();

        let err = result.expect_err("a hung child must error after the timeout");
        assert!(err.contains("killed"), "unexpected error: {err}");
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "the wait must be bounded; took {elapsed:?}"
        );

        // kill + wait must have reaped the child: `kill -0` fails for a
        // non-existent process.
        let probe = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .expect("run kill -0");
        assert!(
            !probe.success(),
            "hung child survived the bounded wait (pid {pid})"
        );
    }
}
