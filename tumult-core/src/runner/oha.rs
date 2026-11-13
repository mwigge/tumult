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
//!
//! `stop` lets the in-flight window finish (the first window always runs)
//! and skips the windows not yet started; it waits at most the plan's
//! remaining time plus the window grace before killing the process. A run
//! that is cut before any window completed is an error, never a passing
//! result, and a run with no completed window never meets its thresholds.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{load_profile, LoadExecutor, LoadHandle};
use crate::types::{LoadConfig, LoadResult, LoadSample, LoadTool};

/// Grace added to a window's planned length before its process is killed,
/// and to the plan's remaining time before `stop` gives up waiting.
pub const DEFAULT_WINDOW_GRACE: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// oha load executor.
///
/// The binary is resolved from `TUMULT_OHA_BINARY`, falling back to `oha`
/// on `PATH`. `LoadConfig::script` carries the URL to hit.
#[derive(Debug, Clone)]
pub struct OhaLoadExecutor {
    window_grace: Duration,
}

impl Default for OhaLoadExecutor {
    fn default() -> Self {
        Self {
            window_grace: DEFAULT_WINDOW_GRACE,
        }
    }
}

impl OhaLoadExecutor {
    /// An executor whose windows are killed `grace` after their planned
    /// length (tests use a short one; production keeps the default).
    #[must_use]
    pub fn with_window_grace(grace: Duration) -> Self {
        Self {
            window_grace: grace,
        }
    }
}

/// What one window produced.
#[derive(Debug)]
struct WindowOutcome {
    ts_ns: i64,
    duration_s: f64,
    vus: u32,
    summary: Result<OhaSummary, String>,
}

type CurrentChild = Arc<Mutex<Option<std::process::Child>>>;

struct OhaHandle {
    stop: Arc<AtomicBool>,
    current: CurrentChild,
    /// `None` once `stop` has taken it.
    worker: Option<std::thread::JoinHandle<Vec<WindowOutcome>>>,
    started_at: Instant,
    started_at_ns: i64,
    plan_total_s: f64,
    grace: Duration,
    tool: LoadTool,
    vus: u32,
    thresholds: std::collections::HashMap<String, f64>,
}

impl Drop for OhaHandle {
    /// A handle dropped without `stop` (a runner error path) must not leave
    /// an oha process or the worker behind.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(child) = lock(&self.current).as_mut() {
            kill_tree(child);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// The fields of an oha `--json` summary the adapter keeps. Latencies are
/// converted from oha's seconds to milliseconds. `errors` counts transport
/// failures (`errorDistribution`) and every non-2xx/3xx answer, so the
/// error rate matches what a caller would see, unlike oha's own
/// `successRate` which only knows about transport.
#[derive(Debug, Clone, PartialEq)]
pub struct OhaSummary {
    pub requests: u64,
    pub errors: u64,
    pub throughput_rps: f64,
    pub latency_p50_ms: f64,
    pub latency_p95_ms: f64,
    pub latency_p99_ms: f64,
}

impl OhaSummary {
    /// `errors / requests`, zero when nothing was sent.
    #[must_use]
    pub fn error_rate(&self) -> f64 {
        if self.requests == 0 {
            return 0.0;
        }
        // Request counts are far below 2^53.
        #[allow(clippy::cast_precision_loss)]
        {
            (self.errors as f64 / self.requests as f64).clamp(0.0, 1.0)
        }
    }
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
    let statuses = v
        .get("statusCodeDistribution")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "oha summary lacks statusCodeDistribution".to_string())?;
    let mut requests = 0;
    let mut errors = 0;
    for (code, count) in statuses {
        let count = count.as_u64().unwrap_or(0);
        requests += count;
        if !code.parse::<u16>().is_ok_and(|c| (200..400).contains(&c)) {
            errors += count;
        }
    }
    let transport: u64 = v
        .get("errorDistribution")
        .and_then(serde_json::Value::as_object)
        .map(|m| m.values().filter_map(serde_json::Value::as_u64).sum())
        .unwrap_or(0);
    Ok(OhaSummary {
        requests: requests + transport,
        errors: errors + transport,
        throughput_rps: number(&["summary", "requestsPerSec"])?,
        latency_p50_ms: number(&["latencyPercentiles", "p50"])? * 1000.0,
        latency_p95_ms: number(&["latencyPercentiles", "p95"])? * 1000.0,
        latency_p99_ms: number(&["latencyPercentiles", "p99"])? * 1000.0,
    })
}

/// oha's `-z` duration: whole seconds as `Ns`, anything else in `ms`,
/// never below one millisecond.
fn oha_duration(seconds: f64) -> String {
    if seconds >= 1.0 && seconds.fract() == 0.0 {
        return format!("{seconds:.0}s");
    }
    let ms = (seconds * 1000.0).round().max(1.0);
    format!("{ms:.0}ms")
}

/// Spawn one window's process and wait for it, bounded by the window's
/// length plus `grace`. The child is parked in `current` while it runs so
/// `stop()` (or `Drop`) can kill it.
fn run_window(
    binary: &str,
    url: &std::ffi::OsStr,
    window: &load_profile::Window,
    current: &Mutex<Option<std::process::Child>>,
    grace: Duration,
) -> Result<OhaSummary, String> {
    let mut cmd = std::process::Command::new(binary);
    #[cfg(unix)]
    {
        // Its own process group, so a kill reaches every process oha (or a
        // wrapper script) spawned, not only the leader.
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    }
    let child = cmd
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
    let deadline = Instant::now() + Duration::from_secs_f64(window.duration_s.max(0.0)) + grace;
    let output = wait_parked(child, current, deadline)?;
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

/// Park `child` in `current`, drain its pipes on reader threads, and poll
/// until it exits or `deadline` passes (then kill and reap it).
fn wait_parked(
    mut child: std::process::Child,
    current: &Mutex<Option<std::process::Child>>,
    deadline: Instant,
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
                Ok(None) if Instant::now() >= deadline => {
                    kill_tree(child);
                    Err("oha did not exit within its window plus grace; killed".to_string())
                }
                Ok(None) => Ok(None),
                Err(e) => {
                    // The wait itself failed: make sure nothing is left
                    // running or unreaped before reporting.
                    kill_tree(child);
                    Err(format!("oha wait failed: {e}"))
                }
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
                // Killed or unwaitable: the pipes may still be held by a
                // straggler, so the drain threads are left to finish on
                // their own rather than blocking the adapter.
                break Err(e);
            }
        }
    };
    let status = status?;
    let stdout = stdout_thread.map_or_else(Vec::new, |t| t.join().unwrap_or_default());
    let stderr = stderr_thread.map_or_else(Vec::new, |t| t.join().unwrap_or_default());
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

/// Kill the window's whole process group (Unix) or the leader (elsewhere)
/// and reap the leader. The pipes may stay open until every descendant is
/// gone, so callers must not block on draining them afterwards.
fn kill_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        if let Ok(pid) = i32::try_from(child.id()) {
            // SAFETY: plain syscall on a process group this adapter created.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
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
        let current: CurrentChild = Arc::new(Mutex::new(None));
        let grace = self.window_grace;

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

        let started_at = Instant::now();
        let started_at_ns = super::epoch_nanos_now();
        let window_count = windows.len();
        let worker = {
            let stop = Arc::clone(&stop);
            let current = Arc::clone(&current);
            std::thread::spawn(move || {
                let mut outcomes = Vec::with_capacity(windows.len());
                for (i, window) in windows.into_iter().enumerate() {
                    // The first window always runs so a started load
                    // leaves evidence; `stop` skips the ones after the
                    // in-flight window.
                    if i > 0 && stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let ts_ns = super::epoch_nanos_now();
                    let summary = run_window(&binary, &url, &window, &current, grace);
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
            windows = window_count,
            peak_vus = plan.peak_vus(),
            total_s = plan.total_duration_s(),
            "oha load plan started"
        );
        Ok(LoadHandle {
            inner: Box::new(OhaHandle {
                stop,
                current,
                worker: Some(worker),
                started_at,
                started_at_ns,
                plan_total_s: plan.total_duration_s(),
                grace,
                tool: config.tool.clone(),
                vus: plan.peak_vus(),
                thresholds: config.thresholds.clone(),
            }),
        })
    }

    fn stop(&self, handle: LoadHandle) -> Result<LoadResult, String> {
        let mut oha: Box<OhaHandle> = handle
            .inner
            .downcast::<OhaHandle>()
            .map_err(|_| "invalid load handle")?;
        oha.stop.store(true, Ordering::SeqCst);
        let worker = oha
            .worker
            .take()
            .ok_or_else(|| "oha handle was already stopped".to_string())?;
        // Let the plan run out (minus the windows we just told it to
        // skip). The in-flight window kills its own process one grace
        // after its length, so the worker is only interrupted here when
        // it is hung a further grace beyond that.
        let remaining = Duration::from_secs_f64(oha.plan_total_s.max(0.0))
            .saturating_sub(oha.started_at.elapsed());
        let deadline = Instant::now() + remaining + oha.grace + oha.grace;
        while !worker.is_finished() && Instant::now() < deadline {
            std::thread::sleep(POLL_INTERVAL);
        }
        let interrupted = !worker.is_finished();
        if interrupted {
            if let Some(child) = lock(&oha.current).as_mut() {
                kill_tree(child);
            }
        }
        let outcomes = worker
            .join()
            .map_err(|_| "oha worker thread panicked".to_string())?;
        let outcomes = finish(outcomes, interrupted)?;
        let ended_at_ns = super::epoch_nanos_now();
        let run = RunShape {
            started_at_ns: oha.started_at_ns,
            ended_at_ns,
            tool: oha.tool.clone(),
            vus: oha.vus,
        };
        Ok(aggregate(&run, outcomes, &oha.thresholds))
    }
}

/// What `stop` keeps of the worker's outcomes. A window cut short by the
/// kill is dropped only if it failed (a window that still produced a
/// summary is evidence); a run interrupted before any window completed
/// is an error, like a k6 that never exited cleanly.
fn finish(
    mut outcomes: Vec<WindowOutcome>,
    interrupted: bool,
) -> Result<Vec<WindowOutcome>, String> {
    if interrupted && outcomes.last().is_some_and(|o| o.summary.is_err()) {
        outcomes.pop();
    }
    let completed = outcomes.iter().any(|o| o.summary.is_ok());
    if interrupted && !completed {
        return Err("oha run was cut before any window completed".into());
    }
    Ok(outcomes)
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
/// latencies), and every window with a summary becomes a sample. A run
/// with no completed window never meets its thresholds.
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
                error_rate: summary.error_rate(),
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
    if samples.is_empty() {
        tracing::warn!("oha: no window completed; thresholds reported unmet");
    }
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
        thresholds_met: all_windows_ok && !samples.is_empty(),
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

    const OHA_JSON: &str = r#"{"summary":{"successRate":0.98,"total":5.0,"slowest":0.2,"fastest":0.01,"average":0.05,"requestsPerSec":120.5,"totalData":100,"sizePerRequest":10,"sizePerSec":100},"responseTimeHistogram":{},"latencyPercentiles":{"p10":0.01,"p25":0.02,"p50":0.04,"p75":0.06,"p90":0.09,"p95":0.15,"p99":0.19,"p99.9":0.2,"p99.99":0.2},"rps":{"mean":120.5,"stddev":1.0,"max":130.0,"min":110.0,"percentiles":{}},"details":{},"statusCodeDistribution":{"200":580,"302":8,"500":2,"429":10},"errorDistribution":{"connection refused":10}}"#;

    #[test]
    fn parse_oha_summary_counts_transport_and_http_errors() {
        let s = parse_oha_summary(OHA_JSON).expect("parse");
        // 580 + 8 + 2 + 10 status rows plus 10 transport failures.
        assert_eq!(s.requests, 610);
        // 500 and 429 answers plus the transport failures; 302 is fine.
        assert_eq!(s.errors, 22);
        assert!((s.error_rate() - 22.0 / 610.0).abs() < 1e-12);
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
    fn error_rate_of_an_empty_window_is_zero() {
        let s = OhaSummary {
            requests: 0,
            errors: 0,
            throughput_rps: 0.0,
            latency_p50_ms: 0.0,
            latency_p95_ms: 0.0,
            latency_p99_ms: 0.0,
        };
        assert!((s.error_rate()).abs() < f64::EPSILON);
    }

    #[test]
    fn oha_duration_uses_whole_seconds_or_milliseconds_never_below_one() {
        assert_eq!(oha_duration(10.0), "10s");
        assert_eq!(oha_duration(1.5), "1500ms");
        assert_eq!(oha_duration(0.25), "250ms");
        assert_eq!(oha_duration(0.0), "1ms");
        assert_eq!(oha_duration(0.0001), "1ms");
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
                errors: 0,
                throughput_rps: rps,
                latency_p50_ms: 10.0,
                latency_p95_ms: p95_s * 1000.0,
                latency_p99_ms: 30.0,
            }),
        }
    }

    fn failed(ts_ns: i64) -> WindowOutcome {
        WindowOutcome {
            ts_ns,
            duration_s: 10.0,
            vus: 20,
            summary: Err("boom".into()),
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
        let result = aggregate(
            &run(20),
            vec![window(1, 100, 0.1, 10.0), failed(3)],
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
    fn aggregate_of_no_windows_never_meets_thresholds() {
        let result = aggregate(&run(10), vec![], &thresholds(&[]));
        assert_eq!(result.total_requests, 0);
        assert!(result.samples.is_empty());
        assert!(!result.thresholds_met, "no evidence must not pass");
        assert!((result.latency_p95_ms).abs() < f64::EPSILON);
        assert!((result.throughput_rps).abs() < f64::EPSILON);
    }

    #[test]
    fn finish_errors_when_interrupted_before_any_window_completed() {
        let err = finish(vec![], true).unwrap_err();
        assert!(err.contains("cut before any window completed"), "{err}");
        let err = finish(vec![failed(1)], true).unwrap_err();
        assert!(err.contains("cut before"), "{err}");
    }

    #[test]
    fn finish_drops_only_a_failed_in_flight_window() {
        let kept = finish(vec![window(1, 10, 0.1, 1.0), failed(2)], true).unwrap();
        assert_eq!(kept.len(), 1);
        let kept = finish(vec![window(1, 10, 0.1, 1.0), window(2, 10, 0.1, 1.0)], true).unwrap();
        assert_eq!(
            kept.len(),
            2,
            "a completed window is evidence even on interrupt"
        );
        let kept = finish(vec![failed(1)], false).unwrap();
        assert_eq!(
            kept.len(),
            1,
            "an uninterrupted failure is reported, not dropped"
        );
    }
}
