use std::path::Path;

use tumult_core::runner::{LoadExecutor, LoadHandle};
use tumult_core::types::{LoadConfig, LoadResult, LoadTool};

// ── K6 Load Executor ────────────────────────────────────────

/// K6 load test executor.
///
/// Spawns k6 as a background process, waits for it to complete,
/// and parses the JSON summary to produce a `LoadResult`.
pub(crate) struct K6LoadExecutor;

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

impl LoadExecutor for K6LoadExecutor {
    fn start(&self, config: &LoadConfig) -> Result<LoadHandle, String> {
        let vus = config.vus.unwrap_or(10);
        let duration = config
            .duration_s
            .map_or_else(|| "30s".to_string(), |s| format!("{s}s"));

        let k6_binary = std::env::var("TUMULT_K6_BINARY").unwrap_or_else(|_| "k6".to_string());

        let summary_file = tempfile::Builder::new()
            .prefix("tumult-k6-summary-")
            .suffix(".json")
            .tempfile()
            .map_err(|e| format!("failed to create k6 summary file: {e}"))?;

        let mut cmd = std::process::Command::new(&k6_binary);
        cmd.arg("run")
            .arg("--vus")
            .arg(vus.to_string())
            .arg("--duration")
            .arg(&duration)
            .arg("--summary-export")
            .arg(summary_file.path())
            .arg(config.script.as_os_str())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        // Propagate OTel endpoint to k6 if available
        if let Ok(endpoint) = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT") {
            cmd.env("K6_OTEL_EXPORTER_OTLP_ENDPOINT", &endpoint);
        }

        let child = cmd
            .spawn()
            .map_err(|e| format!("failed to start k6: {e}"))?;

        let started_at_ns = tumult_core::runner::epoch_nanos_now();

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

        let output = k6
            .child
            .wait_with_output()
            .map_err(|e| format!("k6 wait failed: {e}"))?;

        let ended_at_ns = tumult_core::runner::epoch_nanos_now();
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

        // Check failure rate and total iterations are derived from custom
        // counters that scripts may legitimately omit, so a missing value
        // is treated as zero without a warning.
        let checks_total = k6_summary_count(summary.as_ref(), "checks_total")
            .or_else(|| parse_k6_counter(&combined, "checks_total"))
            .unwrap_or(0);
        let checks_failed = k6_summary_count(summary.as_ref(), "checks_failed")
            .or_else(|| parse_k6_counter(&combined, "checks_failed"))
            .unwrap_or(0);
        let error_rate = if checks_total > 0 {
            #[allow(clippy::cast_precision_loss)]
            {
                checks_failed as f64 / checks_total as f64
            }
        } else {
            0.0
        };

        let iterations = k6_summary_count(summary.as_ref(), "iterations")
            .or_else(|| parse_k6_counter(&combined, "iterations"))
            .unwrap_or(0);

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
            total_requests: iterations,
            thresholds_met: output.status.success(),
        })
    }
}

/// Reads and parses a k6 `--summary-export` JSON file.
///
/// Returns `None` (logging a warning) if the file is missing, empty, or not
/// valid JSON — callers fall back to text-based parsing in that case.
pub(crate) fn read_k6_summary(path: &Path) -> Option<serde_json::Value> {
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
pub(crate) fn k6_summary_metric(
    summary: Option<&serde_json::Value>,
    metric: &str,
    field: &str,
) -> Option<f64> {
    summary?.get("metrics")?.get(metric)?.get(field)?.as_f64()
}

/// Looks up `metrics.<metric>.count` in a k6 JSON summary.
pub(crate) fn k6_summary_count(summary: Option<&serde_json::Value>, metric: &str) -> Option<u64> {
    summary?.get("metrics")?.get(metric)?.get("count")?.as_u64()
}

/// Returns `value`, or logs a warning and defaults to `0.0` if the metric
/// could not be parsed from either the JSON summary or the text fallback.
pub(crate) fn k6_metric_or_warn(value: Option<f64>, metric: &str) -> f64 {
    value.unwrap_or_else(|| {
        tracing::warn!(metric, "k6: failed to parse metric; defaulting to 0");
        0.0
    })
}

/// Parses a k6 summary metric value from stdout.
///
/// k6 outputs lines like:
///   `iteration_duration...: avg=97.77ms min=55.75ms med=63.81ms max=201.09ms p(90)=67.34ms p(95)=148.01ms`
pub(crate) fn parse_k6_metric(output: &str, metric_name: &str, stat: &str) -> Option<f64> {
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
pub(crate) fn parse_k6_counter(output: &str, counter_name: &str) -> Option<u64> {
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(counter_name) {
            // After the dots and colons, find the first number
            if let Some(colon_pos) = trimmed.find(':') {
                let after = trimmed[colon_pos + 1..].trim();
                let num_str: String = after.chars().take_while(char::is_ascii_digit).collect();
                return num_str.parse().ok();
            }
        }
    }
    None
}

/// Parses a k6 rate value (requests/s) from the counter line.
///
/// k6 outputs: `iterations...........: 300 29.82/s`
pub(crate) fn parse_k6_rate(output: &str, counter_name: &str) -> Option<f64> {
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
