//! The `tumult` binary's logging contract: command output on stdout,
//! structured diagnostics on stderr.

use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

/// Runs `tumult` with `args` in a clean environment and a private store.
fn tumult(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tumult"))
        .args(args)
        .current_dir(dir)
        .env("HOME", dir)
        .env("TUMULT_LAKE_PATH", dir.join("lake.duckdb"))
        .env_remove("RUST_LOG")
        .env_remove("TUMULT_LOG_FORMAT")
        .env_remove("TUMULT_ENVIRONMENT")
        .env_remove("TUMULT_DAEMON_URL")
        .env_remove("OTEL_SERVICE_NAME")
        .env_remove("OTEL_RESOURCE_ATTRIBUTES")
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
        .output()
        .expect("run tumult")
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
fn a_failed_command_ends_with_a_structured_fatal_event() {
    let dir = tempfile::tempdir().unwrap();
    let output = tumult(dir.path(), &["validate", "missing.toon"]);

    assert_eq!(output.status.code(), Some(1));
    let events = json_lines(&output.stderr);
    let fatal = events.last().expect("at least one event");
    assert_eq!(fatal["event.name"], "process.fatal");
    assert_eq!(fatal["level"], "ERROR");
    assert_eq!(fatal["service.name"], "tumult");
    let message = fatal["error.message"].as_str().unwrap();
    assert!(message.contains("missing.toon"), "{message}");
    for event in &events {
        for key in ["timestamp", "level", "target", "message", "service.name"] {
            assert!(event.get(key).is_some(), "missing {key}: {event}");
        }
    }
}

#[test]
fn a_skipped_journal_is_a_structured_warning_and_the_command_still_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let journals = dir.path().join("journals");
    std::fs::create_dir(&journals).unwrap();
    std::fs::write(journals.join("corrupt.toon"), "{{{{ not a journal").unwrap();

    let output = tumult(dir.path(), &["trend", "journals"]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let events = json_lines(&output.stderr);
    let skipped: Vec<_> = events
        .iter()
        .filter(|event| event["event.name"] == "cli.journal.skipped")
        .collect();
    assert_eq!(skipped.len(), 1, "{events:?}");
    assert_eq!(skipped[0]["level"], "WARN");
    assert!(
        skipped[0]["file.path"]
            .as_str()
            .is_some_and(|path| path.ends_with("corrupt.toon")),
        "{}",
        skipped[0]
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("cli.journal.skipped"), "{stdout}");
}
