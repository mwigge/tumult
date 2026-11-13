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
