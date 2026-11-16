//! Process-level logging behaviour: what reaches stderr when a process
//! panics or dies of an error, whatever the log filter says.
//!
//! The panic hook and the global subscriber are per process, so every case
//! re-runs this test binary as a child restricted to [`child_entry`], which
//! installs the real subscriber and then fails the way `CHILD_MODE_ENV`
//! says.

use std::process::{Command, Output, Stdio};

use serde_json::Value;
use tumult_otel::logging::{self, LogFormat, LogOptions};

const CHILD_MODE_ENV: &str = "TUMULT_LOGGING_TEST_CHILD";
const SERVICE: &str = "logging-child";
/// What `libtest` exits with when a test panicked.
const TEST_FAILED: i32 = 101;

// Not a test of its own: the body of the child processes spawned below.
#[test]
#[ignore = "entry point of the child processes spawned by the other tests"]
fn child_entry() {
    let Ok(mode) = std::env::var(CHILD_MODE_ENV) else {
        return;
    };
    logging::init(LogOptions::new(SERVICE).with_format(LogFormat::Json)).unwrap();
    match mode.as_str() {
        "panic" => panic!("the child gave up"),
        "fatal" => logging::log_fatal("the store is locked"),
        other => unreachable!("unknown child mode {other}"),
    }
}

/// This binary, restricted to [`child_entry`] in `mode`, with a clean
/// logging environment.
fn child(mode: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["child_entry", "--exact", "--ignored", "--nocapture"])
        .env(CHILD_MODE_ENV, mode)
        .env_remove("RUST_LOG")
        .env_remove("RUST_BACKTRACE")
        .env_remove("TUMULT_LOG_FORMAT")
        .env_remove("TUMULT_ENVIRONMENT")
        .env_remove("OTEL_RESOURCE_ATTRIBUTES")
        .stdout(Stdio::null());
    command
}

fn stderr_lines(output: &Output) -> Vec<String> {
    String::from_utf8_lossy(&output.stderr)
        .lines()
        .map(str::to_string)
        .collect()
}

/// The single JSON line on the child's stderr.
fn only_line(output: &Output) -> Value {
    let lines = stderr_lines(output);
    assert_eq!(lines.len(), 1, "{lines:#?}");
    serde_json::from_str(&lines[0]).unwrap_or_else(|e| panic!("not JSON ({e}): {}", lines[0]))
}

fn assert_is_the_panic(event: &Value) {
    assert_eq!(event["event.name"], "process.panic", "{event}");
    assert_eq!(event["level"], "ERROR");
    assert_eq!(event["service.name"], SERVICE);
    assert_eq!(event["exception.type"], "panic");
    assert_eq!(event["exception.message"], "the child gave up");
    let file = event["code.file.path"].as_str().unwrap();
    assert!(file.ends_with("logging_process.rs"), "{file}");
    assert!(event["code.line.number"].is_u64(), "{event}");
}

fn assert_is_the_fatal(event: &Value) {
    assert_eq!(event["event.name"], "process.fatal", "{event}");
    assert_eq!(event["level"], "ERROR");
    assert_eq!(event["service.name"], SERVICE);
    assert_eq!(event["error.message"], "the store is locked");
}

#[test]
fn a_panic_is_one_structured_line() {
    let output = child("panic").output().unwrap();

    assert_eq!(output.status.code(), Some(TEST_FAILED));
    assert_is_the_panic(&only_line(&output));
}

#[test]
fn a_panic_is_reported_when_logging_is_off() {
    let output = child("panic").env("RUST_LOG", "off").output().unwrap();

    assert_eq!(output.status.code(), Some(TEST_FAILED));
    assert_is_the_panic(&only_line(&output));
}

#[test]
fn a_panic_is_reported_when_the_filter_names_another_crate() {
    let output = child("panic")
        .env("RUST_LOG", "tumult_core=debug")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(TEST_FAILED));
    assert_is_the_panic(&only_line(&output));
}

#[test]
fn a_requested_backtrace_follows_the_structured_line() {
    let output = child("panic").env("RUST_BACKTRACE", "1").output().unwrap();

    let lines = stderr_lines(&output);
    let first = lines.first().expect("at least the panic event");
    assert_is_the_panic(&serde_json::from_str(first).unwrap());
    let rest = lines[1..].join("\n");
    assert!(rest.contains("panicked at"), "{rest}");
    assert!(rest.contains("stack backtrace"), "{rest}");
}

#[test]
fn a_panic_with_stderr_closed_still_ends_as_a_panic_not_an_abort() {
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);

    let status = child("panic").stderr(writer).status().unwrap();

    assert_eq!(status.code(), Some(TEST_FAILED), "{status:?}");
}

#[test]
fn a_fatal_error_is_one_structured_line() {
    let output = child("fatal").output().unwrap();

    assert_eq!(output.status.code(), Some(0));
    assert_is_the_fatal(&only_line(&output));
}

#[test]
fn a_fatal_error_is_reported_when_logging_is_off() {
    let output = child("fatal").env("RUST_LOG", "off").output().unwrap();

    assert_eq!(output.status.code(), Some(0));
    assert_is_the_fatal(&only_line(&output));
}

#[test]
fn a_fatal_error_is_reported_when_the_filter_names_another_crate() {
    let output = child("fatal")
        .env("RUST_LOG", "tumult_core=debug")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    assert_is_the_fatal(&only_line(&output));
}
