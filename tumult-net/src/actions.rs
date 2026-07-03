//! TCP chaos-proxy actions.
//!
//! Every action is a plain async free function dispatched by the CLI. Because
//! the CLI runs each action in a fresh `block_in_place` + `block_on` call, a
//! long-running proxy fault cannot live in process memory: the disruptive
//! `start` functions spawn a detached `tumult-net-proxyd` daemon and record its
//! PID in a pidfile under the OS temp directory, and the [`stop_proxy`] rollback
//! tears that daemon down from a fresh invocation. `stop_proxy` is idempotent
//! and safe to call when no proxy is running.

use std::io::ErrorKind;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Stdio;

use crate::config::{FaultProfile, ProxySpec};
use crate::error::NetError;

/// Environment variable that overrides daemon discovery (used by tests and by
/// non-standard install layouts).
const PROXYD_ENV: &str = "TUMULT_NET_PROXYD";
/// The daemon binary name shipped alongside the CLI.
const PROXYD_BIN: &str = "tumult-net-proxyd";

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

/// Spawn a detached daemon for `spec`, write its pidfile, and return the PID.
async fn spawn_proxy(
    listen: SocketAddr,
    upstream: SocketAddr,
    profile: FaultProfile,
) -> Result<u32, NetError> {
    profile.validate()?;
    let daemon = locate_proxyd()?;
    let pidfile = pidfile_path(listen);
    let spec = ProxySpec {
        listen,
        upstream,
        profile,
        pidfile: pidfile.clone(),
    };

    // Fire-and-forget: the detached daemon must outlive this CLI invocation, so
    // it is launched via std (which never kills the child on drop) with its
    // standard streams pointed at the null device.
    let child = std::process::Command::new(&daemon)
        .args(spec.to_argv())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let pid = child.id();
    drop(child);

    tokio::fs::write(&pidfile, pid.to_string()).await?;
    Ok(pid)
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

    // Best-effort terminate; the daemon may already have exited.
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
        // Simulate a leftover pidfile pointing at a long-dead PID.
        tokio::fs::write(&pidfile, "999999999").await.unwrap();
        let out = stop_proxy(listen).await.expect("rollback");
        assert!(out.contains("stopped"));
        assert!(!pidfile.exists());
    }
}
