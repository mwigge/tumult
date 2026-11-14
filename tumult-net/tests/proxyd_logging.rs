//! `tumult-net-proxyd` reports its failures as structured events on stderr.

use std::process::Command;

use serde_json::Value;

#[test]
fn invalid_arguments_end_with_one_structured_error_on_stderr() {
    let output = Command::new(env!("CARGO_BIN_EXE_tumult-net-proxyd"))
        .arg("--no-such-flag")
        .env_remove("RUST_LOG")
        .env_remove("TUMULT_LOG_FORMAT")
        .output()
        .expect("run tumult-net-proxyd");

    assert!(!output.status.success());
    assert!(output.stdout.is_empty(), "stdout carries no log lines");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let events: Vec<Value> = stderr
        .lines()
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("stderr line is not JSON ({e}): {line}"))
        })
        .collect();
    assert_eq!(events.len(), 1, "{stderr}");
    assert_eq!(events[0]["event.name"], "net.proxyd.args.invalid");
    assert_eq!(events[0]["level"], "ERROR");
    assert_eq!(events[0]["service.name"], "tumult-net-proxyd");
    assert!(events[0]["error.message"].is_string(), "{stderr}");
}

/// A loopback address nothing is bound to (bound once, then released).
fn free_addr() -> std::net::SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind an ephemeral port");
    listener.local_addr().expect("local addr")
}

#[tokio::test]
async fn the_daemon_log_outlives_readiness_and_is_removed_by_the_rollback() {
    use tumult_net::actions::{start_proxy, stop_proxy};

    // Held to the end of the test: the rollback locates the daemon too.
    let mut env = tumult_test_utils::EnvGuard::lock();
    env.set("TUMULT_NET_PROXYD", env!("CARGO_BIN_EXE_tumult-net-proxyd"));
    let listen = free_addr();
    let key: String = listen
        .to_string()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    // The user's runtime directory when there is one, the temp directory
    // otherwise.
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .filter(|runtime_dir| runtime_dir.is_dir())
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join(format!("tumult-net-{key}.log.d"));
    let log = dir.join("stderr.log");

    start_proxy(listen, free_addr(), tumult_net::FaultProfile::default())
        .await
        .expect("the proxy starts");
    // Looked at before the proxy is stopped, asserted after: a failed
    // assertion must not leave the daemon running.
    let kept = log.is_file();
    let modes = (mode_of(&dir), mode_of(&log));
    let stopped = stop_proxy(listen).await;

    assert!(
        kept,
        "a running daemon keeps its log file: {}",
        log.display()
    );
    #[cfg(unix)]
    assert_eq!(
        modes,
        (Some(0o700), Some(0o600)),
        "the log is its owner's alone"
    );
    stopped.expect("the proxy stops");
    assert!(!dir.exists(), "the rollback removes the daemon's log");
}

/// The permission bits of `path`; `None` when it does not exist or the
/// platform has none.
fn mode_of(path: &std::path::Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        let metadata = std::fs::symlink_metadata(path).ok()?;
        Some(metadata.permissions().mode() & 0o777)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}
