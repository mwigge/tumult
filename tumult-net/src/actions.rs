//! TCP chaos-proxy actions.
//!
//! Every action is a plain async free function dispatched by the CLI. Because
//! the CLI runs each action in a fresh `sync_await` call, a
//! long-running proxy fault cannot live in process memory: the disruptive
//! `start` functions spawn a detached `tumult-net-proxyd` daemon, which records
//! its PID in a pidfile under the OS temp directory once it serves, and the
//! [`stop_proxy`] rollback tears that daemon down from a fresh invocation.
//! `stop_proxy` is idempotent and safe to call when no proxy is running.
//!
//! Two guards make the detached-daemon hand-off safe:
//!
//! - **Readiness** — a start on an address whose pidfile names a live proxyd
//!   is refused; after spawning, the start waits (with a short deadline) until
//!   the pidfile names the new daemon; a dead-on-arrival daemon (e.g. the port
//!   was already bound) fails the start with its captured stderr, and the
//!   start does not touch the pidfile.
//! - **Identity** — [`stop_proxy`] verifies the pidfile's PID actually belongs
//!   to a `tumult-net-proxyd` process before signalling it, so a stale or
//!   planted pidfile in the world-writable temp dir cannot kill an unrelated
//!   process.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tumult_otel::logging::err;

use crate::config::{FaultProfile, ProxySpec};
use crate::error::NetError;
use crate::pidfile;
use crate::startup_log::{self, StartupLog};

/// Environment variable that overrides daemon discovery (used by tests and by
/// non-standard install layouts).
const PROXYD_ENV: &str = "TUMULT_NET_PROXYD";
/// The daemon binary name shipped alongside the CLI.
const PROXYD_BIN: &str = "tumult-net-proxyd";
/// How long a freshly spawned daemon gets to record its pid in its pidfile
/// before the start is declared failed.
const READY_TIMEOUT: Duration = Duration::from_secs(2);
/// Interval between looks at the pidfile and the daemon during a start.
const READY_POLL: Duration = Duration::from_millis(50);

/// Derive the pidfile path used to discover a proxy bound to `listen`.
fn pidfile_path(listen: SocketAddr) -> PathBuf {
    let key: String = listen
        .to_string()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    std::env::temp_dir().join(format!("tumult-net-{key}.pid"))
}

/// Locate the `tumult-net-proxyd` daemon: honour the override env var, then look
/// next to the current executable (and one directory up, covering the Cargo
/// `target/<profile>/deps` test layout).
fn locate_proxyd() -> Result<PathBuf, NetError> {
    if let Ok(path) = std::env::var(PROXYD_ENV) {
        let candidate = PathBuf::from(path);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    let exe = std::env::current_exe()?;
    if let Some(dir) = exe.parent() {
        for candidate in [dir.join(PROXYD_BIN), dir.join("..").join(PROXYD_BIN)] {
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }

    Err(NetError::invalid(
        "proxyd",
        format!("could not locate `{PROXYD_BIN}`; set {PROXYD_ENV} to its path"),
    ))
}

/// Where the private directory holding the stderr log of the proxy bound to
/// `listen` may be, the preferred place first; see [`startup_log`].
fn log_dirs(listen: SocketAddr) -> Vec<PathBuf> {
    startup_log::dirs_for(&pidfile_path(listen), std::env::var_os("XDG_RUNTIME_DIR"))
}

/// Creates the daemon's stderr log off the async runtime's threads.
async fn create_startup_log(dirs: Vec<PathBuf>) -> Result<(StartupLog, std::fs::File), NetError> {
    let created = tokio::task::spawn_blocking(move || StartupLog::create_in_first(&dirs))
        .await
        .map_err(std::io::Error::other)?;
    Ok(created?)
}

/// Removes the log of the proxy bound to `listen` from wherever a start
/// may have put it.
async fn remove_startup_log(listen: SocketAddr) {
    for dir in log_dirs(listen) {
        startup_log::remove(&dir).await;
    }
}

/// What became of a freshly spawned daemon.
enum Startup {
    /// The pidfile names it: it serves its listen address.
    Recorded,
    /// It exited first, and is reaped.
    Exited(std::process::ExitStatus),
    /// Neither happened within [`READY_TIMEOUT`].
    TimedOut,
}

/// Wait until the pidfile names `child`, `child` exits, or [`READY_TIMEOUT`]
/// elapses.
async fn await_startup(
    child: &mut std::process::Child,
    pidfile: &Path,
) -> Result<Startup, NetError> {
    let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Startup::Exited(status));
        }
        if pidfile::read(pidfile).await? == Some(child.id()) {
            return Ok(Startup::Recorded);
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(Startup::TimedOut);
        }
        tokio::time::sleep(READY_POLL).await;
    }
}

/// Kill a daemon that did not come up and wait for it, so not even a zombie
/// is left. The start fails either way, so a failure here is logged.
async fn end_daemon(mut child: std::process::Child) {
    let pid = child.id();
    let ended = match tokio::task::spawn_blocking(move || {
        child.kill()?;
        child.wait()
    })
    .await
    {
        Ok(waited) => waited.map(drop),
        Err(join) => Err(std::io::Error::other(join)),
    };
    if let Err(error) = ended {
        tracing::warn!(
            name: "net.proxy.reap.failed",
            { process.pid = pid, error.type = "io", error.message = %err(&error) },
            "the chaos proxy that did not come up could not be stopped"
        );
    }
}

/// Refuse a start on `listen` while its pidfile names a live proxyd: a new
/// daemon could not bind the address, and the pidfile must go on naming the
/// proxy that serves it. A pidfile that names no live proxyd is stale.
async fn refuse_a_second_proxy(listen: SocketAddr, pidfile: &Path) -> Result<(), NetError> {
    let Some(pid) = pidfile::read(pidfile).await? else {
        return Ok(());
    };
    if !is_proxyd_process(pid).await {
        return Ok(());
    }
    Err(NetError::invalid(
        "listen",
        format!(
            "a chaos proxy already runs on {listen} (pid {pid}); stop it with `stop_proxy` before starting another"
        ),
    ))
}

/// Spawn a detached daemon for `spec`, wait until it records itself in its
/// pidfile, and return the PID.
///
/// A start on an address whose pidfile names a live proxyd is refused before
/// anything is spawned. The daemon writes its pid to the pidfile once it has
/// bound `listen`; the start waits up to [`READY_TIMEOUT`] for that. A daemon
/// that exits first (e.g. the port was already bound) or does not record
/// itself in time fails the start with its captured stderr: it is stopped
/// and reaped, and the start does not touch the pidfile.
async fn spawn_proxy(
    listen: SocketAddr,
    upstream: SocketAddr,
    profile: FaultProfile,
) -> Result<u32, NetError> {
    if listen == upstream {
        return Err(NetError::invalid(
            "upstream",
            format!("listen and upstream are both {listen}; a proxy loop is never useful"),
        ));
    }
    profile.validate()?;
    let daemon = locate_proxyd()?;
    let pidfile = pidfile_path(listen);
    refuse_a_second_proxy(listen, &pidfile).await?;
    let spec = ProxySpec {
        listen,
        upstream,
        profile,
        pidfile: pidfile.clone(),
    };

    // The detached daemon must outlive this CLI invocation, so it is launched
    // via std (which never kills the child on drop). stderr goes to a log
    // file instead of the null device so the readiness check can report
    // *why* a dead-on-arrival daemon failed. The log removes itself when
    // this function leaves without a running daemon.
    let (log, stderr_file) = create_startup_log(log_dirs(listen)).await?;
    let mut command = std::process::Command::new(&daemon);
    command
        .args(spec.to_argv())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr_file));
    let mut child = tumult_spawn::alone(|| command.spawn())?;
    let pid = child.id();

    let failure = match await_startup(&mut child, &pidfile).await {
        Ok(Startup::Recorded) => {
            // The log stays for as long as the daemon runs: it is where the
            // daemon's structured events go. `stop_proxy` removes it. The
            // daemon outlives this call, so its handle is let go unwaited.
            log.keep();
            return Ok(pid);
        }
        Ok(Startup::Exited(status)) => format!("exited before it served {listen}: {status}"),
        Ok(Startup::TimedOut) => {
            end_daemon(child).await;
            format!(
                "did not record its pid in {} within {}s",
                pidfile.display(),
                READY_TIMEOUT.as_secs()
            )
        }
        Err(error) => {
            end_daemon(child).await;
            return Err(error);
        }
    };

    // Dead on arrival: surface the daemon's captured stderr.
    let reason = startup_failure_reason(&log.read().await);
    Err(NetError::invalid(
        "proxyd",
        format!("`{PROXYD_BIN}` (pid {pid}) {failure} ({reason})"),
    ))
}

/// Best-effort `kill <pid>`. The caller carries on either way (the daemon
/// may already be gone), so the outcome is logged rather than returned.
async fn terminate(pid: u32) {
    let result = tokio::process::Command::new("kill")
        .arg(pid.to_string())
        .status()
        .await;
    log_kill_result(pid, &result);
}

/// Logs a `kill` that did not deliver its signal: a warning when `kill`
/// itself could not be run (the daemon may still be alive), a debug event
/// when it ran and refused (normally: no such process).
fn log_kill_result(pid: u32, result: &std::io::Result<std::process::ExitStatus>) {
    match result {
        Ok(status) if status.success() => {}
        Ok(status) => tracing::debug!(
            name: "net.proxy.kill.rejected",
            { process.pid = pid, process.exit.code = status.code() },
            "kill did not signal the chaos proxy"
        ),
        Err(error) => tracing::warn!(
            name: "net.proxy.kill.failed",
            { process.pid = pid, error.type = "io", error.message = %err(error) },
            "kill could not be run, the chaos proxy may still be alive"
        ),
    }
}

/// Why a dead-on-arrival daemon failed, from its captured stderr.
///
/// The daemon logs JSON lines when stderr is a file; the last line's
/// `error.message` is the cause. Anything else (a panic message, text
/// format) is passed through as written.
fn startup_failure_reason(stderr: &str) -> String {
    let detail = stderr.trim();
    if detail.is_empty() {
        return "daemon produced no output".to_string();
    }
    let structured = detail
        .lines()
        .next_back()
        .and_then(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .and_then(|event| event.get("error.message")?.as_str().map(str::to_owned));
    format!("daemon stderr: {}", structured.as_deref().unwrap_or(detail))
}

/// Start a composite chaos proxy applying every fault in `profile`.
///
/// # Errors
///
/// Returns [`NetError`] if the profile is invalid, the daemon binary cannot be
/// located, or the daemon fails to spawn.
#[tracing::instrument]
#[must_use = "callers must check whether the proxy was started"]
pub async fn start_proxy(
    listen: SocketAddr,
    upstream: SocketAddr,
    profile: FaultProfile,
) -> Result<String, NetError> {
    let listen_s = listen.to_string();
    let upstream_s = upstream.to_string();
    let _span = crate::telemetry::begin_start_proxy(&listen_s, &upstream_s);
    let pid = spawn_proxy(listen, upstream, profile).await?;
    crate::telemetry::event_proxy_started(pid);
    Ok(format!(
        "chaos proxy started on {listen_s} → {upstream_s} (pid {pid})"
    ))
}

/// Inject one-way latency (with optional deterministic jitter) into a proxy.
///
/// # Errors
///
/// Returns [`NetError`] if the daemon cannot be located or spawned.
#[tracing::instrument]
#[must_use = "callers must check whether the latency fault was injected"]
pub async fn inject_latency(
    listen: SocketAddr,
    upstream: SocketAddr,
    delay_ms: u64,
    jitter_ms: u64,
    seed: u64,
) -> Result<String, NetError> {
    let listen_s = listen.to_string();
    let _span = crate::telemetry::begin_inject_latency(&listen_s, delay_ms);
    let profile = FaultProfile {
        delay_ms,
        jitter_ms,
        seed,
        ..FaultProfile::default()
    };
    let pid = spawn_proxy(listen, upstream, profile).await?;
    crate::telemetry::event_proxy_started(pid);
    Ok(format!(
        "latency fault active on {listen_s}: {delay_ms}ms +{jitter_ms}ms jitter (pid {pid})"
    ))
}

/// Throttle egress bandwidth through a proxy to `rate_bps` bytes/second.
///
/// # Errors
///
/// Returns [`NetError`] if the daemon cannot be located or spawned.
#[tracing::instrument]
#[must_use = "callers must check whether the throttle fault was injected"]
pub async fn throttle_bandwidth(
    listen: SocketAddr,
    upstream: SocketAddr,
    rate_bps: usize,
) -> Result<String, NetError> {
    let listen_s = listen.to_string();
    let _span = crate::telemetry::begin_throttle_bandwidth(&listen_s, rate_bps);
    let profile = FaultProfile {
        rate_bps,
        ..FaultProfile::default()
    };
    let pid = spawn_proxy(listen, upstream, profile).await?;
    crate::telemetry::event_proxy_started(pid);
    Ok(format!(
        "bandwidth throttle active on {listen_s}: {rate_bps} B/s (pid {pid})"
    ))
}

/// Fragment writes through a proxy into fixed `slice_bytes`-sized segments.
///
/// # Errors
///
/// Returns [`NetError`] if the daemon cannot be located or spawned.
#[tracing::instrument]
#[must_use = "callers must check whether the fragmentation fault was injected"]
pub async fn fragment_stream(
    listen: SocketAddr,
    upstream: SocketAddr,
    slice_bytes: usize,
) -> Result<String, NetError> {
    let listen_s = listen.to_string();
    let _span = crate::telemetry::begin_fragment_stream(&listen_s, slice_bytes);
    let profile = FaultProfile {
        slice_bytes,
        ..FaultProfile::default()
    };
    let pid = spawn_proxy(listen, upstream, profile).await?;
    crate::telemetry::event_proxy_started(pid);
    Ok(format!(
        "fragmentation fault active on {listen_s}: {slice_bytes}-byte slices (pid {pid})"
    ))
}

/// Corrupt forwarded bytes with per-byte `probability`.
///
/// The corruption RNG is seeded from `seed`, so the exact byte edits are
/// reproducible across runs (via `tokio-netem`'s `Corrupter::from_seed`).
///
/// # Errors
///
/// Returns [`NetError`] if `probability` is out of range or the daemon cannot
/// be located or spawned.
#[tracing::instrument]
#[must_use = "callers must check whether the corruption fault was injected"]
pub async fn corrupt_bytes(
    listen: SocketAddr,
    upstream: SocketAddr,
    probability: f64,
    seed: u64,
) -> Result<String, NetError> {
    let listen_s = listen.to_string();
    let _span = crate::telemetry::begin_corrupt_bytes(&listen_s, probability);
    let profile = FaultProfile {
        corrupt_prob: probability,
        seed,
        ..FaultProfile::default()
    };
    let pid = spawn_proxy(listen, upstream, profile).await?;
    crate::telemetry::event_proxy_started(pid);
    Ok(format!(
        "corruption fault active on {listen_s}: p={probability} (pid {pid})"
    ))
}

/// Probabilistically hard-close forwarded connections mid-stream.
///
/// The termination RNG is seeded from `seed` for reproducibility (via
/// `tokio-netem`'s `Terminator::from_seed`).
///
/// # Errors
///
/// Returns [`NetError`] if `probability` is out of range or the daemon cannot
/// be located or spawned.
#[tracing::instrument]
#[must_use = "callers must check whether the termination fault was injected"]
pub async fn terminate_connections(
    listen: SocketAddr,
    upstream: SocketAddr,
    probability: f64,
    seed: u64,
) -> Result<String, NetError> {
    let listen_s = listen.to_string();
    let _span = crate::telemetry::begin_terminate_connections(&listen_s, probability);
    let profile = FaultProfile {
        terminate_prob: probability,
        seed,
        ..FaultProfile::default()
    };
    let pid = spawn_proxy(listen, upstream, profile).await?;
    crate::telemetry::event_proxy_started(pid);
    Ok(format!(
        "termination fault active on {listen_s}: p={probability} (pid {pid})"
    ))
}

/// Confirm that `pid` belongs to a live `tumult-net-proxyd` daemon.
///
/// The pidfile lives in the world-writable OS temp dir, so PID reuse or a
/// planted file must never cause [`stop_proxy`] to kill an unrelated process:
/// on Linux `/proc/<pid>/cmdline` must contain the proxyd binary name. A
/// process that cannot be identified (already exited, so no `/proc` entry) is
/// conservatively treated as *not ours*.
#[cfg(target_os = "linux")]
async fn is_proxyd_process(pid: u32) -> bool {
    match tokio::fs::read(format!("/proc/{pid}/cmdline")).await {
        Ok(cmdline) => String::from_utf8_lossy(&cmdline).contains(PROXYD_BIN),
        Err(_) => false,
    }
}

/// Off Linux there is no portable `/proc`-style process-identity source, so
/// a pid that can be signalled counts as the daemon and the historic
/// best-effort kill is kept (a world-writable shared temp dir is primarily a
/// Linux deployment shape). A pid that cannot be signalled is no live daemon:
/// its pidfile is stale.
#[cfg(all(unix, not(target_os = "linux")))]
async fn is_proxyd_process(pid: u32) -> bool {
    can_be_signalled(pid).await
}

/// Without `kill` every pid counts as the daemon.
#[cfg(not(unix))]
async fn is_proxyd_process(_pid: u32) -> bool {
    true
}

/// Whether `kill -0` reaches `pid`: a process of this user that is not
/// reaped yet.
#[cfg(any(test, all(unix, not(target_os = "linux"))))]
async fn can_be_signalled(pid: u32) -> bool {
    tokio::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok_and(|status| status.success())
}

/// Stop the chaos proxy bound to `listen` and remove its pidfile.
///
/// This is the rollback for every disruptive action above. It is idempotent:
/// if no pidfile exists it returns `Ok` without error, and a best-effort kill
/// tolerates a daemon that has already exited.
///
/// # Errors
///
/// Returns [`NetError::Io`] only if the pidfile exists but cannot be read for a
/// reason other than absence.
#[tracing::instrument]
#[must_use = "callers must check whether the proxy rollback succeeded"]
pub async fn stop_proxy(listen: SocketAddr) -> Result<String, NetError> {
    let listen_s = listen.to_string();
    let _span = crate::telemetry::begin_stop_proxy(&listen_s);
    let pidfile = pidfile_path(listen);

    let Some(pid) = pidfile::read(&pidfile).await? else {
        crate::telemetry::event_proxy_stopped(None);
        return Ok(format!("no chaos proxy running on {listen_s}"));
    };

    // Never signal a process we cannot identify as our daemon (see
    // `is_proxyd_process`): refuse the kill, drop the stale pidfile, and
    // report instead of killing whatever happens to hold the PID today.
    if !is_proxyd_process(pid).await {
        let _ = tokio::fs::remove_file(&pidfile).await;
        remove_startup_log(listen).await;
        crate::telemetry::event_proxy_stopped(None);
        return Ok(format!(
            "no chaos proxy running on {listen_s} (removed stale pidfile: pid {pid} is not a live `{PROXYD_BIN}`)"
        ));
    }

    // Verified proxyd process; best-effort terminate — the daemon may still
    // have exited between the identity check and the kill.
    terminate(pid).await;
    let _ = tokio::fs::remove_file(&pidfile).await;
    remove_startup_log(listen).await;
    crate::telemetry::event_proxy_stopped(Some(pid));
    Ok(format!("chaos proxy on {listen_s} stopped (pid {pid})"))
}

#[cfg(test)]
mod tests {
    use tumult_test_utils::EnvGuard;

    use super::*;

    fn addr(s: &str) -> SocketAddr {
        s.parse().expect("valid socket addr")
    }

    /// Whether a log directory of the proxy on `listen` exists anywhere.
    fn log_dir_exists(listen: SocketAddr) -> bool {
        log_dirs(listen).iter().any(|dir| dir.exists())
    }

    /// A loopback address picked by the kernel, with the listener that owns
    /// it. The pidfile is keyed on the address, so the caller keeps the
    /// listener for as long as it uses the pidfile: while it is bound, the
    /// kernel cannot hand the same port to another test.
    fn reserved_addr() -> (std::net::TcpListener, SocketAddr) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind an ephemeral port");
        let addr = listener.local_addr().expect("read the bound address");
        (listener, addr)
    }

    #[test]
    fn pidfile_path_is_stable_and_sanitised() {
        let a = pidfile_path(addr("127.0.0.1:8080"));
        let b = pidfile_path(addr("127.0.0.1:8080"));
        assert_eq!(a, b);
        let name = a.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("tumult-net-"));
        assert_eq!(a.extension().and_then(|e| e.to_str()), Some("pid"));
        assert!(!name.contains(':'));
    }

    #[tokio::test]
    async fn stop_proxy_is_safe_when_nothing_is_running() {
        // A port keyed pidfile that certainly does not exist.
        let out = stop_proxy(addr("127.0.0.1:1")).await.expect("idempotent");
        assert!(out.contains("no chaos proxy running"));
    }

    #[tokio::test]
    async fn stop_proxy_removes_a_stale_pidfile() {
        let (_reserved, listen) = reserved_addr();
        let pidfile = pidfile_path(listen);
        // Simulate a leftover pidfile pointing at a long-dead PID. On Linux the
        // identity check refuses the kill; on another Unix a pid that cannot
        // be signalled is no live daemon. Either way the stale file is
        // cleaned up. Without Unix every pid counts as the daemon, and the
        // best-effort kill path runs instead. All paths remove the pidfile.
        tokio::fs::write(&pidfile, "999999999").await.unwrap();
        // ... and the log that daemon left behind.
        // Wherever it was put: the stop may run in another environment
        // than the start did.
        for dir in log_dirs(listen) {
            let (log, _file) = StartupLog::create(&dir).unwrap();
            log.keep();
            assert!(dir.exists());
        }
        let out = stop_proxy(listen).await.expect("rollback");
        #[cfg(unix)]
        assert!(out.contains("stale pidfile"), "out: {out}");
        #[cfg(not(unix))]
        assert!(out.contains("stopped"), "out: {out}");
        assert!(!pidfile.exists());
        assert!(!log_dir_exists(listen), "the log goes with the pidfile");
    }

    #[tokio::test]
    async fn start_proxy_rejects_listen_equal_to_upstream() {
        // Rejected before any daemon lookup/spawn, so no proxyd is needed.
        let err = start_proxy(
            addr("127.0.0.1:65000"),
            addr("127.0.0.1:65000"),
            FaultProfile::default(),
        )
        .await
        .expect_err("a proxy loop must be rejected");
        assert!(err.to_string().contains("proxy loop"), "err: {err}");
    }

    /// A pidfile pointing at the test runner itself (not a proxyd) must not be
    /// signalled — if the identity check regresses this test dies loudly.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn stop_proxy_refuses_to_kill_a_non_proxyd_process() {
        let (_reserved, listen) = reserved_addr();
        let pidfile = pidfile_path(listen);
        tokio::fs::write(&pidfile, std::process::id().to_string())
            .await
            .unwrap();
        let out = stop_proxy(listen).await.expect("rollback");
        assert!(out.contains("stale pidfile"), "out: {out}");
        assert!(!pidfile.exists());
    }

    /// A process whose cmdline carries the proxyd name is killed as before.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn stop_proxy_kills_a_verified_proxyd_process() {
        use std::os::unix::process::CommandExt;

        // argv[0] IS the proxyd name from the moment of exec: no shell
        // (`dash` lacks `exec -a`, so the previous `sh -c` stand-in never
        // started on Debian/Ubuntu runners) and no mid-exec /proc race.
        let Ok(mut child) = std::process::Command::new("sleep")
            .arg0(PROXYD_BIN)
            .arg("60")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            return; // no `sleep` on this host — nothing to test
        };
        let (_reserved, listen) = reserved_addr();
        let pidfile = pidfile_path(listen);
        tokio::fs::write(&pidfile, child.id().to_string())
            .await
            .unwrap();

        // Wait for the exec to land: /proc/<pid>/cmdline is empty between
        // fork and execve, and a loaded CI runner can delay the child's
        // first scheduling long enough for stop_proxy to read it mid-exec
        // and refuse the kill as "stale".
        let cmdline = format!("/proc/{}/cmdline", child.id());
        let mut execd = false;
        for _ in 0..100 {
            if tokio::fs::read(&cmdline)
                .await
                .is_ok_and(|c| String::from_utf8_lossy(&c).contains(PROXYD_BIN))
            {
                execd = true;
                break;
            }
            // sleep: interval of the bounded poll of /proc above.
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(execd, "stand-in proxyd never exec'd");

        let out = stop_proxy(listen).await.expect("rollback");
        assert!(out.contains("stopped"), "out: {out}");
        assert!(!pidfile.exists());
        let status = child.wait().expect("reap stand-in daemon");
        assert!(!status.success(), "stand-in should have been signalled");
    }

    fn unique_temp_file(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("tumult-net-test-{tag}-{}", std::process::id()))
    }

    /// The pid a message names as `(pid N)`.
    #[cfg(unix)]
    fn pid_named_in(text: &str) -> Option<u32> {
        let (_, rest) = text.split_once("(pid ")?;
        rest.split(|c: char| !c.is_ascii_digit())
            .next()?
            .parse()
            .ok()
    }

    /// Whether `pid` is a process of the host, a zombie included.
    #[cfg(unix)]
    fn process_exists(pid: u32) -> bool {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    /// `TUMULT_NET_PROXYD` is process-global, so every scenario that mutates it
    /// is driven sequentially from this one test.
    #[tokio::test]
    async fn proxyd_env_override_scenarios() {
        // 1. An override pointing at an existing file wins over any daemon
        //    found beside the test executable.
        let real_file = unique_temp_file("real-proxyd");
        std::fs::write(&real_file, b"not really a daemon").unwrap();
        let mut env = EnvGuard::lock();
        env.set(PROXYD_ENV, &real_file);
        let found = locate_proxyd().expect("an existing override file must be honoured");
        assert_eq!(found, real_file);

        // 2. An override pointing at a nonexistent path falls through to the
        //    sibling-daemon search beside the current executable; with no such
        //    daemon the error must name the env var. Which branch runs depends
        //    on the build layout (Cargo places `tumult-net-proxyd` one
        //    directory up from the test binary when it is built).
        let missing = unique_temp_file("missing-proxyd");
        env.set(PROXYD_ENV, &missing);
        let exe = std::env::current_exe().unwrap();
        let dir = exe.parent().unwrap();
        let sibling_exists =
            dir.join(PROXYD_BIN).is_file() || dir.join("..").join(PROXYD_BIN).is_file();
        match locate_proxyd() {
            Err(err) => {
                assert!(!sibling_exists, "a sibling daemon exists but was not found");
                assert!(err.to_string().contains(PROXYD_ENV), "err: {err}");
            }
            Ok(path) => {
                assert!(
                    sibling_exists,
                    "no sibling daemon exists, yet one was found"
                );
                assert!(path.ends_with(PROXYD_BIN), "path: {}", path.display());
            }
        }

        // 3. A stand-in that never serves: it runs but never records its pid,
        //    so the start must fail when the wait is over, surfacing the
        //    stand-in's captured stderr, and leave no pidfile, no log and no
        //    process behind.
        let script = unique_temp_file("dead-proxyd");
        #[cfg(unix)]
        tumult_test_utils::write_executable(&script, "#!/bin/sh\necho boom >&2\nexec sleep 30\n");
        #[cfg(not(unix))]
        std::fs::write(&script, b"#!/bin/sh\necho boom >&2\nexec sleep 30\n").unwrap();
        env.set(PROXYD_ENV, &script);
        // A port this test holds: the pidfile is keyed on the address, and
        // no proxy can serve an address that is taken.
        let (_reserved, listen) = reserved_addr();
        let err = start_proxy(listen, addr("127.0.0.1:9"), FaultProfile::default())
            .await
            .expect_err("a daemon that never records its pid must fail the start");
        let msg = err.to_string();
        assert!(msg.contains("did not record its pid"), "err: {msg}");
        assert!(msg.contains("boom"), "captured stderr missing, err: {msg}");
        assert!(
            !pidfile_path(listen).exists(),
            "no pidfile is left when the daemon never comes up"
        );
        assert!(
            !log_dir_exists(listen),
            "the log must be removed when the daemon never comes up"
        );
        #[cfg(unix)]
        {
            let pid = pid_named_in(&msg).expect("the error names the stand-in's pid");
            assert!(!process_exists(pid), "the stand-in {pid} is reaped");
        }
        let _ = std::fs::remove_file(&script);

        // 4. An override pointing at an existing but non-executable file fails
        //    the spawn itself.
        let not_exec = unique_temp_file("not-exec-proxyd");
        std::fs::write(&not_exec, b"plain text, no exec bit").unwrap();
        env.set(PROXYD_ENV, &not_exec);
        let err = start_proxy(listen, addr("127.0.0.1:9"), FaultProfile::default())
            .await
            .expect_err("a non-executable daemon path must fail to spawn");
        assert!(matches!(err, NetError::Io(_)), "err: {err}");
        assert!(
            !log_dir_exists(listen),
            "a daemon that never started must leave no log behind"
        );
        let _ = std::fs::remove_file(&not_exec);
        let _ = std::fs::remove_file(&real_file);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_pid_can_be_signalled_while_its_process_runs_and_not_after_it_is_reaped() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        let pid = child.id();

        let while_it_runs = can_be_signalled(pid).await;
        child.kill().expect("kill sleep");
        child.wait().expect("reap sleep");
        let after = can_be_signalled(pid).await;

        assert!(while_it_runs, "a running process can be signalled");
        assert!(!after, "a reaped process cannot");
    }

    #[tokio::test]
    async fn invalid_profile_fails_validation_before_daemon_lookup() {
        // `FaultProfile::validate` runs before any daemon discovery, so this
        // fails deterministically regardless of whether a proxyd exists; the
        // typed field name proves the validation branch ran.
        let profile = FaultProfile {
            corrupt_prob: 1.5,
            ..FaultProfile::default()
        };
        let err = start_proxy(addr("127.0.0.1:0"), addr("127.0.0.1:9"), profile)
            .await
            .expect_err("an out-of-range probability must be rejected");
        match err {
            NetError::InvalidConfig { field, .. } => assert_eq!(field, "corrupt_prob"),
            other @ NetError::Io(_) => panic!("expected InvalidConfig, got: {other}"),
        }
    }

    #[tokio::test]
    async fn proxy_loop_is_rejected_by_every_fault_action() {
        // listen == upstream is rejected before any daemon lookup, so every
        // action body runs deterministically with no proxyd present.
        let same = addr("127.0.0.1:64123");
        let results = [
            start_proxy(same, same, FaultProfile::default()).await,
            inject_latency(same, same, 10, 5, 42).await,
            throttle_bandwidth(same, same, 1024).await,
            fragment_stream(same, same, 64).await,
            corrupt_bytes(same, same, 0.5, 7).await,
            terminate_connections(same, same, 0.25, 7).await,
        ];
        for out in results {
            let err = out.expect_err("a proxy loop must be rejected");
            assert!(err.to_string().contains("proxy loop"), "err: {err}");
        }
    }

    #[tokio::test]
    async fn stop_proxy_treats_a_garbage_pidfile_as_nothing_running() {
        let (_reserved, listen) = reserved_addr();
        let pidfile = pidfile_path(listen);
        tokio::fs::write(&pidfile, "not-a-pid").await.unwrap();
        let out = stop_proxy(listen).await.expect("idempotent rollback");
        assert!(out.contains("no chaos proxy running"), "out: {out}");
        // An unparseable pidfile is reported as "nothing running" but left in
        // place (only a verified-stale one is removed); tidy it up here.
        assert!(pidfile.exists(), "a garbage pidfile is not removed");
        let _ = tokio::fs::remove_file(&pidfile).await;
    }

    #[test]
    fn a_kill_that_cannot_be_run_is_a_warning() {
        let failure = Err(std::io::Error::other("kill: command not found"));
        let logs = tumult_otel::logging::testing::capture_json(tracing::Level::DEBUG, || {
            log_kill_result(4242, &failure);
        });
        let event = &logs.named("net.proxy.kill.failed")[0];
        assert_eq!(event["level"], "WARN");
        assert_eq!(event["process.pid"], 4242);
        assert_eq!(event["error.type"], "io");
        assert_eq!(event["error.message"], "kill: command not found");
    }

    #[cfg(unix)]
    #[test]
    fn only_a_kill_that_did_not_signal_is_logged() {
        use std::os::unix::process::ExitStatusExt;

        // Wait status 0x0100 is "exited with code 1": the daemon was already
        // gone, which rollback tolerates. Status 0 is a delivered signal.
        let rejected = Ok(std::process::ExitStatus::from_raw(0x0100));
        let delivered = Ok(std::process::ExitStatus::from_raw(0));
        let logs = tumult_otel::logging::testing::capture_json(tracing::Level::DEBUG, || {
            log_kill_result(4242, &delivered);
            log_kill_result(4243, &rejected);
        });
        // One event for the two kills, and it is the rejected one.
        assert_eq!(logs.events.len(), 1, "{}", logs.raw);
        let event = &logs.events[0];
        assert_eq!(event["event.name"], "net.proxy.kill.rejected");
        assert_eq!(event["level"], "DEBUG");
        assert_eq!(event["process.pid"], 4243);
        assert_eq!(event["process.exit.code"], 1);
    }

    #[test]
    fn a_startup_failure_reason_is_the_structured_cause_or_else_the_raw_text() {
        let structured = concat!(
            r#"{"timestamp":"2026-01-01T00:00:00.000000Z","level":"ERROR","#,
            r#""event.name":"net.proxyd.failed","message":"proxy failed","#,
            r#""error.message":"Address already in use (os error 98)"}"#,
            "\n"
        );
        // A JSON line without a cause is no better than text: shown whole.
        let causeless = r#"{"level":"INFO","message":"listening"}"#;
        let cases = [
            (
                structured.to_string(),
                "daemon stderr: Address already in use (os error 98)".to_string(),
            ),
            (
                format!("starting\n{structured}"),
                "daemon stderr: Address already in use (os error 98)".to_string(),
            ),
            (
                "thread 'main' panicked\n".to_string(),
                "daemon stderr: thread 'main' panicked".to_string(),
            ),
            (causeless.to_string(), format!("daemon stderr: {causeless}")),
            ("  \n".to_string(), "daemon produced no output".to_string()),
        ];
        for (stderr, expected) in cases {
            assert_eq!(startup_failure_reason(&stderr), expected, "{stderr:?}");
        }
    }
}
