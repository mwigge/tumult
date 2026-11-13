//! `oha` load adapter — the second [`LoadExecutor`] behind the same
//! boundary as [`super::k6`], proving a profile means the same thing
//! whichever tool runs it.
//!
//! oha cannot shape traffic over time, so the adapter runs the plan's
//! [`load_profile::windows`] one process at a time
//! (`-c <vus> -z <len> -w -t <timeout>`), parses each JSON summary into a
//! [`LoadSample`], and aggregates the windows into the run's
//! [`LoadResult`]. A soak profile with `sample_interval_s` therefore
//! streams one sample per interval; a flat config is a single window and a
//! single summary, as with k6.
//!
//! Every window runs with `-w`, so a request still in flight when the
//! window's time is up is waited for and measured rather than dropped, and
//! with `-t`, so a request that never answers is counted as a timeout
//! error. A window may therefore outlast its planned length by up to the
//! request timeout.
//!
//! There are two ways to end a run:
//!
//! - [`LoadExecutor::stop`] (the method finished) lets the in-flight
//!   window finish — the first window always runs — and skips the windows
//!   not yet started.
//! - [`LoadExecutor::abort`] (a guard halted the run, or it was
//!   interrupted) kills the in-flight window's process group at once and
//!   keeps the windows that had completed. A run aborted before any window
//!   completed is an error, never a passing result.
//!
//! A run with no completed window, or one that sent no request, never
//! meets its thresholds.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{load_profile, LoadExecutor, LoadHandle};
use crate::types::{LoadConfig, LoadResult, LoadSample, LoadTool};

/// Slack added to a window's planned length plus the request timeout
/// before its process is killed.
pub const DEFAULT_WINDOW_GRACE: Duration = Duration::from_secs(10);
/// How long oha waits for one answer before counting the request as a
/// timeout error. Overridden by `TUMULT_OHA_REQUEST_TIMEOUT_S`.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(50);
/// How long a finished window's output pipes may stay open before the
/// window is failed instead of read.
const DRAIN_WAIT: Duration = Duration::from_secs(2);

/// oha load executor.
///
/// The binary is resolved from `TUMULT_OHA_BINARY`, falling back to `oha`
/// on `PATH`. `LoadConfig::script` carries the URL to hit.
#[derive(Debug, Clone)]
pub struct OhaLoadExecutor {
    window_grace: Duration,
    /// `None` reads `TUMULT_OHA_REQUEST_TIMEOUT_S`, then the default.
    request_timeout: Option<Duration>,
}

impl Default for OhaLoadExecutor {
    fn default() -> Self {
        Self {
            window_grace: DEFAULT_WINDOW_GRACE,
            request_timeout: None,
        }
    }
}

impl OhaLoadExecutor {
    /// An executor whose windows are killed `grace` after their planned
    /// length plus the request timeout (tests use a short one; production
    /// keeps the default).
    #[must_use]
    pub fn with_window_grace(grace: Duration) -> Self {
        Self {
            window_grace: grace,
            request_timeout: None,
        }
    }

    /// Fix the per-request timeout instead of reading the environment.
    #[must_use]
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = Some(timeout);
        self
    }

    fn resolved_request_timeout(&self) -> Result<Duration, String> {
        if let Some(timeout) = self.request_timeout {
            return Ok(timeout);
        }
        match std::env::var("TUMULT_OHA_REQUEST_TIMEOUT_S") {
            Ok(text) => request_timeout_from(&text),
            Err(_) => Ok(DEFAULT_REQUEST_TIMEOUT),
        }
    }
}

/// Parse `TUMULT_OHA_REQUEST_TIMEOUT_S`: finite seconds above zero.
fn request_timeout_from(text: &str) -> Result<Duration, String> {
    let invalid = || {
        format!("TUMULT_OHA_REQUEST_TIMEOUT_S must be a positive number of seconds, got {text:?}")
    };
    let seconds: f64 = text.trim().parse().map_err(|_| invalid())?;
    if !(seconds.is_finite() && seconds > 0.0) {
        return Err(invalid());
    }
    Duration::try_from_secs_f64(seconds).map_err(|_| invalid())
}

/// What one window produced.
#[derive(Debug)]
struct WindowOutcome {
    ts_ns: i64,
    duration_s: f64,
    vus: u32,
    summary: Result<OhaSummary, String>,
}

/// The two ways a run is told to end, shared with the worker thread.
#[derive(Debug, Default)]
struct Control {
    /// Finish the in-flight window, start no further one.
    stop: AtomicBool,
    /// Kill the in-flight window now and start no further one.
    abort: AtomicBool,
}

struct OhaHandle {
    control: Arc<Control>,
    /// `None` once `stop` or `abort` has taken it.
    worker: Option<std::thread::JoinHandle<Vec<WindowOutcome>>>,
    started_at_ns: i64,
    tool: LoadTool,
    vus: u32,
    thresholds: std::collections::HashMap<String, f64>,
}

impl Drop for OhaHandle {
    /// A handle dropped without `stop` (a runner error path) must not leave
    /// an oha process or the worker behind. The worker sees the abort flag
    /// within one poll interval, whether or not its first process is up.
    fn drop(&mut self) {
        self.control.abort.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                tracing::error!("oha worker thread panicked");
            }
        }
    }
}

/// The latency percentiles of the answered requests, in milliseconds.
#[derive(Debug, Clone, PartialEq)]
pub struct OhaLatency {
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
}

/// The fields of an oha JSON summary the adapter keeps. `errors` counts
/// transport failures (`errorDistribution`, timeouts included) and every
/// non-2xx/3xx answer, so the error rate matches what a caller would see,
/// unlike oha's own `successRate` which only knows about transport.
/// `throughput_rps` is answered requests per second of the window's real
/// length. `latency` is `None` when no request was answered.
#[derive(Debug, Clone, PartialEq)]
pub struct OhaSummary {
    pub requests: u64,
    pub errors: u64,
    pub throughput_rps: f64,
    pub latency: Option<OhaLatency>,
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

/// The `errorDistribution` key oha files the requests under that were still
/// in flight when the run's `-z` deadline passed. With `-w` there should be
/// none; at most one per connection is how a run ends, never more.
const DEADLINE_ABORT: &str = "aborted due to deadline";

/// The counts of one of oha's `{"<key>": <count>}` objects. A missing
/// object or a value that is not a whole number is an error, never a
/// silent zero.
fn counts(v: &serde_json::Value, field: &str) -> Result<Vec<(String, u64)>, String> {
    let map = v
        .get(field)
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| format!("oha summary lacks {field}"))?;
    map.iter()
        .map(|(key, count)| {
            let count = count
                .as_u64()
                .ok_or_else(|| format!("oha summary {field}[{key:?}] is not a count: {count}"))?;
            Ok((key.clone(), count))
        })
        .collect()
}

/// A number at `path`, or `None` when oha printed `null` there (it does
/// for every latency figure when no request was answered).
fn optional_number(v: &serde_json::Value, path: &[&str]) -> Result<Option<f64>, String> {
    let mut cur = v;
    for key in path {
        cur = cur
            .get(key)
            .ok_or_else(|| format!("oha summary lacks {}", path.join(".")))?;
    }
    if cur.is_null() {
        return Ok(None);
    }
    cur.as_f64()
        .map(Some)
        .ok_or_else(|| format!("oha summary {} is not a number", path.join(".")))
}

fn number(v: &serde_json::Value, path: &[&str]) -> Result<f64, String> {
    optional_number(v, path)?.ok_or_else(|| format!("oha summary {} is null", path.join(".")))
}

/// The percentiles of the answered requests: all present when anything
/// was answered, `None` when nothing was.
fn latency(v: &serde_json::Value, answered: u64) -> Result<Option<OhaLatency>, String> {
    if answered == 0 {
        return Ok(None);
    }
    let ms = |name: &str| number(v, &["latencyPercentiles", name]).map(|s| s * 1000.0);
    Ok(Some(OhaLatency {
        p50_ms: ms("p50")?,
        p95_ms: ms("p95")?,
        p99_ms: ms("p99")?,
    }))
}

/// Parse oha's JSON output (`oha --output-format json`) for a window run
/// on `connections` connections. Every field the adapter needs must be
/// present; a missing or malformed one is an error rather than a silent
/// zero.
///
/// Deadline aborts up to one per connection are how a window without `-w`
/// ends and are not requests; any beyond that are counted as errors.
///
/// # Errors
///
/// Returns an error when the text is not JSON, lacks `summary.total`,
/// `statusCodeDistribution` or `errorDistribution`, carries a count that
/// is not a whole number, or lacks a percentile although requests were
/// answered.
pub fn parse_oha_summary(text: &str, connections: u32) -> Result<OhaSummary, String> {
    let v: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("oha output is not JSON: {e}"))?;
    let mut answered = 0;
    let mut errors = 0;
    for (code, count) in counts(&v, "statusCodeDistribution")? {
        answered += count;
        if !code.parse::<u16>().is_ok_and(|c| (200..400).contains(&c)) {
            errors += count;
        }
    }
    let mut transport = 0;
    for (reason, count) in counts(&v, "errorDistribution")? {
        transport += if reason == DEADLINE_ABORT {
            count.saturating_sub(u64::from(connections))
        } else {
            count
        };
    }
    let total_s = number(&v, &["summary", "total"])?;
    // Request counts are far below 2^53.
    #[allow(clippy::cast_precision_loss)]
    let throughput_rps = if total_s > 0.0 {
        answered as f64 / total_s
    } else {
        0.0
    };
    Ok(OhaSummary {
        requests: answered + transport,
        errors: errors + transport,
        throughput_rps,
        latency: latency(&v, answered)?,
    })
}

/// A duration in oha's vocabulary (`-z`, `-t`): whole seconds as `Ns`,
/// anything else in `ms`, never below one millisecond.
fn oha_duration(seconds: f64) -> String {
    if seconds >= 1.0 && seconds.fract() == 0.0 {
        return format!("{seconds:.0}s");
    }
    let ms = (seconds * 1000.0).round().max(1.0);
    format!("{ms:.0}ms")
}

/// What every window of one run shares.
struct WindowRunner {
    binary: String,
    url: std::ffi::OsString,
    request_timeout: Duration,
    grace: Duration,
    control: Arc<Control>,
}

impl WindowRunner {
    /// Run the plan's windows in order until it is exhausted or the run
    /// is told to end.
    fn run(&self, windows: Vec<load_profile::Window>) -> Vec<WindowOutcome> {
        let mut outcomes = Vec::with_capacity(windows.len());
        for (i, window) in windows.into_iter().enumerate() {
            if self.control.abort.load(Ordering::SeqCst) {
                break;
            }
            // The first window always runs so a load that was stopped
            // normally leaves evidence; `stop` skips the ones after the
            // in-flight window.
            if i > 0 && self.control.stop.load(Ordering::SeqCst) {
                break;
            }
            let ts_ns = super::epoch_nanos_now();
            let summary = self.run_window(&window);
            outcomes.push(WindowOutcome {
                ts_ns,
                duration_s: window.duration_s,
                vus: window.vus,
                summary,
            });
        }
        outcomes
    }

    /// Spawn one window's process and wait for it, bounded by the
    /// window's length plus the request timeout plus grace.
    fn run_window(&self, window: &load_profile::Window) -> Result<OhaSummary, String> {
        let mut cmd = std::process::Command::new(&self.binary);
        #[cfg(unix)]
        {
            // Its own process group, so a kill reaches every process oha
            // (or a wrapper script) spawned, not only the leader.
            std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        }
        let child = cmd
            .arg("--no-tui")
            .arg("--output-format")
            .arg("json")
            .arg("-c")
            .arg(window.vus.to_string())
            .arg("-z")
            .arg(oha_duration(window.duration_s))
            // Wait for the requests in flight at the deadline, so a slow
            // answer is measured instead of dropped …
            .arg("-w")
            // … and give up on one that never comes, as a timeout error.
            .arg("-t")
            .arg(oha_duration(self.request_timeout.as_secs_f64()))
            .arg(&self.url)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("failed to start oha: {e}"))?;
        let deadline = Instant::now()
            + Duration::from_secs_f64(window.duration_s.max(0.0))
            + self.request_timeout
            + self.grace;
        let output = wait_bounded(child, deadline, &self.control.abort)?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "oha exited with {}: {}",
                output.status,
                stderr.trim()
            ));
        }
        parse_oha_summary(&String::from_utf8_lossy(&output.stdout), window.vus)
    }
}

/// Read `reader` to its end on a thread; the bytes arrive on the channel
/// once every writer has closed the pipe.
fn drain<R: std::io::Read + Send + 'static>(
    mut reader: R,
) -> std::sync::mpsc::Receiver<std::io::Result<Vec<u8>>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let read = reader.read_to_end(&mut buf).map(|_| buf);
        // The receiver is gone when the window was killed or failed.
        let _ = tx.send(read);
    });
    rx
}

/// What a drained pipe held, waiting until `deadline` at most: a process
/// that escaped the group may hold the pipe open long after the window.
fn drained(
    pipe: Option<std::sync::mpsc::Receiver<std::io::Result<Vec<u8>>>>,
    deadline: Instant,
) -> Result<Vec<u8>, String> {
    let Some(pipe) = pipe else {
        return Ok(Vec::new());
    };
    let wait = deadline.saturating_duration_since(Instant::now());
    match pipe.recv_timeout(wait) {
        Ok(Ok(bytes)) => Ok(bytes),
        Ok(Err(e)) => Err(format!("reading oha output failed: {e}")),
        Err(_) => Err("oha exited but its output pipe is still held open".into()),
    }
}

/// Wait for `child` while draining its pipes: until it exits, `deadline`
/// passes, or `abort` is raised (then its process group is killed). Polled
/// from the moment the child exists, so an abort raised before the spawn
/// is seen on the first turn.
fn wait_bounded(
    mut child: std::process::Child,
    deadline: Instant,
    abort: &AtomicBool,
) -> Result<std::process::Output, String> {
    let stdout = child.stdout.take().map(drain);
    let stderr = child.stderr.take().map(drain);
    let status = loop {
        if abort.load(Ordering::SeqCst) {
            kill_tree(&mut child);
            return Err("oha window aborted".into());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                kill_tree(&mut child);
                return Err(
                    "oha did not exit within its window, request timeout and grace; killed".into(),
                );
            }
            Ok(None) => std::thread::sleep(POLL_INTERVAL),
            Err(e) => {
                // The wait itself failed: make sure nothing is left
                // running or unreaped before reporting.
                kill_tree(&mut child);
                return Err(format!("oha wait failed: {e}"));
            }
        }
    };
    // The leader is gone; anything it left behind in its group would keep
    // the pipes open, so it goes too.
    kill_group(child.id());
    let drain_deadline = Instant::now() + DRAIN_WAIT;
    Ok(std::process::Output {
        status,
        stdout: drained(stdout, drain_deadline)?,
        stderr: drained(stderr, drain_deadline)?,
    })
}

/// SIGKILL the process group led by `pid` (Unix only; a no-op elsewhere).
fn kill_group(pid: u32) {
    #[cfg(unix)]
    {
        if let Ok(pid) = i32::try_from(pid) {
            // SAFETY: plain syscall on a process group this adapter created.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
    }
}

/// Kill the window's whole process group (Unix) or the leader (elsewhere)
/// and reap the leader. The pipes may stay open until every descendant is
/// gone, so callers must not block on draining them afterwards.
fn kill_tree(child: &mut std::process::Child) {
    kill_group(child.id());
    // Both fail only when the process is already gone and reaped.
    let _ = child.kill();
    let _ = child.wait();
}

/// How a run was told to end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    Stop,
    Abort,
}

impl OhaLoadExecutor {
    /// Raise the flag for `end`, wait for the worker and fold what it
    /// produced. The wait is bounded by the worker itself: an in-flight
    /// window kills its own process at its deadline, and an aborted one
    /// within a poll interval.
    fn end(handle: LoadHandle, end: End) -> Result<LoadResult, String> {
        let mut oha: Box<OhaHandle> = handle
            .inner
            .downcast::<OhaHandle>()
            .map_err(|_| "invalid load handle")?;
        let flag = match end {
            End::Stop => &oha.control.stop,
            End::Abort => &oha.control.abort,
        };
        flag.store(true, Ordering::SeqCst);
        let worker = oha
            .worker
            .take()
            .ok_or_else(|| "oha handle was already stopped".to_string())?;
        let outcomes = worker
            .join()
            .map_err(|_| "oha worker thread panicked".to_string())?;
        let outcomes = finish(outcomes, end == End::Abort)?;
        let run = RunShape {
            started_at_ns: oha.started_at_ns,
            ended_at_ns: super::epoch_nanos_now(),
            tool: oha.tool.clone(),
            vus: oha.vus,
        };
        Ok(aggregate(&run, outcomes, &oha.thresholds))
    }
}

impl LoadExecutor for OhaLoadExecutor {
    fn start(&self, config: &LoadConfig) -> Result<LoadHandle, String> {
        let binary = std::env::var("TUMULT_OHA_BINARY").unwrap_or_else(|_| "oha".to_string());
        let request_timeout = self.resolved_request_timeout()?;
        let plan = load_profile::plan(config);
        let windows = load_profile::windows(&plan);

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

        let control = Arc::new(Control::default());
        let runner = WindowRunner {
            binary,
            url: config.script.as_os_str().to_owned(),
            request_timeout,
            grace: self.window_grace,
            control: Arc::clone(&control),
        };
        let started_at_ns = super::epoch_nanos_now();
        let window_count = windows.len();
        let worker = std::thread::spawn(move || runner.run(windows));
        tracing::info!(
            windows = window_count,
            peak_vus = plan.peak_vus(),
            total_s = plan.total_duration_s(),
            request_timeout_s = request_timeout.as_secs_f64(),
            "oha load plan started"
        );
        Ok(LoadHandle {
            inner: Box::new(OhaHandle {
                control,
                worker: Some(worker),
                started_at_ns,
                tool: config.tool.clone(),
                vus: plan.peak_vus(),
                thresholds: config.thresholds.clone(),
            }),
        })
    }

    fn stop(&self, handle: LoadHandle) -> Result<LoadResult, String> {
        Self::end(handle, End::Stop)
    }

    fn abort(&self, handle: LoadHandle) -> Result<LoadResult, String> {
        Self::end(handle, End::Abort)
    }
}

/// What the run keeps of the worker's outcomes. The window an abort cut
/// short is dropped (it failed because of the kill, not the target); an
/// aborted run with no completed window is an error, like a k6 that never
/// exited cleanly.
fn finish(mut outcomes: Vec<WindowOutcome>, aborted: bool) -> Result<Vec<WindowOutcome>, String> {
    if aborted && outcomes.last().is_some_and(|o| o.summary.is_err()) {
        outcomes.pop();
    }
    let completed = outcomes.iter().any(|o| o.summary.is_ok());
    if aborted && !completed {
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

/// The windows folded together, before thresholds are applied.
#[derive(Default)]
struct Folded {
    samples: Vec<LoadSample>,
    requests: u64,
    errors: u64,
    /// Seconds and answered-request seconds of the windows with a summary.
    time_s: f64,
    answered: f64,
    /// False once a window failed or answered nothing.
    all_windows_ok: bool,
}

fn fold(outcomes: Vec<WindowOutcome>) -> Folded {
    let mut folded = Folded {
        all_windows_ok: true,
        ..Folded::default()
    };
    for outcome in outcomes {
        let summary = match outcome.summary {
            Ok(summary) => summary,
            Err(e) => {
                folded.all_windows_ok = false;
                tracing::warn!(error = %e, vus = outcome.vus, "oha window failed");
                continue;
            }
        };
        folded.requests += summary.requests;
        folded.errors += summary.errors;
        folded.time_s += outcome.duration_s;
        folded.answered += summary.throughput_rps * outcome.duration_s;
        let Some(latency) = summary.latency.as_ref() else {
            folded.all_windows_ok = false;
            tracing::warn!(
                requests = summary.requests,
                vus = outcome.vus,
                "oha window answered no request"
            );
            continue;
        };
        folded.samples.push(LoadSample {
            ts_ns: outcome.ts_ns,
            duration_s: outcome.duration_s,
            vus: outcome.vus,
            throughput_rps: summary.throughput_rps,
            latency_p50_ms: latency.p50_ms,
            latency_p95_ms: latency.p95_ms,
            latency_p99_ms: latency.p99_ms,
            error_rate: summary.error_rate(),
            requests: summary.requests,
        });
    }
    folded
}

/// Fold the windows into one [`LoadResult`]. Throughput is time-weighted,
/// latency percentiles are request-weighted means of the window
/// percentiles (an approximation, since oha does not expose raw
/// latencies), the error rate is all errors over all requests, and every
/// window that answered something becomes a sample. A run with a failed
/// or unanswered window, no completed window, or no request at all never
/// meets its thresholds.
fn aggregate(
    run: &RunShape,
    outcomes: Vec<WindowOutcome>,
    thresholds: &std::collections::HashMap<String, f64>,
) -> LoadResult {
    let folded = fold(outcomes);
    let samples = folded.samples;
    #[allow(clippy::cast_precision_loss)]
    let elapsed_s = (run.ended_at_ns - run.started_at_ns) as f64 / 1_000_000_000.0;
    let sampled_requests: u64 = samples.iter().map(|s| s.requests).sum();
    let weighted = |f: fn(&LoadSample) -> f64| -> f64 {
        if sampled_requests == 0 {
            return 0.0;
        }
        #[allow(clippy::cast_precision_loss)]
        let sum: f64 = samples.iter().map(|s| f(s) * s.requests as f64).sum();
        #[allow(clippy::cast_precision_loss)]
        {
            sum / sampled_requests as f64
        }
    };
    let throughput_rps = if folded.time_s > 0.0 {
        folded.answered / folded.time_s
    } else {
        0.0
    };
    #[allow(clippy::cast_precision_loss)]
    let error_rate = if folded.requests > 0 {
        (folded.errors as f64 / folded.requests as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let has_evidence = !samples.is_empty() && folded.requests > 0;
    if !has_evidence {
        tracing::warn!("oha: no window answered a request; thresholds reported unmet");
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
        error_rate,
        total_requests: folded.requests,
        thresholds_met: folded.all_windows_ok && has_evidence,
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

    const OHA_JSON: &str = r#"{"summary":{"successRate":0.98,"total":5.0,"slowest":0.2,"fastest":0.01,"average":0.05,"requestsPerSec":122.0,"totalData":100,"sizePerRequest":10,"sizePerSec":100},"responseTimeHistogram":{},"latencyPercentiles":{"p10":0.01,"p25":0.02,"p50":0.04,"p75":0.06,"p90":0.09,"p95":0.15,"p99":0.19,"p99.9":0.2,"p99.99":0.2},"rps":{"mean":120.5,"stddev":1.0,"max":130.0,"min":110.0,"percentiles":{}},"details":{},"statusCodeDistribution":{"200":580,"302":8,"500":2,"429":10},"errorDistribution":{"connection refused":10}}"#;

    #[test]
    fn parse_oha_summary_counts_transport_and_http_errors() {
        let s = parse_oha_summary(OHA_JSON, 4).expect("parse");
        // 580 + 8 + 2 + 10 status rows plus 10 transport failures.
        assert_eq!(s.requests, 610);
        // 500 and 429 answers plus the transport failures; 302 is fine.
        assert_eq!(s.errors, 22);
        assert!((s.error_rate() - 22.0 / 610.0).abs() < 1e-12);
        // Answered requests over the window's real length: 600 / 5 s.
        assert!((s.throughput_rps - 120.0).abs() < 1e-9);
        let latency = s.latency.expect("answered requests have a latency");
        assert!((latency.p50_ms - 40.0).abs() < 1e-9);
        assert!((latency.p95_ms - 150.0).abs() < 1e-9);
        assert!((latency.p99_ms - 190.0).abs() < 1e-9);
    }

    fn with_errors(errors: &str) -> String {
        format!(
            r#"{{"summary":{{"total":0.5}},
            "latencyPercentiles":{{"p50":0.043,"p95":0.044,"p99":0.044}},
            "statusCodeDistribution":{{"200":45}},
            "errorDistribution":{errors}}}"#
        )
    }

    #[test]
    fn one_deadline_abort_per_connection_is_how_a_window_ends() {
        let text = with_errors(r#"{"aborted due to deadline":4,"connection refused":2}"#);
        let s = parse_oha_summary(&text, 4).expect("parse");
        assert_eq!(s.requests, 47);
        assert_eq!(s.errors, 2);
    }

    #[test]
    fn deadline_aborts_beyond_one_per_connection_are_errors() {
        let text = with_errors(r#"{"aborted due to deadline":10}"#);
        let s = parse_oha_summary(&text, 4).expect("parse");
        // Six more than the four connections can explain.
        assert_eq!(s.errors, 6);
        assert_eq!(s.requests, 51);
    }

    #[test]
    fn timeouts_are_errors() {
        let text = with_errors(r#"{"timeout":5}"#);
        let s = parse_oha_summary(&text, 4).expect("parse");
        assert_eq!(s.errors, 5);
        assert_eq!(s.requests, 50);
    }

    #[test]
    fn a_window_with_no_answer_has_no_latency() {
        // oha 1.16 against a target that never answers, run with `-w -t`.
        let text = r#"{"summary":{"successRate":0.0,"total":1.0,"slowest":null,"requestsPerSec":3.9},
            "latencyPercentiles":{"p50":null,"p95":null,"p99":null},
            "statusCodeDistribution":{},"errorDistribution":{"timeout":4}}"#;
        let s = parse_oha_summary(text, 4).expect("parse");
        assert_eq!(s.requests, 4);
        assert_eq!(s.errors, 4);
        assert!(s.latency.is_none());
        assert!(s.throughput_rps.abs() < f64::EPSILON);
    }

    #[test]
    fn a_missing_percentile_of_answered_requests_is_an_error() {
        let text = r#"{"summary":{"total":1.0},
            "latencyPercentiles":{"p50":0.01,"p95":null,"p99":0.02},
            "statusCodeDistribution":{"200":3},"errorDistribution":{}}"#;
        let err = parse_oha_summary(text, 4).unwrap_err();
        assert!(err.contains("latencyPercentiles.p95"), "{err}");
    }

    #[test]
    fn a_malformed_count_is_an_error_not_a_zero() {
        let text = r#"{"summary":{"total":1.0},
            "latencyPercentiles":{"p50":0.01,"p95":0.02,"p99":0.02},
            "statusCodeDistribution":{"200":"many"},"errorDistribution":{}}"#;
        let err = parse_oha_summary(text, 4).unwrap_err();
        assert!(err.contains("statusCodeDistribution"), "{err}");
        assert!(err.contains("not a count"), "{err}");

        let text = r#"{"summary":{"total":1.0},
            "latencyPercentiles":{"p50":0.01,"p95":0.02,"p99":0.02},
            "statusCodeDistribution":{"200":3},"errorDistribution":{"timeout":-1}}"#;
        let err = parse_oha_summary(text, 4).unwrap_err();
        assert!(err.contains("errorDistribution"), "{err}");
    }

    #[test]
    fn parse_oha_summary_rejects_non_json_and_missing_fields() {
        assert!(parse_oha_summary("not json", 4).is_err());
        let err = parse_oha_summary(r#"{"summary":{"successRate":1.0}}"#, 4).unwrap_err();
        assert!(err.contains("statusCodeDistribution"), "{err}");
        let err = parse_oha_summary(
            r#"{"summary":{"total":1.0},"statusCodeDistribution":{}}"#,
            4,
        )
        .unwrap_err();
        assert!(err.contains("errorDistribution"), "{err}");
    }

    #[test]
    fn error_rate_of_an_empty_window_is_zero() {
        let s = OhaSummary {
            requests: 0,
            errors: 0,
            throughput_rps: 0.0,
            latency: None,
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

    #[test]
    fn request_timeout_override_must_be_positive_seconds() {
        assert_eq!(request_timeout_from("2.5"), Ok(Duration::from_millis(2500)));
        assert_eq!(request_timeout_from(" 30 "), Ok(Duration::from_secs(30)));
        for bad in ["0", "-1", "soon", "inf", "NaN", ""] {
            let err = request_timeout_from(bad).unwrap_err();
            assert!(err.contains("TUMULT_OHA_REQUEST_TIMEOUT_S"), "{err}");
        }
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
                latency: Some(OhaLatency {
                    p50_ms: 10.0,
                    p95_ms: p95_s * 1000.0,
                    p99_ms: 30.0,
                }),
            }),
        }
    }

    /// A window whose every request failed: counted, but no latency.
    fn unanswered(ts_ns: i64, requests: u64) -> WindowOutcome {
        WindowOutcome {
            ts_ns,
            duration_s: 10.0,
            vus: 20,
            summary: Ok(OhaSummary {
                requests,
                errors: requests,
                throughput_rps: 0.0,
                latency: None,
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
    fn a_completed_window_with_zero_requests_never_meets_thresholds() {
        // The window ran and reported, but nothing was sent.
        let result = aggregate(&run(10), vec![window(1, 0, 0.0, 0.0)], &thresholds(&[]));
        assert_eq!(result.total_requests, 0);
        assert!(!result.thresholds_met, "zero requests must not pass");
    }

    #[test]
    fn an_unanswered_window_counts_its_errors_but_is_no_sample() {
        let result = aggregate(
            &run(20),
            vec![window(1, 100, 0.1, 10.0), unanswered(2, 100)],
            &thresholds(&[]),
        );
        assert_eq!(result.samples.len(), 1);
        assert_eq!(result.total_requests, 200);
        assert!((result.error_rate - 0.5).abs() < 1e-9);
        // The latency is that of the requests that were answered.
        assert!((result.latency_p95_ms - 100.0).abs() < 1e-9);
        // 100 answers over the 20 s both windows ran.
        assert!((result.throughput_rps - 5.0).abs() < 1e-9);
        assert!(!result.thresholds_met);
    }

    #[test]
    fn an_aborted_run_without_a_completed_window_is_an_error() {
        let err = finish(vec![], true).unwrap_err();
        assert!(err.contains("cut before any window completed"), "{err}");
        let err = finish(vec![failed(1)], true).unwrap_err();
        assert!(err.contains("cut before"), "{err}");
    }

    #[test]
    fn finish_drops_only_the_window_an_abort_cut() {
        let kept = finish(vec![window(1, 10, 0.1, 1.0), failed(2)], true).unwrap();
        assert_eq!(kept.len(), 1);
        let kept = finish(vec![window(1, 10, 0.1, 1.0), window(2, 10, 0.1, 1.0)], true).unwrap();
        assert_eq!(kept.len(), 2, "a completed window is evidence");
        let kept = finish(vec![failed(1)], false).unwrap();
        assert_eq!(kept.len(), 1, "a failure on a normal stop is reported");
    }
}
