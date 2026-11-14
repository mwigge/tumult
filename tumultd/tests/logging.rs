//! The daemon binary's logging contract: structured events on stderr,
//! nothing but command output on stdout.

use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

/// Runs `tumultd import <missing file>` against a private store: a cheap
/// subcommand that logs nothing on its way to a fatal error.
fn failing_import(dir: &Path, format: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tumultd"));
    command
        .arg("import")
        .arg(dir.join("missing.csv"))
        .env("TUMULT_LAKE_PATH", dir.join("lake.duckdb"))
        .env_remove("RUST_LOG")
        .env_remove("TUMULT_LOG_FORMAT")
        .env_remove("TUMULT_ENVIRONMENT")
        .env_remove("OTEL_SERVICE_NAME")
        .env_remove("OTEL_RESOURCE_ATTRIBUTES")
        .env_remove("KRONIKA_DB");
    if let Some(format) = format {
        command.env("TUMULT_LOG_FORMAT", format);
    }
    command.output().expect("run tumultd")
}

fn json_lines(stderr: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(stderr)
        .lines()
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("stderr line is not JSON ({e}): {line}"))
        })
        .collect()
}

#[test]
fn fatal_error_is_a_structured_event_on_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let output = failing_import(dir.path(), None);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        output.stdout.is_empty(),
        "stdout carries only command output: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let events = json_lines(&output.stderr);
    let fatal = events.last().expect("at least one event");
    assert_eq!(fatal["event.name"], "process.fatal");
    assert_eq!(fatal["level"], "ERROR");
    assert_eq!(fatal["service.name"], "tumultd");
    let message = fatal["error.message"].as_str().unwrap();
    assert!(message.starts_with("import "), "{message}");
    assert!(message.contains("missing.csv"), "{message}");
    for event in &events {
        for key in ["timestamp", "level", "target", "message", "service.name"] {
            assert!(event.get(key).is_some(), "missing {key}: {event}");
        }
    }
}

#[test]
fn text_format_is_an_explicit_opt_in() {
    let dir = tempfile::tempdir().unwrap();
    let output = failing_import(dir.path(), Some("text"));

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let last = stderr.lines().last().expect("at least one line");
    assert!(serde_json::from_str::<Value>(last).is_err(), "{last}");
    assert!(
        last.contains("exiting because of an unrecoverable error"),
        "{last}"
    );
    assert!(last.contains("missing.csv"), "{last}");
}

#[test]
fn service_name_follows_the_otel_convention() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tumultd"))
        .arg("import")
        .arg(dir.path().join("missing.csv"))
        .env("TUMULT_LAKE_PATH", dir.path().join("lake.duckdb"))
        .env("OTEL_SERVICE_NAME", "tumultd-eu")
        .env_remove("RUST_LOG")
        .env_remove("TUMULT_LOG_FORMAT")
        .env_remove("KRONIKA_DB")
        .output()
        .expect("run tumultd");

    let events = json_lines(&output.stderr);
    assert_eq!(events.last().unwrap()["service.name"], "tumultd-eu");
}

/// A `tumultd serve` child that is killed if the test leaves it running.
#[cfg(unix)]
struct Daemon(Option<std::process::Child>);

#[cfg(unix)]
impl Daemon {
    /// Starts the daemon on a private store and free loopback ports and
    /// waits until it answers `/healthz`.
    fn start(dir: &Path) -> Self {
        let grpc_port = free_port();
        let http_port = free_port();
        let child = Command::new(env!("CARGO_BIN_EXE_tumultd"))
            .arg("serve")
            .env("HOME", dir)
            .env("TUMULT_LAKE_PATH", dir.join("lake.duckdb"))
            .env("KRONIKA_OTLP_GRPC_ADDR", format!("127.0.0.1:{grpc_port}"))
            .env("KRONIKA_OTLP_HTTP_ADDR", format!("127.0.0.1:{http_port}"))
            .env(
                "KRONIKA_METRICS_DIR",
                concat!(env!("CARGO_MANIFEST_DIR"), "/../metrics"),
            )
            .env_remove("RUST_LOG")
            .env_remove("TUMULT_LOG_FORMAT")
            .env_remove("OTEL_SERVICE_NAME")
            .env_remove("KRONIKA_DB")
            .env_remove("KRONIKA_INGEST_TOKEN")
            .env_remove("KRONIKA_TLS_CERT")
            .env_remove("KRONIKA_TLS_KEY")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn tumultd");
        // Owned before waiting, so a daemon that never answers is killed.
        let mut daemon = Self(Some(child));
        daemon.wait_until_healthy(http_port);
        daemon
    }

    /// Waits for `/healthz`. A daemon that has exited (the port it was
    /// given was taken in the meantime, say) fails the test at once with
    /// what it wrote, rather than after the whole wait.
    fn wait_until_healthy(&mut self, http_port: u16) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        while !is_healthy(http_port) {
            self.assert_running();
            assert!(
                std::time::Instant::now() < deadline,
                "tumultd did not answer /healthz"
            );
            // sleep: the interval of the bounded poll for /healthz
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    fn assert_running(&mut self) {
        let child = self.0.as_mut().expect("daemon is running");
        let Some(status) = child.try_wait().expect("poll tumultd") else {
            return;
        };
        let output = self
            .0
            .take()
            .expect("daemon is running")
            .wait_with_output()
            .expect("collect what tumultd wrote");
        panic!(
            "tumultd exited before it was healthy ({status}): {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Sends SIGTERM and returns everything the daemon wrote.
    fn terminate(mut self) -> Output {
        let child = self.0.take().expect("daemon is running");
        let status = Command::new("kill")
            .arg(child.id().to_string())
            .status()
            .expect("run kill");
        assert!(status.success(), "kill failed");
        child.wait_with_output().expect("wait for tumultd")
    }
}

#[cfg(unix)]
impl Drop for Daemon {
    fn drop(&mut self) {
        let Some(child) = self.0.as_mut() else {
            return;
        };
        // Best effort: the test is already failing when this runs.
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// A loopback port that was free a moment ago. The daemon derives the
/// endpoint it exports its own telemetry to from the configured port before
/// it binds, and has no way to report a port the OS chose, so it can be
/// handed neither port 0 nor an already bound listener.
#[cfg(unix)]
fn free_port() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .and_then(|listener| listener.local_addr())
        .expect("bind a free loopback port")
        .port()
}

#[cfg(unix)]
fn is_healthy(http_port: u16) -> bool {
    use std::io::{Read, Write};

    let Ok(mut stream) = std::net::TcpStream::connect(("127.0.0.1", http_port)) else {
        return false;
    };
    let request = b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    if stream.write_all(request).is_err() {
        return false;
    }
    let mut response = String::new();
    // A reset connection reads as "not healthy yet".
    let _ = stream.read_to_string(&mut response);
    response.starts_with("HTTP/1.1 200")
}

/// The daemon exports its own log events to its own OTLP listener. A clean
/// stop must flush them while that listener still accepts, not after it is
/// gone: a late flush loses the events and logs an export error on every
/// shutdown.
#[cfg(unix)]
#[test]
fn clean_shutdown_reports_no_failed_telemetry_export() {
    let dir = tempfile::tempdir().unwrap();
    let output = Daemon::start(dir.path()).terminate();

    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty());
    let events = json_lines(&output.stderr);
    let unexpected: Vec<&Value> = events
        .iter()
        .filter(|event| {
            event["level"] == "ERROR"
                || event["target"]
                    .as_str()
                    .is_some_and(|target| target.starts_with("opentelemetry"))
        })
        .collect();
    assert!(unexpected.is_empty(), "{unexpected:#?}");
    assert_eq!(events.last().unwrap()["event.name"], "daemon.stopped");
    // No export error is only half of it: the events must have arrived.
    let ingested = ingested_log_bodies(&dir.path().join("lake.duckdb"));
    assert!(
        ingested
            .iter()
            .any(|body| body == "shutdown signal received"),
        "the daemon's last exported event is in its own store: {ingested:?}"
    );
}

/// The bodies of the log events the daemon ingested from itself, read from
/// its store once it has exited and released it.
#[cfg(unix)]
fn ingested_log_bodies(store: &Path) -> Vec<String> {
    let rows = tumult_lake::Store::open(store)
        .expect("open the stopped daemon's store")
        .read_only()
        .expect("read the store")
        .query_json_rows("SELECT body FROM logs WHERE service_name = 'tumultd'")
        .expect("query the ingested logs");
    rows.iter()
        .filter_map(|row| row["body"].as_str().map(str::to_owned))
        .collect()
}
