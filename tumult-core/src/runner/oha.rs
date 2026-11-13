//! `oha` load adapter — the second [`LoadExecutor`] behind the same
//! boundary as [`super::k6`], proving a profile means the same thing
//! whichever tool runs it.
//!
//! oha cannot shape traffic over time, so the adapter runs the plan's
//! [`load_profile::windows`] one process at a time (`-c <vus> -z <len>`),
//! parses each JSON summary into a [`LoadSample`], and aggregates the
//! windows into the run's [`LoadResult`]. A soak profile with
//! `sample_interval_s` therefore streams one sample per interval; a flat
//! config is a single window and a single summary, as with k6.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::{load_profile, LoadExecutor, LoadHandle};
use crate::types::{LoadConfig, LoadResult, LoadSample, LoadTool};

/// oha load executor.
///
/// The binary is resolved from `TUMULT_OHA_BINARY`, falling back to `oha`
/// on `PATH`. `LoadConfig::script` carries the URL to hit.
pub struct OhaLoadExecutor;

/// Grace added to a window's planned length before its process is killed.
const WINDOW_GRACE: std::time::Duration = std::time::Duration::from_secs(10);
/// How long `stop()` lets the in-flight window finish before killing it.
const OHA_STOP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

/// What one window produced.
struct WindowOutcome {
    ts_ns: i64,
    duration_s: f64,
    vus: u32,
    summary: Result<OhaSummary, String>,
}

struct OhaHandle {
    stop: Arc<AtomicBool>,
    current: Arc<Mutex<Option<std::process::Child>>>,
    worker: std::thread::JoinHandle<Vec<WindowOutcome>>,
    started_at_ns: i64,
    tool: LoadTool,
    vus: u32,
    thresholds: std::collections::HashMap<String, f64>,
}

/// The fields of an oha `--json` summary the adapter keeps. Latencies are
/// converted from oha's seconds to milliseconds.
#[derive(Debug, Clone, PartialEq)]
pub struct OhaSummary {
    pub requests: u64,
    pub success_rate: f64,
    pub throughput_rps: f64,
    pub latency_p50_ms: f64,
    pub latency_p95_ms: f64,
    pub latency_p99_ms: f64,
}

/// Parse oha's JSON output (`oha -j`). Every field the adapter needs must
/// be present; a missing one is an error rather than a silent zero.
///
/// # Errors
///
/// Returns an error when the text is not JSON or lacks `summary`,
/// `latencyPercentiles` or `statusCodeDistribution`.
pub fn parse_oha_summary(text: &str) -> Result<OhaSummary, String> {
    let v: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("oha output is not JSON: {e}"))?;
    let number = |path: &[&str]| -> Result<f64, String> {
        let mut cur = &v;
        for key in path {
            cur = cur
                .get(key)
                .ok_or_else(|| format!("oha summary lacks {}", path.join(".")))?;
        }
        cur.as_f64()
            .ok_or_else(|| format!("oha summary {} is not a number", path.join(".")))
    };
    let count = |key: &str| -> Result<u64, String> {
        let Some(map) = v.get(key) else {
            return Err(format!("oha summary lacks {key}"));
        };
        let map = map
            .as_object()
            .ok_or_else(|| format!("oha summary {key} is not an object"))?;
        Ok(map.values().filter_map(serde_json::Value::as_u64).sum())
    };
    let statuses = count("statusCodeDistribution")?;
    let errors = count("errorDistribution").unwrap_or(0);
    Ok(OhaSummary {
        requests: statuses + errors,
        success_rate: number(&["summary", "successRate"])?,
        throughput_rps: number(&["summary", "requestsPerSec"])?,
        latency_p50_ms: number(&["latencyPercentiles", "p50"])? * 1000.0,
        latency_p95_ms: number(&["latencyPercentiles", "p95"])? * 1000.0,
        latency_p99_ms: number(&["latencyPercentiles", "p99"])? * 1000.0,
    })
}

/// oha's `-z` duration: whole seconds as `Ns`, anything else in `ms`.
fn oha_duration(seconds: f64) -> String {
    if seconds.fract() == 0.0 {
        return format!("{seconds:.0}s");
    }
    format!("{:.0}ms", (seconds * 1000.0).round())
}

/// Spawn one window's process and wait for it, bounded by the window's
/// length plus grace. The child is parked in `current` while it runs so
/// `stop()` can kill it.
fn run_window(
    binary: &str,
    url: &std::ffi::OsStr,
    window: &load_profile::Window,
    current: &Mutex<Option<std::process::Child>>,
) -> Result<OhaSummary, String> {
    let child = std::process::Command::new(binary)
        .arg("--no-tui")
        .arg("-j")
        .arg("-c")
        .arg(window.vus.to_string())
        .arg("-z")
        .arg(oha_duration(window.duration_s))
        .arg(url)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to start oha: {e}"))?;
    let output = wait_parked(child, current, window_deadline(window.duration_s))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "oha exited with {}: {}",
            output.status,
            stderr.trim()
        ));
    }
    parse_oha_summary(&String::from_utf8_lossy(&output.stdout))
}

fn window_deadline(duration_s: f64) -> std::time::Instant {
    std::time::Instant::now()
        + std::time::Duration::from_secs_f64(duration_s.max(0.0))
        + WINDOW_GRACE
}

/// Park `child` in `current`, drain its pipes on reader threads, and poll
/// until it exits or `deadline` passes (then kill and reap it).
fn wait_parked(
    mut child: std::process::Child,
    current: &Mutex<Option<std::process::Child>>,
    deadline: std::time::Instant,
) -> Result<std::process::Output, String> {
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
    *lock(current) = Some(child);

    let status = loop {
        let mut slot = lock(current);
        let polled = match slot.as_mut() {
            None => Err("oha window was removed while running".to_string()),
            Some(child) => match child.try_wait() {
                Ok(Some(status)) => Ok(Some(status)),
                Ok(None) if std::time::Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    Err("oha did not exit within its window plus grace; killed".to_string())
                }
                Ok(None) => Ok(None),
                Err(e) => Err(format!("oha wait failed: {e}")),
            },
        };
        match polled {
            Ok(None) => {
                drop(slot);
                std::thread::sleep(POLL_INTERVAL);
            }
            Ok(Some(status)) => {
                *slot = None;
                break Ok(status);
            }
            Err(e) => {
                *slot = None;
                break Err(e);
            }
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

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl LoadExecutor for OhaLoadExecutor {
    fn start(&self, config: &LoadConfig) -> Result<LoadHandle, String> {
        let binary = std::env::var("TUMULT_OHA_BINARY").unwrap_or_else(|_| "oha".to_string());
        let plan = load_profile::plan(config);
        let windows = load_profile::windows(&plan);
        let url = config.script.as_os_str().to_owned();
        let stop = Arc::new(AtomicBool::new(false));
        let current = Arc::new(Mutex::new(None));

        // Fail fast on a missing binary instead of discovering it in the
        // worker thread, so `start` errors like the k6 adapter does.
        let probe = std::process::Command::new(&binary)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        if let Err(e) = probe {
            return Err(format!("failed to start oha ({binary}): {e}"));
        }

        let started_at_ns = super::epoch_nanos_now();
        let worker = {
            let stop = Arc::clone(&stop);
            let current = Arc::clone(&current);
            std::thread::spawn(move || {
                let mut outcomes = Vec::with_capacity(windows.len());
                for window in windows {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let ts_ns = super::epoch_nanos_now();
                    let summary = run_window(&binary, &url, &window, &current);
                    outcomes.push(WindowOutcome {
                        ts_ns,
                        duration_s: window.duration_s,
                        vus: window.vus,
                        summary,
                    });
                }
                outcomes
            })
        };
        tracing::info!(
            windows = plan.stages.len(),
            peak_vus = plan.peak_vus(),
            "oha load plan started"
        );
        Ok(LoadHandle {
            inner: Box::new(OhaHandle {
                stop,
                current,
                worker,
                started_at_ns,
                tool: config.tool.clone(),
                vus: plan.peak_vus(),
                thresholds: config.thresholds.clone(),
            }),
        })
    }

    fn stop(&self, handle: LoadHandle) -> Result<LoadResult, String> {
        let oha: OhaHandle = *handle
            .inner
            .downcast::<OhaHandle>()
            .map_err(|_| "invalid load handle")?;
        let OhaHandle {
            stop,
            current,
            worker,
            started_at_ns,
            tool,
            vus,
            thresholds,
        } = oha;
        stop.store(true, Ordering::SeqCst);
        let deadline = std::time::Instant::now() + OHA_STOP_TIMEOUT;
        while !worker.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(POLL_INTERVAL);
        }
        let interrupted = !worker.is_finished();
        if interrupted {
            if let Some(child) = lock(&current).as_mut() {
                let _ = child.kill();
            }
        }
        let mut outcomes = worker
            .join()
            .map_err(|_| "oha worker thread panicked".to_string())?;
        if interrupted {
            // The window we cut short carries no usable summary.
            outcomes.pop();
        }
        let ended_at_ns = super::epoch_nanos_now();
        let run = RunShape {
            started_at_ns,
            ended_at_ns,
            tool,
            vus,
        };
        Ok(aggregate(&run, outcomes, &thresholds))
    }
}

/// The run-level facts the aggregate carries over unchanged.
struct RunShape {
    started_at_ns: i64,
    ended_at_ns: i64,
    tool: LoadTool,
    vus: u32,
}

/// Fold the windows into one [`LoadResult`]. Throughput is time-weighted,
/// latency percentiles are request-weighted means of the window
/// percentiles (an approximation, since oha does not expose raw
/// latencies), and every window with a summary becomes a sample.
fn aggregate(
    run: &RunShape,
    outcomes: Vec<WindowOutcome>,
    thresholds: &std::collections::HashMap<String, f64>,
) -> LoadResult {
    let mut samples = Vec::new();
    let mut all_windows_ok = true;
    for outcome in outcomes {
        match outcome.summary {
            Ok(summary) => samples.push(LoadSample {
                ts_ns: outcome.ts_ns,
                duration_s: outcome.duration_s,
                vus: outcome.vus,
                throughput_rps: summary.throughput_rps,
                latency_p50_ms: summary.latency_p50_ms,
                latency_p95_ms: summary.latency_p95_ms,
                latency_p99_ms: summary.latency_p99_ms,
                error_rate: (1.0 - summary.success_rate).clamp(0.0, 1.0),
                requests: summary.requests,
            }),
            Err(e) => {
                all_windows_ok = false;
                tracing::warn!(error = %e, vus = outcome.vus, "oha window failed");
            }
        }
    }
    #[allow(clippy::cast_precision_loss)]
    let elapsed_s = (run.ended_at_ns - run.started_at_ns) as f64 / 1_000_000_000.0;
    let total_requests: u64 = samples.iter().map(|s| s.requests).sum();
    let total_time: f64 = samples.iter().map(|s| s.duration_s).sum();
    let weighted = |f: fn(&LoadSample) -> f64| -> f64 {
        if total_requests == 0 {
            return 0.0;
        }
        #[allow(clippy::cast_precision_loss)]
        let sum: f64 = samples.iter().map(|s| f(s) * s.requests as f64).sum();
        #[allow(clippy::cast_precision_loss)]
        {
            sum / total_requests as f64
        }
    };
    let throughput_rps = if total_time > 0.0 {
        samples
            .iter()
            .map(|s| s.throughput_rps * s.duration_s)
            .sum::<f64>()
            / total_time
    } else {
        0.0
    };
    let result = LoadResult {
        tool: run.tool.clone(),
        started_at_ns: run.started_at_ns,
        ended_at_ns: run.ended_at_ns,
        duration_s: elapsed_s,
        vus: run.vus,
        throughput_rps,
        latency_p50_ms: weighted(|s| s.latency_p50_ms),
        latency_p95_ms: weighted(|s| s.latency_p95_ms),
        latency_p99_ms: weighted(|s| s.latency_p99_ms),
        error_rate: weighted(|s| s.error_rate),
        total_requests,
        thresholds_met: all_windows_ok,
        samples,
    };
    let thresholds_met = result.thresholds_met && thresholds_hold(&result, thresholds);
    LoadResult {
        thresholds_met,
        ..result
    }
}

/// oha has no threshold language of its own, so the config's thresholds
/// are checked here against the aggregate: upper bounds on latency and
/// error rate, a lower bound on throughput. An unknown metric name is
/// reported, never silently passed.
fn thresholds_hold(
    result: &LoadResult,
    thresholds: &std::collections::HashMap<String, f64>,
) -> bool {
    let mut held = true;
    for (metric, bound) in thresholds {
        let ok = match metric.as_str() {
            "latency_p50_ms" => result.latency_p50_ms <= *bound,
            "latency_p95_ms" => result.latency_p95_ms <= *bound,
            "latency_p99_ms" => result.latency_p99_ms <= *bound,
            "error_rate" => result.error_rate <= *bound,
            "throughput_rps" => result.throughput_rps >= *bound,
            other => {
                tracing::warn!(
                    metric = other,
                    "oha: unknown threshold metric; treated as unmet"
                );
                false
            }
        };
        if !ok {
            tracing::warn!(metric, bound, "oha: threshold not met");
            held = false;
        }
    }
    held
}

#[cfg(test)]
mod tests {
    use super::*;

    const OHA_JSON: &str = r#"{"summary":{"successRate":0.98,"total":5.0,"slowest":0.2,"fastest":0.01,"average":0.05,"requestsPerSec":120.5,"totalData":100,"sizePerRequest":10,"sizePerSec":100},"responseTimeHistogram":{},"latencyPercentiles":{"p10":0.01,"p25":0.02,"p50":0.04,"p75":0.06,"p90":0.09,"p95":0.15,"p99":0.19,"p99.9":0.2,"p99.99":0.2},"rps":{"mean":120.5,"stddev":1.0,"max":130.0,"min":110.0,"percentiles":{}},"details":{},"statusCodeDistribution":{"200":588,"500":2},"errorDistribution":{"connection refused":10}}"#;

    #[test]
    fn parse_oha_summary_reads_counts_and_converts_latency_to_ms() {
        let s = parse_oha_summary(OHA_JSON).expect("parse");
        assert_eq!(s.requests, 600);
        assert!((s.success_rate - 0.98).abs() < f64::EPSILON);
        assert!((s.throughput_rps - 120.5).abs() < f64::EPSILON);
        assert!((s.latency_p50_ms - 40.0).abs() < 1e-9);
        assert!((s.latency_p95_ms - 150.0).abs() < 1e-9);
        assert!((s.latency_p99_ms - 190.0).abs() < 1e-9);
    }

    #[test]
    fn parse_oha_summary_rejects_non_json_and_missing_fields() {
        assert!(parse_oha_summary("not json").is_err());
        let err = parse_oha_summary(r#"{"summary":{"successRate":1.0}}"#).unwrap_err();
        assert!(err.contains("statusCodeDistribution"), "{err}");
    }

    #[test]
    fn oha_duration_uses_whole_seconds_or_milliseconds() {
        assert_eq!(oha_duration(10.0), "10s");
        assert_eq!(oha_duration(1.5), "1500ms");
        assert_eq!(oha_duration(0.25), "250ms");
    }

    fn run(ended_at_ns: i64) -> RunShape {
        RunShape {
            started_at_ns: 0,
            ended_at_ns,
            tool: LoadTool::Oha,
            vus: 20,
        }
    }

    fn thresholds(pairs: &[(&str, f64)]) -> std::collections::HashMap<String, f64> {
        pairs.iter().map(|(k, v)| ((*k).to_string(), *v)).collect()
    }

    fn window(ts_ns: i64, requests: u64, p95_s: f64, rps: f64) -> WindowOutcome {
        WindowOutcome {
            ts_ns,
            duration_s: 10.0,
            vus: 20,
            summary: Ok(OhaSummary {
                requests,
                success_rate: 1.0,
                throughput_rps: rps,
                latency_p50_ms: 10.0,
                latency_p95_ms: p95_s * 1000.0,
                latency_p99_ms: 30.0,
            }),
        }
    }

    #[test]
    fn aggregate_weights_windows_and_keeps_each_as_a_sample() {
        let outcomes = vec![window(1, 100, 0.1, 10.0), window(2, 300, 0.2, 30.0)];
        let result = aggregate(&run(20_000_000_000), outcomes, &thresholds(&[]));
        assert_eq!(result.samples.len(), 2);
        assert_eq!(result.total_requests, 400);
        // Request-weighted p95: (100*100 + 300*200) / 400 = 175 ms.
        assert!((result.latency_p95_ms - 175.0).abs() < 1e-9);
        // Time-weighted throughput over two equal windows: 20 rps.
        assert!((result.throughput_rps - 20.0).abs() < 1e-9);
        assert!(result.thresholds_met);
        assert!((result.duration_s - 20.0).abs() < 1e-9);
    }

    #[test]
    fn aggregate_marks_failed_windows_and_unmet_thresholds() {
        let failed = WindowOutcome {
            ts_ns: 3,
            duration_s: 10.0,
            vus: 20,
            summary: Err("boom".into()),
        };
        let result = aggregate(
            &run(20),
            vec![window(1, 100, 0.1, 10.0), failed],
            &thresholds(&[]),
        );
        assert_eq!(result.samples.len(), 1);
        assert!(!result.thresholds_met);

        let strict = thresholds(&[("latency_p95_ms", 50.0)]);
        let result = aggregate(&run(20), vec![window(1, 100, 0.1, 10.0)], &strict);
        assert!(!result.thresholds_met);

        let unknown = thresholds(&[("bogus", 1.0)]);
        let result = aggregate(&run(20), vec![window(1, 100, 0.1, 10.0)], &unknown);
        assert!(!result.thresholds_met);

        let lenient = thresholds(&[("latency_p95_ms", 500.0), ("throughput_rps", 5.0)]);
        let result = aggregate(&run(20), vec![window(1, 100, 0.1, 10.0)], &lenient);
        assert!(result.thresholds_met);
    }

    #[test]
    fn aggregate_of_no_windows_is_all_zero() {
        let result = aggregate(&run(10), vec![], &thresholds(&[]));
        assert_eq!(result.total_requests, 0);
        assert!(result.samples.is_empty());
        assert!((result.latency_p95_ms).abs() < f64::EPSILON);
        assert!((result.throughput_rps).abs() < f64::EPSILON);
    }
}
