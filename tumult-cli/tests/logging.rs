//! The `tumult` binary's logging contract: command output on stdout,
//! structured diagnostics on stderr.

use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

/// `tumult` with `args`, a clean environment and a private store.
fn command(dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tumult"));
    command
        .args(args)
        .current_dir(dir)
        .env("HOME", dir)
        .env("TUMULT_LAKE_PATH", dir.join("lake.duckdb"))
        .env_remove("RUST_LOG")
        .env_remove("RUST_BACKTRACE")
        .env_remove("TUMULT_LOG_FORMAT")
        .env_remove("TUMULT_ENVIRONMENT")
        .env_remove("TUMULT_DAEMON_URL")
        .env_remove("OTEL_SERVICE_NAME")
        .env_remove("OTEL_RESOURCE_ATTRIBUTES")
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT");
    command
}

/// Runs `tumult` with `args` in a clean environment and a private store.
fn tumult(dir: &Path, args: &[&str]) -> Output {
    command(dir, args).output().expect("run tumult")
}

/// An experiment whose only lasting effect is its rollback creating
/// `marker`.
fn experiment_with_rollback(marker: &Path) -> String {
    format!(
        r#"title: Rollback leaves a marker
description: The method does nothing; the rollback creates a file

method[1]:
  - name: no-op
    activity_type: action
    provider:
      type: process
      path: sh
      arguments[2]: "-c", "echo done"
      timeout_s: 5.0

rollbacks[1]:
  - name: leave-marker
    activity_type: action
    provider:
      type: process
      path: touch
      arguments[1]: "{}"
      timeout_s: 5.0
"#,
        marker.display()
    )
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

/// A reader that goes away (`tumult run ... 2>&1 | head`) must not take the
/// experiment down with it: the rollback still has to run.
#[test]
fn a_closed_stderr_does_not_abort_the_run_before_its_rollback() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("rolled-back");
    std::fs::write(
        dir.path().join("experiment.toon"),
        experiment_with_rollback(&marker),
    )
    .unwrap();
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);

    let status = command(
        dir.path(),
        &[
            "run",
            "experiment.toon",
            "--no-ingest",
            "--rollback-strategy",
            "always",
        ],
    )
    .env("RUST_LOG", "debug")
    .stdout(Stdio::null())
    .stderr(writer)
    .status()
    .expect("run tumult");

    assert_eq!(status.code(), Some(0), "{status:?}");
    assert!(marker.exists(), "the rollback did not run");
}

/// A reader of the command's output that goes away (`tumult run ... | head
/// -1`) must not end the run: the journal is still written and ingested.
#[test]
fn a_closed_stdout_does_not_end_the_run_before_its_journal_is_ingested() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("experiment.toon"),
        experiment_with_rollback(&dir.path().join("rolled-back")),
    )
    .unwrap();
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);

    let output = command(dir.path(), &["run", "experiment.toon"])
        .stdout(writer)
        .output()
        .expect("run tumult");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert!(!stderr.contains("process.panic"), "{stderr}");
    assert!(dir.path().join("journal.toon").is_file(), "no journal");
    let stats = tumult(dir.path(), &["store", "stats"]);
    let stats = String::from_utf8_lossy(&stats.stdout);
    assert!(stats.contains("Experiments: 1"), "not ingested: {stats}");
}

/// The same for the findings `validate` addresses to its reader on stderr.
#[test]
fn a_closed_stderr_does_not_end_a_validation_that_has_findings() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("experiment.toon"),
        r"title: Uses a plugin nobody has
description: The finding goes to stderr

method[1]:
  - name: unknown
    activity_type: action
    provider:
      type: native
      plugin: no-such-plugin
      function: nothing
",
    )
    .unwrap();
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);

    let output = command(dir.path(), &["validate", "experiment.toon"])
        .stderr(writer)
        .output()
        .expect("run tumult");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("Method steps: 1"), "{stdout}");
}

/// What `tumult` printed on stdout for `args`, after it succeeded.
fn stdout_of(dir: &Path, args: &[&str]) -> String {
    let output = tumult(dir, args);
    assert_eq!(
        output.status.code(),
        Some(0),
        "tumult {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("utf-8 output")
}

#[test]
fn agents_prints_one_row_per_adapter_on_stdout() {
    let dir = tempfile::tempdir().unwrap();

    let table = stdout_of(dir.path(), &["agents"]);

    let mut lines = table.lines();
    let header = lines.next().expect("a header line");
    assert!(header.starts_with("ADAPTER"), "{table}");
    assert!(header.ends_with("DETAIL"), "{table}");
    let adapters: Vec<&str> = lines
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    assert!(adapters.contains(&"claude-code"), "{table}");
    assert!(adapters.contains(&"codex"), "{table}");
}

#[test]
fn compliance_reports_on_the_journal_of_a_run_and_lists_its_sources() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("experiment.toon"),
        experiment_with_rollback(&dir.path().join("rolled-back")),
    )
    .unwrap();
    stdout_of(dir.path(), &["run", "experiment.toon", "--no-ingest"]);

    let report = stdout_of(
        dir.path(),
        &["compliance", "journal.toon", "--framework", "soc2"],
    );
    assert!(report.contains("Journals analyzed: 1\n"), "{report}");
    assert!(report.contains("  completed: 1 run(s)\n"), "{report}");
    assert!(report.ends_with("=== End Report ===\n"), "{report}");

    // --sources lists the citation registry and needs no journals.
    let sources = stdout_of(
        dir.path(),
        &["compliance", "--framework", "dora", "--sources"],
    );
    assert!(sources.contains(" — citation registry ===\n"), "{sources}");
    assert!(sources.ends_with("=== End registry ===\n"), "{sources}");
    assert!(!sources.contains("Journals analyzed"), "{sources}");
}

#[test]
fn recommend_prints_its_json_document_on_stdout() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("experiment.toon"),
        experiment_with_rollback(&dir.path().join("rolled-back")),
    )
    .unwrap();
    // The run is ingested into the private store the recommendation reads.
    stdout_of(dir.path(), &["run", "experiment.toon"]);

    let document = stdout_of(
        dir.path(),
        &[
            "recommend",
            "--goal",
            "improve postgres resilience",
            "--store-path",
            "lake.duckdb",
            "--format",
            "json",
        ],
    );

    let value: Value = serde_json::from_str(&document)
        .unwrap_or_else(|e| panic!("stdout is not one JSON document ({e}): {document}"));
    assert!(value.is_object(), "{document}");
    assert!(value.get("source").is_some(), "{document}");
}

/// The one line a failing run must leave on stderr, whatever the filter.
fn only_fatal_line(rust_log: &str) -> Value {
    let dir = tempfile::tempdir().unwrap();
    let output = command(dir.path(), &["validate", "missing.toon"])
        .env("RUST_LOG", rust_log)
        .output()
        .expect("run tumult");

    assert_eq!(output.status.code(), Some(1));
    let mut events = json_lines(&output.stderr);
    assert_eq!(events.len(), 1, "RUST_LOG={rust_log}: {events:?}");
    events.remove(0)
}

#[test]
fn the_fatal_error_is_reported_even_when_logging_is_off() {
    let fatal = only_fatal_line("off");

    assert_eq!(fatal["event.name"], "process.fatal");
    assert_eq!(fatal["level"], "ERROR");
    assert_eq!(fatal["service.name"], "tumult");
    let message = fatal["error.message"].as_str().unwrap();
    assert!(message.contains("missing.toon"), "{message}");
}

#[test]
fn the_fatal_error_is_reported_when_the_filter_names_another_crate() {
    let fatal = only_fatal_line("tumult_core=debug");

    assert_eq!(fatal["event.name"], "process.fatal");
    let message = fatal["error.message"].as_str().unwrap();
    assert!(message.contains("missing.toon"), "{message}");
}
