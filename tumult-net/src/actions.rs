//! TCP chaos-proxy actions.
//!
//! Every action is a plain async free function dispatched by the CLI. Because
//! the CLI runs each action in a fresh `sync_await` call, a
//! long-running proxy fault cannot live in process memory: the disruptive
//! `start` functions spawn a detached `tumult-net-proxyd` daemon and record its
//! PID in a pidfile under the OS temp directory, and the [`stop_proxy`] rollback
//! tears that daemon down from a fresh invocation. `stop_proxy` is idempotent
//! and safe to call when no proxy is running.
//!
//! Two guards make the detached-daemon hand-off safe:
//!
//! - **Readiness** — after spawning, the daemon's listen address is polled
//!   until it accepts a connection (with a short deadline); a dead-on-arrival
//!   daemon (e.g. the port was already bound) fails the start with its
//!   captured stderr instead of leaving a pidfile pointing at a dead PID.
//! - **Identity** — [`stop_proxy`] verifies the pidfile's PID actually belongs
//!   to a `tumult-net-proxyd` process before signalling it, so a stale or
//!   planted pidfile in the world-writable temp dir cannot kill an unrelated
//!   process.

use std::io::ErrorKind;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use crate::config::{FaultProfile, ProxySpec};
use crate::error::NetError;

/// Environment variable that overrides daemon discovery (used by tests and by
/// non-standard install layouts).
const PROXYD_ENV: &str = "TUMULT_NET_PROXYD";
/// The daemon binary name shipped alongside the CLI.
const PROXYD_BIN: &str = "tumult-net-proxyd";
/// How long a freshly spawned daemon gets to accept its first connection
/// before the start is declared failed.
const READY_TIMEOUT: Duration = Duration::from_secs(2);
/// Interval between readiness probes of the daemon's listen address.
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

/// Derive the path of the startup log capturing the daemon's stderr, so a
/// dead-on-arrival daemon's error can be surfaced by the readiness check.
fn stderr_log_path(listen: SocketAddr) -> PathBuf {
    pidfile_path(listen).with_extension("stderr.log")
}

/// Poll `listen` until the daemon accepts a TCP connection or [`READY_TIMEOUT`]
/// elapses. A successful probe is dropped immediately; the proxy sees it as an
/// aborted client connection.
async fn wait_ready(listen: SocketAddr) -> bool {
    tokio::time::timeout(READY_TIMEOUT, async {
        loop {
            if tokio::net::TcpStream::connect(listen).await.is_ok() {
                return;
            }
            tokio::time::sleep(READY_POLL).await;
        }
    })
    .await
    .is_ok()
}

/// Spawn a detached daemon for `spec`, verify it comes up, write its pidfile,
/// and return the PID.
///
/// After the spawn the daemon's listen address is polled for up to
/// [`READY_TIMEOUT`]. If it never accepts a connection (e.g. the port was
/// already bound) the daemon's captured stderr is returned as the error, any
/// half-alive process is signalled, and the pidfile is removed rather than
/// left pointing at a dead PID.
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
    let stderr_log = stderr_log_path(listen);
    let spec = ProxySpec {
        listen,
        upstream,
        profile,
        pidfile: pidfile.clone(),
    };

    // The detached daemon must outlive this CLI invocation, so it is launched
    // via std (which never kills the child on drop). stderr goes to a startup
    // log file instead of the null device so the readiness check can report
    // *why* a dead-on-arrival daemon failed.
    let stderr_file = std::fs::File::create(&stderr_log)?;
    let child = std::process::Command::new(&daemon)
        .args(spec.to_argv())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr_file))
        .spawn()?;
    let pid = child.id();
    drop(child);

    tokio::fs::write(&pidfile, pid.to_string()).await?;

    if wait_ready(listen).await {
        // Came up cleanly; nothing more will be written to the startup log.
        let _ = tokio::fs::remove_file(&stderr_log).await;
        return Ok(pid);
    }

    // Dead on arrival: surface the daemon's captured stderr, stop any
    // half-alive process, and leave no pidfile behind for rollback to chase.
    let details = tokio::fs::read_to_string(&stderr_log)
        .await
        .unwrap_or_default();
    let _ = tokio::process::Command::new("kill")
        .arg(pid.to_string())
        .status()
        .await;
    let _ = tokio::fs::remove_file(&pidfile).await;
    let _ = tokio::fs::remove_file(&stderr_log).await;
    let detail = details.trim();
    let reason = if detail.is_empty() {
        "daemon produced no output".to_string()
    } else {
        format!("daemon stderr: {detail}")
    };
    Err(NetError::invalid(
        "proxyd",
        format!(
            "`{PROXYD_BIN}` did not accept connections on {listen} within {}s ({reason})",
            READY_TIMEOUT.as_secs()
        ),
    ))
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
/// the historic best-effort kill is kept (a world-writable shared temp dir is
/// primarily a Linux deployment shape).
#[cfg(not(target_os = "linux"))]
async fn is_proxyd_process(_pid: u32) -> bool {
    true
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

    let pid = match tokio::fs::read_to_string(&pidfile).await {
        Ok(contents) => contents.trim().parse::<u32>().ok(),
        Err(e) if e.kind() == ErrorKind::NotFound => None,
        Err(e) => return Err(NetError::Io(e)),
    };

    let Some(pid) = pid else {
        crate::telemetry::event_proxy_stopped(None);
        return Ok(format!("no chaos proxy running on {listen_s}"));
    };

    // Never signal a process we cannot identify as our daemon (see
    // `is_proxyd_process`): refuse the kill, drop the stale pidfile, and
    // report instead of killing whatever happens to hold the PID today.
    if !is_proxyd_process(pid).await {
        let _ = tokio::fs::remove_file(&pidfile).await;
        crate::telemetry::event_proxy_stopped(None);
        return Ok(format!(
            "no chaos proxy running on {listen_s} (removed stale pidfile: pid {pid} is not a live `{PROXYD_BIN}`)"
        ));
    }

    // Verified proxyd process; best-effort terminate — the daemon may still
    // have exited between the identity check and the kill.
    let _ = tokio::process::Command::new("kill")
        .arg(pid.to_string())
        .status()
        .await;
    let _ = tokio::fs::remove_file(&pidfile).await;
    crate::telemetry::event_proxy_stopped(Some(pid));
    Ok(format!("chaos proxy on {listen_s} stopped (pid {pid})"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(s: &str) -> SocketAddr {
        s.parse().expect("valid socket addr")
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
        let listen = addr("127.0.0.1:65533");
        let pidfile = pidfile_path(listen);
        // Simulate a leftover pidfile pointing at a long-dead PID. The identity
        // check refuses the kill and the stale file is cleaned up instead.
        tokio::fs::write(&pidfile, "999999999").await.unwrap();
        let out = stop_proxy(listen).await.expect("rollback");
        assert!(out.contains("stale pidfile"), "out: {out}");
        assert!(!pidfile.exists());
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
        let listen = addr("127.0.0.1:65532");
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
        let Ok(mut child) = std::process::Command::new("sh")
            .args(["-c", "exec -a tumult-net-proxyd sleep 60"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            return; // no `sh` on this host — nothing to test
        };
        // Wait until the stand-in has fully exec'd (its argv[0] IS the proxyd
        // name) before writing the pidfile. Reading /proc/<pid>/cmdline while
        // the process is mid-exec can yield an empty string, which would fail
        // the identity check below non-deterministically under parallel load.
        let mut execd = false;
        for _ in 0..100 {
            if let Ok(cmdline) = tokio::fs::read(format!("/proc/{}/cmdline", child.id())).await {
                if cmdline.split(|b| *b == 0).next() == Some(PROXYD_BIN.as_bytes()) {
                    execd = true;
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        if !execd {
            let _ = child.kill();
            panic!("stand-in daemon never exec'd as `{PROXYD_BIN}`");
        }
        let listen = addr("127.0.0.1:65531");
        let pidfile = pidfile_path(listen);
        tokio::fs::write(&pidfile, child.id().to_string())
            .await
            .unwrap();

        let out = stop_proxy(listen).await.expect("rollback");
        assert!(out.contains("stopped"), "out: {out}");
        assert!(!pidfile.exists());
        let status = child.wait().expect("reap stand-in daemon");
        assert!(!status.success(), "stand-in should have been signalled");
    }
}
