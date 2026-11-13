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
//! # What a window measures
//!
//! Every window runs with `-w`, so a request still in flight when the
//! window's time is up is waited for and measured rather than dropped, and
//! with `-t`, so a request that never answers is counted as a timeout
//! error. A window may therefore outlast its slot by up to the request
//! timeout. That timeout is the window's planned length, never below
//! [`MIN_REQUEST_TIMEOUT`] nor above [`DEFAULT_REQUEST_TIMEOUT`], unless
//! `TUMULT_OHA_REQUEST_TIMEOUT_S` sets it. Each sample carries the length
//! the window really ran, not the planned one.
//!
//! # The plan clock
//!
//! Windows are scheduled against the plan, not against each other: window
//! `n` owns the slot from its planned offset to its planned end, measured
//! from the start of the run. A window that would start early waits for
//! its slot. One that starts late (by more than start-up jitter) because
//! the window before it ran over runs for what is left of its slot when
//! at least half of it is left, and is skipped otherwise. A stretched
//! window therefore costs the next one time, never the rest of the plan
//! its place: the peak of a ramp is reached when the plan says so.
//!
//! # Planned against completed
//!
//! The result records how many windows the plan called for and how many
//! ran to their end ([`LoadExecution`]). A plan that fell short — a window
//! skipped, failed, or never started — never meets its thresholds, and the
//! reasons say why. A window in which nothing was answered is counted and
//! recorded; it fails the run only through a declared threshold.
//!
//! # Ending a run
//!
//! - [`LoadExecutor::stop`] (the method finished) lets the in-flight
//!   window finish — the first window always runs — and starts no further
//!   one. It waits at most the rest of that window plus the request
//!   timeout. Windows it left unstarted truncate the plan, so a method
//!   should last at least as long as its load.
//! - [`LoadExecutor::abort`] (a guard halted the run, or it was
//!   interrupted) interrupts the in-flight window, gives oha up to
//!   [`ABORT_SUMMARY_WAIT`] to print what it had measured, then kills the
//!   window's process group. The result is marked aborted and keeps the
//!   completed windows plus whatever the cut one reported.
//!
//! A run that sent no request never meets its thresholds.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{load_profile, LoadExecutor, LoadHandle};
use crate::types::{LoadConfig, LoadExecution, LoadResult, LoadSample, LoadTool};

/// Slack added to a window's length plus the request timeout before its
/// process is killed.
pub const DEFAULT_WINDOW_GRACE: Duration = Duration::from_secs(10);
/// The longest oha waits for one answer before counting the request as a
/// timeout error, unless `TUMULT_OHA_REQUEST_TIMEOUT_S` says otherwise.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// The shortest default request timeout: a one-second window still gives
/// a slow answer this long.
pub const MIN_REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
/// How long an aborted window gets to print its partial summary before
/// its process group is killed.
pub const ABORT_SUMMARY_WAIT: Duration = Duration::from_secs(1);
/// The share of its slot a late window must still have to run at all.
const MIN_SLOT_FRACTION: f64 = 0.5;
/// How late a window may start and still count as on time: process
/// start-up and scheduling jitter, not an overrun worth shortening for.
const ON_TIME_SLACK_S: f64 = 0.05;
const POLL_INTERVAL: Duration = Duration::from_millis(20);
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
    /// `None` reads `TUMULT_OHA_REQUEST_TIMEOUT_S`, then the per-window
    /// default.
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
    /// An executor whose windows are killed `grace` after their length
    /// plus the request timeout (tests use a short one; production keeps
    /// the default).
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

    /// The timeout every window uses, or `None` for the per-window
    /// default ([`default_request_timeout`]).
    fn resolved_request_timeout(&self) -> Result<Option<Duration>, String> {
        if let Some(timeout) = self.request_timeout {
            return Ok(Some(timeout));
        }
        match std::env::var("TUMULT_OHA_REQUEST_TIMEOUT_S") {
            Ok(text) => request_timeout_from(&text).map(Some),
            Err(_) => Ok(None),
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

/// The request timeout of a window planned for `window_s` seconds when
/// none is configured: the window's own length, at least
/// [`MIN_REQUEST_TIMEOUT`] and at most [`DEFAULT_REQUEST_TIMEOUT`]. A
/// window so waits for a slow answer no longer than it ran, which bounds
/// how long a normal stop can keep a fault injected.
#[must_use]
pub fn default_request_timeout(window_s: f64) -> Duration {
    let floor = MIN_REQUEST_TIMEOUT.as_secs_f64();
    let ceiling = DEFAULT_REQUEST_TIMEOUT.as_secs_f64();
    if !window_s.is_finite() {
        return DEFAULT_REQUEST_TIMEOUT;
    }
    Duration::from_secs_f64(window_s.clamp(floor, ceiling))
}

/// How one window ended.
#[derive(Debug)]
enum WindowEnd {
    /// It ran to its end and oha reported.
    Completed(OhaSummary),
    /// oha failed, printed nothing usable, or was killed at its deadline.
    Failed(String),
    /// An abort cut it short; the partial summary oha printed on the way
    /// out, when it did.
    Aborted(Option<OhaSummary>),
}

/// What one started window produced.
#[derive(Debug)]
struct WindowOutcome {
    ts_ns: i64,
    vus: u32,
    end: WindowEnd,
}

/// What the worker hands back: the windows it started, and how many it
/// skipped because their slot was mostly gone.
#[derive(Debug, Default)]
struct PlanRun {
    planned: usize,
    outcomes: Vec<WindowOutcome>,
    overrun: usize,
}

/// The two ways a run is told to end, shared with the worker thread.
#[derive(Debug, Default)]
struct Control {
    /// Finish the in-flight window, start no further one.
    stop: AtomicBool,
    /// Cut the in-flight window now and start no further one.
    abort: AtomicBool,
}

struct OhaHandle {
    control: Arc<Control>,
    /// `None` once `stop` or `abort` has taken it.
    worker: Option<std::thread::JoinHandle<PlanRun>>,
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
/// `elapsed_s` is how long the window really ran (`summary.total`),
/// `answered` the requests that got any HTTP answer, and `throughput_rps`
/// the one over the other. `latency` is `None` when no request was
/// answered.
#[derive(Debug, Clone, PartialEq)]
pub struct OhaSummary {
    pub requests: u64,
    pub errors: u64,
    pub answered: u64,
    pub elapsed_s: f64,
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
    let elapsed_s = number(&v, &["summary", "total"])?;
    // Request counts are far below 2^53.
    #[allow(clippy::cast_precision_loss)]
    let throughput_rps = if elapsed_s > 0.0 {
        answered as f64 / elapsed_s
    } else {
        0.0
    };
    Ok(OhaSummary {
        requests: answered + transport,
        errors: errors + transport,
        answered,
        elapsed_s,
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

/// What the plan clock says about a window about to start.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Slot {
    /// Run it for `length_s`, after waiting `wait` for its slot to open.
    Run { wait: Duration, length_s: f64 },
    /// Too little of its slot is left: leave it out.
    Skip,
}

/// Place `window` on the plan clock, `elapsed_s` seconds into the run. On
/// time (within [`ON_TIME_SLACK_S`]) or early, it runs its full length
/// once its slot opens. Late, it runs for the rest of its slot when at
/// least half of it is left, and is skipped otherwise — so an overrun is
/// paid for by the next window, never by pushing the rest of the plan
/// back.
fn slot_for(elapsed_s: f64, window: &load_profile::Window) -> Slot {
    let late_s = elapsed_s - window.offset_s;
    if late_s <= ON_TIME_SLACK_S {
        return Slot::Run {
            wait: Duration::try_from_secs_f64(-late_s).unwrap_or(Duration::ZERO),
            length_s: window.duration_s,
        };
    }
    let left_s = window.duration_s - late_s;
    if left_s < window.duration_s * MIN_SLOT_FRACTION {
        return Slot::Skip;
    }
    Slot::Run {
        wait: Duration::ZERO,
        length_s: left_s,
    }
}

/// What every window of one run shares.
struct WindowRunner {
    binary: String,
    url: std::ffi::OsString,
    /// `None` gives each window [`default_request_timeout`].
    request_timeout: Option<Duration>,
    grace: Duration,
    control: Arc<Control>,
}

impl WindowRunner {
    /// Run the plan's windows on the plan clock until the plan is
    /// exhausted or the run is told to end.
    fn run(&self, windows: &[load_profile::Window]) -> PlanRun {
        let clock = Instant::now();
        let mut run = PlanRun {
            planned: windows.len(),
            outcomes: Vec::with_capacity(windows.len()),
            overrun: 0,
        };
        for (i, window) in windows.iter().enumerate() {
            let Some(slot) = self.await_slot(clock, window, i == 0) else {
                break;
            };
            let Slot::Run { length_s, .. } = slot else {
                run.overrun += 1;
                tracing::warn!(
                    window = i + 1,
                    planned = run.planned,
                    "oha window skipped: the windows before it overran its slot"
                );
                continue;
            };
            run.outcomes.push(WindowOutcome {
                ts_ns: super::epoch_nanos_now(),
                vus: window.vus,
                end: self.run_window(window, length_s),
            });
        }
        run
    }

    /// Whether the run was told to end. The first window ignores a normal
    /// stop, so a load that was stopped at once still leaves evidence.
    fn ended(&self, first: bool) -> bool {
        if self.control.abort.load(Ordering::SeqCst) {
            return true;
        }
        !first && self.control.stop.load(Ordering::SeqCst)
    }

    /// Wait for `window`'s slot to open and say how it runs; `None` when
    /// the run was told to end first.
    fn await_slot(
        &self,
        clock: Instant,
        window: &load_profile::Window,
        first: bool,
    ) -> Option<Slot> {
        loop {
            if self.ended(first) {
                return None;
            }
            let slot = if first {
                Slot::Run {
                    wait: Duration::ZERO,
                    length_s: window.duration_s,
                }
            } else {
                slot_for(clock.elapsed().as_secs_f64(), window)
            };
            match slot {
                Slot::Run { wait, .. } if !wait.is_zero() => {
                    std::thread::sleep(wait.min(POLL_INTERVAL));
                }
                ready => return Some(ready),
            }
        }
    }

    /// Spawn one window's process for `length_s` and wait for it, bounded
    /// by that length plus the request timeout plus grace.
    fn run_window(&self, window: &load_profile::Window, length_s: f64) -> WindowEnd {
        let request_timeout = self
            .request_timeout
            .unwrap_or_else(|| default_request_timeout(window.duration_s));
        let mut cmd = std::process::Command::new(&self.binary);
        #[cfg(unix)]
        {
            // Its own process group, so a signal reaches every process oha
            // (or a wrapper script) spawned, not only the leader.
            std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        }
        let spawned = cmd
            .arg("--no-tui")
            .arg("--output-format")
            .arg("json")
            .arg("-c")
            .arg(window.vus.to_string())
            .arg("-z")
            .arg(oha_duration(length_s))
            // Wait for the requests in flight at the deadline, so a slow
            // answer is measured instead of dropped …
            .arg("-w")
            // … and give up on one that never comes, as a timeout error.
            .arg("-t")
            .arg(oha_duration(request_timeout.as_secs_f64()))
            .arg(&self.url)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn();
        let child = match spawned {
            Ok(child) => child,
            Err(e) => return WindowEnd::Failed(format!("failed to start oha: {e}")),
        };
        let deadline = Instant::now()
            + Duration::from_secs_f64(length_s.max(0.0))
            + request_timeout
            + self.grace;
        match wait_bounded(child, deadline, &self.control.abort) {
            Waited::Exited(output) => match summary_of(&output, window.vus) {
                Ok(summary) => WindowEnd::Completed(summary),
                Err(e) => WindowEnd::Failed(e),
            },
            Waited::Cut(output) => WindowEnd::Aborted(partial_summary(output, window.vus)),
            Waited::Failed(e) => WindowEnd::Failed(e),
        }
    }
}

/// The summary a finished oha printed, or why there is none.
fn summary_of(output: &std::process::Output, connections: u32) -> Result<OhaSummary, String> {
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "oha exited with {}: {}",
            output.status,
            stderr.trim()
        ));
    }
    parse_oha_summary(&String::from_utf8_lossy(&output.stdout), connections)
}

/// What an interrupted oha managed to report. Having nothing is the
/// normal case for a window cut while its requests hung, so the reason is
/// logged for diagnosis rather than reported as a failure.
fn partial_summary(output: Option<std::process::Output>, connections: u32) -> Option<OhaSummary> {
    match summary_of(&output?, connections) {
        Ok(summary) => Some(summary),
        Err(e) => {
            tracing::debug!(error = %e, "aborted oha window left no partial summary");
            None
        }
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

type Pipe = Option<std::sync::mpsc::Receiver<std::io::Result<Vec<u8>>>>;

/// What a drained pipe held, waiting until `deadline` at most: a process
/// that escaped the group may hold the pipe open long after the window.
fn drained(pipe: Pipe, deadline: Instant) -> Result<Vec<u8>, String> {
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

/// How the wait for one window's process ended.
enum Waited {
    /// It exited by itself; its output was read.
    Exited(std::process::Output),
    /// An abort cut it; the output when it exited on the interrupt.
    Cut(Option<std::process::Output>),
    /// It could not be waited for, read, or outlived its deadline.
    Failed(String),
}

/// Wait for `child` while draining its pipes: until it exits, `deadline`
/// passes (then its process group is killed), or `abort` is raised (then
/// it is cut short). Polled from the moment the child exists, so an abort
/// raised before the spawn is seen on the first turn.
fn wait_bounded(mut child: std::process::Child, deadline: Instant, abort: &AtomicBool) -> Waited {
    let stdout = child.stdout.take().map(drain);
    let stderr = child.stderr.take().map(drain);
    loop {
        if abort.load(Ordering::SeqCst) {
            return Waited::Cut(cut_short(&mut child, stdout, stderr));
        }
        match leader_exited(&mut child) {
            Ok(true) => break,
            Ok(false) if Instant::now() >= deadline => {
                kill_tree(&mut child);
                return Waited::Failed(
                    "oha did not exit within its window, request timeout and grace; killed".into(),
                );
            }
            Ok(false) => std::thread::sleep(POLL_INTERVAL),
            Err(e) => {
                // The wait itself failed: make sure nothing is left
                // running or unreaped before reporting.
                kill_tree(&mut child);
                return Waited::Failed(format!("oha wait failed: {e}"));
            }
        }
    }
    match collect(&mut child, stdout, stderr) {
        Ok(output) => Waited::Exited(output),
        Err(e) => Waited::Failed(e),
    }
}

/// End an in-flight window on abort: interrupt its group so oha prints
/// what it had measured, wait [`ABORT_SUMMARY_WAIT`] at most, then kill
/// whatever is left. `None` when it had to be killed or left no output.
fn cut_short(
    child: &mut std::process::Child,
    stdout: Pipe,
    stderr: Pipe,
) -> Option<std::process::Output> {
    if !interrupt_group(child.id()) {
        kill_tree(child);
        return None;
    }
    let give_up = Instant::now() + ABORT_SUMMARY_WAIT;
    loop {
        match leader_exited(child) {
            Ok(true) => break,
            Ok(false) if Instant::now() < give_up => std::thread::sleep(POLL_INTERVAL),
            Ok(false) => {
                kill_tree(child);
                return None;
            }
            Err(e) => {
                tracing::warn!(error = %e, "waiting for an aborted oha window failed; killed");
                kill_tree(child);
                return None;
            }
        }
    }
    match collect(child, stdout, stderr) {
        Ok(output) => Some(output),
        Err(e) => {
            tracing::warn!(error = %e, "an aborted oha window could not be read");
            None
        }
    }
}

/// Reap a leader that has exited and read what it printed.
fn collect(
    child: &mut std::process::Child,
    stdout: Pipe,
    stderr: Pipe,
) -> Result<std::process::Output, String> {
    let status = reap(child)?;
    let drain_deadline = Instant::now() + DRAIN_WAIT;
    Ok(std::process::Output {
        status,
        stdout: drained(stdout, drain_deadline)?,
        stderr: drained(stderr, drain_deadline)?,
    })
}

/// Whether the group leader has exited, without reaping it where the
/// platform allows: it stays a zombie, so its pid — and with it the id of
/// its process group — cannot be handed to another process before
/// [`reap`] has dealt with the group.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn leader_exited(child: &mut std::process::Child) -> std::io::Result<bool> {
    // SAFETY: `siginfo_t` is plain data for which all-zero is valid.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    // SAFETY: `info` is a valid out-pointer and the pid is our own child's.
    // WNOWAIT leaves the child waitable, WNOHANG keeps the call from
    // blocking.
    let rc = unsafe {
        libc::waitid(
            libc::P_PID,
            child.id(),
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: waitid succeeded, so the SIGCHLD fields are the valid ones;
    // the pid stays zero when the child has not changed state.
    Ok(unsafe { info.si_pid() } != 0)
}

/// Elsewhere the exit can only be observed by reaping, so [`reap`] sends
/// no signal afterwards.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn leader_exited(child: &mut std::process::Child) -> std::io::Result<bool> {
    child.try_wait().map(|status| status.is_some())
}

/// Reap an exited leader. Anything it left behind in its group would keep
/// the pipes open, so it is killed first — only while the leader is still
/// unreaped, because a reaped pid may already belong to someone else.
fn reap(child: &mut std::process::Child) -> Result<std::process::ExitStatus, String> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    signal_group(child.id(), libc::SIGKILL);
    child.wait().map_err(|e| format!("oha wait failed: {e}"))
}

/// Send `signal` to the process group led by `pid`. Callers must hold the
/// leader unreaped. Returns whether the signal was delivered.
#[cfg(unix)]
fn signal_group(pid: u32, signal: libc::c_int) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    // SAFETY: plain syscall on a process group this adapter created and
    // whose leader it has not reaped.
    unsafe { libc::kill(-pid, signal) == 0 }
}

/// Ask the window's group to stop and report (SIGINT, on which oha prints
/// its summary). `false` where that cannot be done.
fn interrupt_group(pid: u32) -> bool {
    #[cfg(unix)]
    {
        signal_group(pid, libc::SIGINT)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

/// Kill the window's whole process group (Unix) or the leader (elsewhere)
/// and reap the leader. Only for a leader not yet reaped. The pipes may
/// stay open until every descendant is gone, so callers must not block on
/// draining them afterwards.
fn kill_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    signal_group(child.id(), libc::SIGKILL);
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
    /// within [`ABORT_SUMMARY_WAIT`].
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
        let plan = worker
            .join()
            .map_err(|_| "oha worker thread panicked".to_string())?;
        let run = RunShape {
            started_at_ns: oha.started_at_ns,
            ended_at_ns: super::epoch_nanos_now(),
            tool: oha.tool.clone(),
            vus: oha.vus,
            abort_requested: end == End::Abort,
        };
        Ok(aggregate(&run, plan, &oha.thresholds))
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
        let worker = std::thread::spawn(move || runner.run(&windows));
        tracing::info!(
            windows = window_count,
            peak_vus = plan.peak_vus(),
            total_s = plan.total_duration_s(),
            request_timeout_s = request_timeout.map(|t| t.as_secs_f64()),
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

/// The run-level facts the aggregate carries over unchanged.
struct RunShape {
    started_at_ns: i64,
    ended_at_ns: i64,
    tool: LoadTool,
    vus: u32,
    /// The run was ended through [`LoadExecutor::abort`].
    abort_requested: bool,
}

/// The windows folded together, before thresholds are applied.
#[derive(Default)]
struct Folded {
    samples: Vec<LoadSample>,
    requests: u64,
    errors: u64,
    /// Real seconds and answered requests of the windows with a summary.
    time_s: f64,
    answered: u64,
    /// Windows that ran to their end and reported.
    completed: usize,
    /// Completed windows in which nothing was answered.
    unanswered: usize,
    /// Why each failed window failed.
    failures: Vec<String>,
    /// A window was cut by an abort.
    cut: bool,
}

impl Folded {
    /// Count one summary and keep it as a sample when anything answered.
    /// Returns whether it did.
    fn add(&mut self, ts_ns: i64, vus: u32, summary: &OhaSummary) -> bool {
        self.requests += summary.requests;
        self.errors += summary.errors;
        self.time_s += summary.elapsed_s;
        self.answered += summary.answered;
        let Some(latency) = summary.latency.as_ref() else {
            return false;
        };
        self.samples.push(LoadSample {
            ts_ns,
            duration_s: summary.elapsed_s,
            vus,
            throughput_rps: summary.throughput_rps,
            latency_p50_ms: latency.p50_ms,
            latency_p95_ms: latency.p95_ms,
            latency_p99_ms: latency.p99_ms,
            error_rate: summary.error_rate(),
            requests: summary.requests,
        });
        true
    }
}

fn fold(outcomes: Vec<WindowOutcome>) -> Folded {
    let mut folded = Folded::default();
    for outcome in outcomes {
        match outcome.end {
            WindowEnd::Completed(summary) => {
                folded.completed += 1;
                if !folded.add(outcome.ts_ns, outcome.vus, &summary) {
                    folded.unanswered += 1;
                    tracing::warn!(
                        requests = summary.requests,
                        vus = outcome.vus,
                        "oha window answered no request"
                    );
                }
            }
            WindowEnd::Failed(e) => {
                tracing::warn!(error = %e, vus = outcome.vus, "oha window failed");
                folded.failures.push(e);
            }
            WindowEnd::Aborted(partial) => {
                folded.cut = true;
                if let Some(summary) = partial {
                    folded.add(outcome.ts_ns, outcome.vus, &summary);
                }
            }
        }
    }
    folded
}

/// A window count for the journal; plans are far below `u32::MAX` windows.
fn window_count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// How the plan was carried out, with the reasons it fell short.
fn execution(
    folded: &Folded,
    planned: usize,
    started: usize,
    overrun: usize,
    abort_requested: bool,
) -> LoadExecution {
    let not_started = planned.saturating_sub(started + overrun);
    // An abort that arrived after the plan had run out cut nothing.
    let aborted = abort_requested && (folded.cut || not_started > 0);
    let completed = folded.completed;
    let mut reasons = Vec::new();
    if aborted {
        reasons.push(format!(
            "load aborted after {completed} of {planned} windows completed"
        ));
    }
    if let Some(first) = folded.failures.first() {
        reasons.push(format!(
            "{} of {planned} windows failed: {first}",
            folded.failures.len()
        ));
    }
    if overrun > 0 {
        reasons.push(format!(
            "{overrun} of {planned} windows skipped: the windows before them overran their slot"
        ));
    }
    if !aborted && not_started > 0 {
        reasons.push(format!(
            "the method ended before {not_started} of {planned} windows started; \
             a method should last at least as long as its load"
        ));
    }
    if folded.requests == 0 {
        reasons.push("no request was sent".into());
    }
    LoadExecution {
        planned_windows: Some(window_count(planned)),
        completed_windows: Some(window_count(completed)),
        unanswered_windows: window_count(folded.unanswered),
        aborted,
        reasons,
    }
}

/// Say out loud what a reader of the journal would otherwise have to
/// work out.
fn log_shortfall(execution: &LoadExecution) {
    if execution.aborted {
        tracing::warn!(
            planned_windows = execution.planned_windows,
            completed_windows = execution.completed_windows,
            "oha load aborted"
        );
    }
    if execution.truncated() {
        tracing::warn!(
            planned_windows = execution.planned_windows,
            completed_windows = execution.completed_windows,
            reasons = ?execution.reasons,
            "oha load plan truncated; thresholds reported unmet"
        );
    }
}

/// Fold the windows into one [`LoadResult`]. Throughput is answered
/// requests over the time the windows really ran, latency percentiles are
/// request-weighted means of the window percentiles (an approximation,
/// since oha does not expose raw latencies), the error rate is all errors
/// over all requests, and every window that answered something becomes a
/// sample of its real length. Thresholds are met only by a run that sent
/// requests, ran every planned window to its end, and kept within the
/// declared bounds.
fn aggregate(
    run: &RunShape,
    plan: PlanRun,
    thresholds: &std::collections::HashMap<String, f64>,
) -> LoadResult {
    let started = plan.outcomes.len();
    let folded = fold(plan.outcomes);
    let execution = execution(
        &folded,
        plan.planned,
        started,
        plan.overrun,
        run.abort_requested,
    );
    log_shortfall(&execution);
    #[allow(clippy::cast_precision_loss)]
    let elapsed_s = (run.ended_at_ns - run.started_at_ns) as f64 / 1_000_000_000.0;
    let sampled_requests: u64 = folded.samples.iter().map(|s| s.requests).sum();
    let weighted = |f: fn(&LoadSample) -> f64| -> f64 {
        if sampled_requests == 0 {
            return 0.0;
        }
        #[allow(clippy::cast_precision_loss)]
        let sum: f64 = folded
            .samples
            .iter()
            .map(|s| f(s) * s.requests as f64)
            .sum();
        #[allow(clippy::cast_precision_loss)]
        {
            sum / sampled_requests as f64
        }
    };
    // Request counts are far below 2^53.
    #[allow(clippy::cast_precision_loss)]
    let throughput_rps = if folded.time_s > 0.0 {
        folded.answered as f64 / folded.time_s
    } else {
        0.0
    };
    #[allow(clippy::cast_precision_loss)]
    let error_rate = if folded.requests > 0 {
        (folded.errors as f64 / folded.requests as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    if folded.requests == 0 {
        tracing::warn!("oha: no request was sent; thresholds reported unmet");
    }
    let ran_as_planned = folded.requests > 0 && !execution.truncated() && !execution.aborted;
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
        thresholds_met: ran_as_planned,
        samples: folded.samples,
        execution,
    };
    let thresholds_met = result.thresholds_met && thresholds_hold(&result, thresholds);
    LoadResult {
        thresholds_met,
        ..result
    }
}

/// oha has no threshold language of its own, so the config's thresholds
/// are checked here against the aggregate: upper bounds on latency and
/// error rate, a lower bound on throughput. A latency bound is unmet
/// when no request was answered, and an unknown metric name is reported,
/// never silently passed.
fn thresholds_hold(
    result: &LoadResult,
    thresholds: &std::collections::HashMap<String, f64>,
) -> bool {
    // No answer, no latency: an unknown figure is never within a bound.
    let answered = !result.samples.is_empty();
    let mut held = true;
    for (metric, bound) in thresholds {
        let ok = match metric.as_str() {
            "latency_p50_ms" => answered && result.latency_p50_ms <= *bound,
            "latency_p95_ms" => answered && result.latency_p95_ms <= *bound,
            "latency_p99_ms" => answered && result.latency_p99_ms <= *bound,
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
        assert_eq!(s.answered, 600);
        // 500 and 429 answers plus the transport failures; 302 is fine.
        assert_eq!(s.errors, 22);
        assert!((s.error_rate() - 22.0 / 610.0).abs() < 1e-12);
        // Answered requests over the window's real length: 600 / 5 s.
        assert!((s.elapsed_s - 5.0).abs() < 1e-12);
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
        assert_eq!(s.answered, 0);
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
            answered: 0,
            elapsed_s: 1.0,
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

    #[test]
    fn the_default_request_timeout_follows_the_window_within_bounds() {
        // A short window still gives a slow answer two seconds …
        assert_eq!(default_request_timeout(1.0), Duration::from_secs(2));
        assert_eq!(default_request_timeout(0.2), Duration::from_secs(2));
        // … a middling one waits as long as it ran …
        assert_eq!(default_request_timeout(10.0), Duration::from_secs(10));
        // … and a long one never more than the ceiling.
        assert_eq!(default_request_timeout(600.0), DEFAULT_REQUEST_TIMEOUT);
        assert_eq!(default_request_timeout(f64::NAN), DEFAULT_REQUEST_TIMEOUT);
    }

    fn slot(offset_s: f64, duration_s: f64) -> load_profile::Window {
        load_profile::Window {
            offset_s,
            duration_s,
            vus: 4,
        }
    }

    fn assert_runs(slot: Slot, wait_s: f64, length_s: f64) {
        let Slot::Run {
            wait,
            length_s: got,
        } = slot
        else {
            panic!("expected a run, got {slot:?}");
        };
        assert!((wait.as_secs_f64() - wait_s).abs() < 1e-9, "wait {wait:?}");
        assert!((got - length_s).abs() < 1e-9, "length {got}");
    }

    #[test]
    fn a_window_on_time_runs_its_full_length() {
        assert_runs(slot_for(10.0, &slot(10.0, 5.0)), 0.0, 5.0);
    }

    #[test]
    fn start_up_jitter_is_not_an_overrun() {
        assert_runs(slot_for(10.04, &slot(10.0, 5.0)), 0.0, 5.0);
    }

    #[test]
    fn an_early_window_waits_for_its_slot() {
        assert_runs(slot_for(8.5, &slot(10.0, 5.0)), 1.5, 5.0);
    }

    #[test]
    fn a_late_window_runs_for_what_is_left_of_its_slot() {
        // 2 s late into a 5 s slot: 3 s left, more than half.
        assert_runs(slot_for(12.0, &slot(10.0, 5.0)), 0.0, 3.0);
        // Exactly half left still runs.
        assert_runs(slot_for(12.5, &slot(10.0, 5.0)), 0.0, 2.5);
    }

    #[test]
    fn a_window_with_less_than_half_its_slot_left_is_skipped() {
        assert_eq!(slot_for(12.6, &slot(10.0, 5.0)), Slot::Skip);
        // Its whole slot has passed.
        assert_eq!(slot_for(30.0, &slot(10.0, 5.0)), Slot::Skip);
    }

    fn run(ended_at_ns: i64) -> RunShape {
        RunShape {
            started_at_ns: 0,
            ended_at_ns,
            tool: LoadTool::Oha,
            vus: 20,
            abort_requested: false,
        }
    }

    fn aborted_run() -> RunShape {
        RunShape {
            abort_requested: true,
            ..run(20)
        }
    }

    fn thresholds(pairs: &[(&str, f64)]) -> std::collections::HashMap<String, f64> {
        pairs.iter().map(|(k, v)| ((*k).to_string(), *v)).collect()
    }

    /// A plan in which every planned window was started.
    fn plan(outcomes: Vec<WindowOutcome>) -> PlanRun {
        PlanRun {
            planned: outcomes.len(),
            outcomes,
            overrun: 0,
        }
    }

    /// `requests` answered over a real 10 s at `rps`.
    fn summary(requests: u64, p95_s: f64, elapsed_s: f64) -> OhaSummary {
        #[allow(clippy::cast_precision_loss)]
        let throughput_rps = requests as f64 / elapsed_s;
        OhaSummary {
            requests,
            errors: 0,
            answered: requests,
            elapsed_s,
            throughput_rps,
            latency: Some(OhaLatency {
                p50_ms: 10.0,
                p95_ms: p95_s * 1000.0,
                p99_ms: 30.0,
            }),
        }
    }

    fn window(ts_ns: i64, requests: u64, p95_s: f64) -> WindowOutcome {
        WindowOutcome {
            ts_ns,
            vus: 20,
            end: WindowEnd::Completed(summary(requests, p95_s, 10.0)),
        }
    }

    /// A window whose every request failed: counted, but no latency.
    fn unanswered(ts_ns: i64, requests: u64) -> WindowOutcome {
        WindowOutcome {
            ts_ns,
            vus: 20,
            end: WindowEnd::Completed(OhaSummary {
                requests,
                errors: requests,
                answered: 0,
                elapsed_s: 10.0,
                throughput_rps: 0.0,
                latency: None,
            }),
        }
    }

    fn failed(ts_ns: i64) -> WindowOutcome {
        WindowOutcome {
            ts_ns,
            vus: 20,
            end: WindowEnd::Failed("boom".into()),
        }
    }

    fn cut(ts_ns: i64, partial: Option<OhaSummary>) -> WindowOutcome {
        WindowOutcome {
            ts_ns,
            vus: 20,
            end: WindowEnd::Aborted(partial),
        }
    }

    #[test]
    fn aggregate_weights_windows_and_keeps_each_as_a_sample() {
        let outcomes = vec![window(1, 100, 0.1), window(2, 300, 0.2)];
        let result = aggregate(&run(20_000_000_000), plan(outcomes), &thresholds(&[]));
        assert_eq!(result.samples.len(), 2);
        assert_eq!(result.total_requests, 400);
        // Request-weighted p95: (100*100 + 300*200) / 400 = 175 ms.
        assert!((result.latency_p95_ms - 175.0).abs() < 1e-9);
        // 400 answers over the 20 s the two windows ran.
        assert!((result.throughput_rps - 20.0).abs() < 1e-9);
        assert!(result.thresholds_met);
        assert!((result.duration_s - 20.0).abs() < 1e-9);
        assert_eq!(result.execution.planned_windows, Some(2));
        assert_eq!(result.execution.completed_windows, Some(2));
        assert!(result.execution.reasons.is_empty());
        assert!(!result.execution.aborted);
    }

    #[test]
    fn a_sample_carries_the_length_the_window_really_ran() {
        // Planned for 10 s, stretched to 14.5 s by a slow tail.
        let stretched = WindowOutcome {
            ts_ns: 1,
            vus: 20,
            end: WindowEnd::Completed(summary(290, 0.1, 14.5)),
        };
        let result = aggregate(&run(20), plan(vec![stretched]), &thresholds(&[]));
        assert!((result.samples[0].duration_s - 14.5).abs() < 1e-9);
        assert!((result.throughput_rps - 20.0).abs() < 1e-9);
    }

    #[test]
    fn aggregate_marks_failed_windows_and_unmet_thresholds() {
        let result = aggregate(
            &run(20),
            plan(vec![window(1, 100, 0.1), failed(3)]),
            &thresholds(&[]),
        );
        assert_eq!(result.samples.len(), 1);
        assert!(!result.thresholds_met);
        assert_eq!(result.execution.completed_windows, Some(1));
        assert_eq!(
            result.execution.reasons,
            vec!["1 of 2 windows failed: boom"]
        );

        let strict = thresholds(&[("latency_p95_ms", 50.0)]);
        let result = aggregate(&run(20), plan(vec![window(1, 100, 0.1)]), &strict);
        assert!(!result.thresholds_met);

        let unknown = thresholds(&[("bogus", 1.0)]);
        let result = aggregate(&run(20), plan(vec![window(1, 100, 0.1)]), &unknown);
        assert!(!result.thresholds_met);

        let lenient = thresholds(&[("latency_p95_ms", 500.0), ("throughput_rps", 5.0)]);
        let result = aggregate(&run(20), plan(vec![window(1, 100, 0.1)]), &lenient);
        assert!(result.thresholds_met);
    }

    #[test]
    fn a_plan_the_method_cut_short_never_meets_its_thresholds() {
        // A 60-window ramp of which the stop let only 35 start.
        let outcomes: Vec<_> = (0..35).map(|i| window(i, 100, 0.1)).collect();
        let truncated = PlanRun {
            planned: 60,
            outcomes,
            overrun: 0,
        };
        let result = aggregate(&run(20), truncated, &thresholds(&[]));
        assert_eq!(result.execution.planned_windows, Some(60));
        assert_eq!(result.execution.completed_windows, Some(35));
        assert!(result.execution.truncated());
        assert!(!result.execution.aborted);
        assert!(!result.thresholds_met, "a truncated plan must not pass");
        let reasons = result.execution.reasons.join("; ");
        assert!(
            reasons.contains("the method ended before 25 of 60 windows started"),
            "{reasons}"
        );
    }

    #[test]
    fn windows_skipped_for_an_overrun_truncate_the_plan() {
        let skipped = PlanRun {
            planned: 4,
            outcomes: vec![window(1, 100, 0.1), window(2, 100, 0.1)],
            overrun: 2,
        };
        let result = aggregate(&run(20), skipped, &thresholds(&[]));
        assert_eq!(result.execution.completed_windows, Some(2));
        assert!(!result.thresholds_met);
        let reasons = result.execution.reasons.join("; ");
        assert!(reasons.contains("2 of 4 windows skipped"), "{reasons}");
        assert!(!reasons.contains("method ended"), "{reasons}");
    }

    #[test]
    fn aggregate_of_no_windows_never_meets_thresholds() {
        let result = aggregate(&run(10), plan(vec![]), &thresholds(&[]));
        assert_eq!(result.total_requests, 0);
        assert!(result.samples.is_empty());
        assert!(!result.thresholds_met, "no evidence must not pass");
        assert!((result.latency_p95_ms).abs() < f64::EPSILON);
        assert!((result.throughput_rps).abs() < f64::EPSILON);
        assert_eq!(result.execution.reasons, vec!["no request was sent"]);
    }

    #[test]
    fn a_completed_window_with_zero_requests_never_meets_thresholds() {
        // The window ran and reported, but nothing was sent.
        let result = aggregate(&run(10), plan(vec![window(1, 0, 0.0)]), &thresholds(&[]));
        assert_eq!(result.total_requests, 0);
        assert!(!result.thresholds_met, "zero requests must not pass");
    }

    #[test]
    fn a_blacked_out_window_is_recorded_not_a_deviation() {
        let outcomes = || vec![window(1, 100, 0.1), unanswered(2, 100)];
        let result = aggregate(&run(20), plan(outcomes()), &thresholds(&[]));
        assert_eq!(result.samples.len(), 1);
        assert_eq!(result.total_requests, 200);
        assert!((result.error_rate - 0.5).abs() < 1e-9);
        // The latency is that of the requests that were answered.
        assert!((result.latency_p95_ms - 100.0).abs() < 1e-9);
        // 100 answers over the 20 s both windows ran.
        assert!((result.throughput_rps - 5.0).abs() < 1e-9);
        assert_eq!(result.execution.unanswered_windows, 1);
        assert_eq!(result.execution.completed_windows, Some(2));
        assert!(
            result.thresholds_met,
            "no threshold was declared, so the blackout is evidence, not a verdict"
        );

        // The same blackout against a declared bound is a miss.
        let bound = thresholds(&[("error_rate", 0.1)]);
        let result = aggregate(&run(20), plan(outcomes()), &bound);
        assert!(!result.thresholds_met);
    }

    #[test]
    fn a_latency_threshold_is_unmet_when_nothing_was_answered() {
        let bound = thresholds(&[("latency_p95_ms", 500.0)]);
        let result = aggregate(&run(20), plan(vec![unanswered(1, 50)]), &bound);
        assert_eq!(result.total_requests, 50);
        assert!(
            !result.thresholds_met,
            "an unknown latency cannot be within its bound"
        );
        // Without a threshold the all-error run is evidence and stands.
        let result = aggregate(&run(20), plan(vec![unanswered(1, 50)]), &thresholds(&[]));
        assert!(result.thresholds_met);
    }

    #[test]
    fn an_aborted_run_keeps_what_the_cut_window_reported() {
        // A flat load: one planned window, cut after 3.2 s.
        let flat = plan(vec![cut(1, Some(summary(64, 0.1, 3.2)))]);
        let result = aggregate(&aborted_run(), flat, &thresholds(&[]));
        assert!(result.execution.aborted);
        assert_eq!(result.execution.planned_windows, Some(1));
        assert_eq!(result.execution.completed_windows, Some(0));
        assert_eq!(result.total_requests, 64);
        assert_eq!(result.samples.len(), 1);
        assert!((result.samples[0].duration_s - 3.2).abs() < 1e-9);
        assert!(!result.thresholds_met, "an aborted load never passes");
        assert_eq!(
            result.execution.reasons,
            vec!["load aborted after 0 of 1 windows completed"]
        );
    }

    #[test]
    fn an_aborted_run_with_no_evidence_is_still_a_marked_result() {
        let result = aggregate(&aborted_run(), plan(vec![cut(1, None)]), &thresholds(&[]));
        assert!(result.execution.aborted);
        assert_eq!(result.total_requests, 0);
        assert!(result.samples.is_empty());
        assert!(!result.thresholds_met);

        // Aborted before the first window was even started.
        let nothing = PlanRun {
            planned: 3,
            outcomes: vec![],
            overrun: 0,
        };
        let result = aggregate(&aborted_run(), nothing, &thresholds(&[]));
        assert!(result.execution.aborted);
        assert_eq!(result.execution.completed_windows, Some(0));
    }

    #[test]
    fn an_abort_does_not_hide_a_window_that_failed_on_its_own() {
        // The second window failed by itself, the third was cut.
        let outcomes = vec![window(1, 10, 0.1), failed(2), cut(3, None)];
        let aborted = PlanRun {
            planned: 5,
            outcomes,
            overrun: 0,
        };
        let result = aggregate(&aborted_run(), aborted, &thresholds(&[]));
        assert!(result.execution.aborted);
        assert_eq!(result.execution.completed_windows, Some(1));
        let reasons = result.execution.reasons.join("; ");
        assert!(reasons.contains("1 of 5 windows failed: boom"), "{reasons}");
        assert!(reasons.contains("load aborted after 1 of 5"), "{reasons}");
    }

    #[test]
    fn an_abort_after_the_plan_ran_out_cut_nothing() {
        let done = plan(vec![window(1, 10, 0.1), window(2, 10, 0.1)]);
        let result = aggregate(&aborted_run(), done, &thresholds(&[]));
        assert!(!result.execution.aborted);
        assert!(result.thresholds_met);
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn an_exited_leader_is_seen_without_being_reaped() {
        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("spawn true");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !leader_exited(&mut child).expect("waitid") {
            assert!(Instant::now() < deadline, "true never exited");
            std::thread::sleep(Duration::from_millis(5));
        }
        // Still a zombie: asking again gives the same answer, and its pid
        // is ours to signal until the reap.
        assert!(leader_exited(&mut child).expect("waitid"));
        let status = reap(&mut child).expect("reap");
        assert!(status.success());
        // Now it is gone for good.
        assert!(leader_exited(&mut child).is_err());
    }
}
