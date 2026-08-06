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
        // Simulate a leftover pidfile pointing at a long-dead PID. On Linux the
        // identity check refuses the kill and the stale file is cleaned up;
        // off Linux there is no /proc identity source, so the documented
        // best-effort kill path runs instead. Both paths remove the pidfile.
        tokio::fs::write(&pidfile, "999999999").await.unwrap();
        let out = stop_proxy(listen).await.expect("rollback");
        #[cfg(target_os = "linux")]
        assert!(out.contains("stale pidfile"), "out: {out}");
        #[cfg(not(target_os = "linux"))]
        assert!(out.contains("stopped"), "out: {out}");
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
        let listen = addr("127.0.0.1:65531");
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
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(execd, "stand-in proxyd never exec'd");

        let out = stop_proxy(listen).await.expect("rollback");
        assert!(out.contains("stopped"), "out: {out}");
        assert!(!pidfile.exists());
        let status = child.wait().expect("reap stand-in daemon");
        assert!(!status.success(), "stand-in should have been signalled");
    }

    /// Restore an env var to its prior value on drop, so a panicking scenario
    /// cannot leak `TUMULT_NET_PROXYD` into other tests in this process.
    struct EnvGuard {
        key: &'static str,
        saved: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &std::path::Path) -> Self {
            let saved = std::env::var(key).ok();
            std::env::set_var(key, value);
            Self { key, saved }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.saved {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }

    fn unique_temp_file(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("tumult-net-test-{tag}-{}", std::process::id()))
    }

    /// `TUMULT_NET_PROXYD` is process-global, so every scenario that mutates it
    /// is driven sequentially from this one test.
    #[tokio::test]
    async fn proxyd_env_override_scenarios() {
        // 1. An override pointing at an existing file wins over any daemon
        //    found beside the test executable.
        let real_file = unique_temp_file("real-proxyd");
        std::fs::write(&real_file, b"not really a daemon").unwrap();
        {
            let _guard = EnvGuard::set(PROXYD_ENV, &real_file);
            let found = locate_proxyd().expect("an existing override file must be honoured");
            assert_eq!(found, real_file);
        }

        // 2. An override pointing at a nonexistent path falls through to the
        //    sibling-daemon search beside the current executable; with no such
        //    daemon the error must name the env var. Which branch runs depends
        //    on the build layout (Cargo places `tumult-net-proxyd` one
        //    directory up from the test binary when it is built).
        let missing = unique_temp_file("missing-proxyd");
        let _guard = EnvGuard::set(PROXYD_ENV, &missing);
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

        // 3. Dead-on-arrival daemon: the script runs but never binds the
        //    listen address, so the start must fail surfacing the daemon's
        //    captured stderr, and must not leave a pidfile behind.
        let script = unique_temp_file("dead-proxyd");
        std::fs::write(&script, b"#!/bin/sh\necho boom >&2\nsleep 30\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let _guard = EnvGuard::set(PROXYD_ENV, &script);
        let listen = addr("127.0.0.1:0");
        let err = start_proxy(listen, addr("127.0.0.1:9"), FaultProfile::default())
            .await
            .expect_err("a daemon that never binds must fail the start");
        let msg = err.to_string();
        assert!(msg.contains("did not accept connections"), "err: {msg}");
        assert!(msg.contains("boom"), "captured stderr missing, err: {msg}");
        assert!(
            !pidfile_path(listen).exists(),
            "pidfile must be removed when the daemon never comes up"
        );
        let _ = std::fs::remove_file(&script);

        // 4. An override pointing at an existing but non-executable file fails
        //    the spawn itself.
        let not_exec = unique_temp_file("not-exec-proxyd");
        std::fs::write(&not_exec, b"plain text, no exec bit").unwrap();
        let _guard = EnvGuard::set(PROXYD_ENV, &not_exec);
        let err = start_proxy(listen, addr("127.0.0.1:9"), FaultProfile::default())
            .await
            .expect_err("a non-executable daemon path must fail to spawn");
        assert!(matches!(err, NetError::Io(_)), "err: {err}");
        // The failed spawn leaves the startup log behind; tidy it up here.
        let _ = std::fs::remove_file(stderr_log_path(listen));
        let _ = std::fs::remove_file(&not_exec);
        let _ = std::fs::remove_file(&real_file);
    }

    #[tokio::test]
    async fn wait_ready_distinguishes_bound_from_closed_ports() {
        let listener = tokio::net::TcpListener::bind(addr("127.0.0.1:0"))
            .await
            .expect("bind ephemeral port");
        let bound = listener.local_addr().unwrap();
        let started = std::time::Instant::now();
        assert!(wait_ready(bound).await, "a listening port must be ready");
        assert!(
            started.elapsed() < READY_TIMEOUT,
            "readiness on a bound port must be prompt"
        );
        drop(listener);

        // Grab an ephemeral port and release it so nothing is listening.
        // On a shared runner another process can win the race and rebind the
        // just-released port between the release and the readiness probe
        // (observed on aarch64-apple-darwin in the v2.21.0 release gate), so
        // retry with a fresh port a few times before calling it a failure.
        let mut failing_elapsed = None;
        for _ in 0..5 {
            let probe = std::net::TcpListener::bind(addr("127.0.0.1:0")).expect("bind ephemeral");
            let candidate = probe.local_addr().unwrap();
            drop(probe);
            let started = std::time::Instant::now();
            if !wait_ready(candidate).await {
                failing_elapsed = Some(started.elapsed());
                break;
            }
        }
        let elapsed =
            failing_elapsed.expect("a closed port must never report ready (5 attempts, all ready)");
        assert!(
            elapsed >= Duration::from_millis(1500),
            "a closed port should only fail after the readiness deadline"
        );
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
        let listen = addr("127.0.0.1:59993");
        let pidfile = pidfile_path(listen);
        tokio::fs::write(&pidfile, "not-a-pid").await.unwrap();
        let out = stop_proxy(listen).await.expect("idempotent rollback");
        assert!(out.contains("no chaos proxy running"), "out: {out}");
        // An unparseable pidfile is reported as "nothing running" but left in
        // place (only a verified-stale one is removed); tidy it up here.
        assert!(pidfile.exists(), "a garbage pidfile is not removed");
        let _ = tokio::fs::remove_file(&pidfile).await;
    }
}
