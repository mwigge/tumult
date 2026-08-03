//! Behavior tests for the `process` provider dispatch: async execution inside
//! a Tokio runtime, the synchronous fallback used on runtime-less threads,
//! timeout kills, and spawn failures. Everything runs through the public
//! [`ProviderExecutor`] API with `/bin/sh`, so these tests are Unix-only.
#![cfg(unix)]

use std::collections::HashMap;

use tumult_core::runner::ActivityExecutor;
use tumult_core::types::{Activity, ActivityType, Provider};
use tumult_exec::ProviderExecutor;

fn process_activity(path: &str, arguments: &[&str], timeout_s: Option<f64>) -> Activity {
    Activity {
        name: "test-activity".into(),
        activity_type: ActivityType::Action,
        provider: Provider::Process {
            path: path.into(),
            arguments: arguments.iter().map(|a| (*a).to_string()).collect(),
            env: HashMap::new(),
            timeout_s,
        },
        tolerance: None,
        pause_before_s: None,
        pause_after_s: None,
        background: false,
        label_selector: None,
    }
}

fn sh(command: &str, timeout_s: Option<f64>) -> Activity {
    process_activity("sh", &["-c", command], timeout_s)
}

// ── Async path (a Tokio runtime is current) ─────────────────

#[tokio::test(flavor = "multi_thread")]
async fn nonexistent_binary_reports_failed_to_execute() {
    let executor = ProviderExecutor::new();
    let outcome = executor.execute(&process_activity(
        "/nonexistent/tumult-cov-no-such-binary",
        &[],
        Some(5.0),
    ));
    assert!(!outcome.success);
    let error = outcome.error.expect("spawn failure must carry an error");
    assert!(
        error.contains("failed to execute"),
        "unexpected error: {error}"
    );
    assert!(
        error.contains("tumult-cov-no-such-binary"),
        "error must name the binary: {error}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn timed_out_process_reports_timeout_quickly() {
    let executor = ProviderExecutor::new();
    let start = std::time::Instant::now();
    let outcome = executor.execute(&sh("sleep 30", Some(0.2)));
    assert!(!outcome.success);
    let error = outcome.error.expect("timeout must carry an error");
    assert!(error.contains("timed out"), "unexpected error: {error}");
    assert!(
        start.elapsed() < std::time::Duration::from_secs(10),
        "the executor must not wait out the child's full sleep"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn timed_out_process_group_is_actually_gone() {
    // The child records its own pid (== its process group id, since children
    // are spawned with process_group(0)) before sleeping past the timeout.
    let dir = std::env::temp_dir().join(format!("tumult-exec-kill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let pid_file = dir.join("child.pid");

    let executor = ProviderExecutor::new();
    let command = format!("echo $$ > '{}'; exec sleep 30", pid_file.display());
    let outcome = executor.execute(&sh(&command, Some(1.0)));
    assert!(outcome
        .error
        .as_deref()
        .is_some_and(|e| e.contains("timed out")));

    // The child writes its pid before sleeping; under load the write can
    // still be in flight when the timeout kill lands, so retry briefly.
    let mut pid = String::new();
    for _ in 0..30 {
        if let Ok(content) = std::fs::read_to_string(&pid_file) {
            if !content.trim().is_empty() {
                pid = content.trim().to_string();
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(!pid.is_empty(), "child never recorded its pid");

    // ESRCH: signalling the group fails because the whole group was killed.
    let probe = std::process::Command::new("sh")
        .args(["-c", &format!("kill -0 -{pid}")])
        .status()
        .unwrap();
    assert!(
        !probe.success(),
        "process group {pid} survived the timeout kill"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn stderr_is_captured_on_non_zero_exit() {
    let executor = ProviderExecutor::new();
    let outcome = executor.execute(&sh("echo out-line; echo err-line >&2; exit 3", Some(5.0)));
    assert!(!outcome.success);
    assert_eq!(outcome.output.as_deref(), Some("out-line"));
    let error = outcome.error.expect("stderr must be surfaced");
    assert!(error.contains("err-line"), "unexpected error: {error}");
}

#[tokio::test(flavor = "multi_thread")]
async fn non_zero_exit_without_stderr_has_no_error_text() {
    let executor = ProviderExecutor::new();
    let outcome = executor.execute(&sh("exit 4", Some(5.0)));
    assert!(!outcome.success);
    assert_eq!(outcome.output, None);
    assert_eq!(outcome.error, None);
}

// ── Sync fallback (no Tokio runtime on the calling thread) ──

/// The runner executes background activities on `std::thread::scope` threads,
/// which never carry a Tokio runtime; the executor must fall back to
/// `std::process::Command` there. Each scenario runs on its own plain thread
/// to prove no runtime is involved.
#[test]
fn sync_fallback_handles_success_failure_timeout_and_spawn_errors() {
    let run = |activity: Activity| {
        std::thread::spawn(move || {
            assert!(
                tokio::runtime::Handle::try_current().is_err(),
                "this scenario must run without a Tokio runtime"
            );
            ProviderExecutor::new().execute(&activity)
        })
        .join()
        .unwrap()
    };

    let outcome = run(sh("echo sync-hello", Some(5.0)));
    assert!(outcome.success, "{:?}", outcome.error);
    assert_eq!(outcome.output.as_deref(), Some("sync-hello"));

    let outcome = run(sh("echo sync-err >&2; exit 3", Some(5.0)));
    assert!(!outcome.success);
    let error = outcome.error.expect("stderr must be surfaced");
    assert!(error.contains("sync-err"), "unexpected error: {error}");

    let outcome = run(sh("exit 4", Some(5.0)));
    assert!(!outcome.success);
    let error = outcome.error.expect("exit status must be reported");
    assert!(
        error.contains("exited with exit status: 4"),
        "unexpected error: {error}"
    );

    let start = std::time::Instant::now();
    let outcome = run(sh("sleep 30", Some(0.2)));
    assert!(!outcome.success);
    let error = outcome.error.expect("timeout must carry an error");
    assert!(error.contains("timed out"), "unexpected error: {error}");
    assert!(
        start.elapsed() < std::time::Duration::from_secs(10),
        "the sync fallback must not wait out the child's full sleep"
    );

    let outcome = run(process_activity(
        "/nonexistent/tumult-cov-no-such-binary",
        &[],
        Some(5.0),
    ));
    assert!(!outcome.success);
    let error = outcome.error.expect("spawn failure must carry an error");
    assert!(
        error.contains("failed to execute"),
        "unexpected error: {error}"
    );
}

// ── Native dispatch through the executor ────────────────────

#[test]
fn unknown_native_plugin_is_a_failed_outcome_not_a_panic() {
    let executor = ProviderExecutor::default();
    let activity = Activity {
        name: "native-test".into(),
        activity_type: ActivityType::Action,
        provider: Provider::Native {
            plugin: "no-such-native-plugin".into(),
            function: "no-such-function".into(),
            arguments: HashMap::new(),
        },
        tolerance: None,
        pause_before_s: None,
        pause_after_s: None,
        background: false,
        label_selector: None,
    };
    let outcome = executor.execute(&activity);
    assert!(!outcome.success);
    let error = outcome.error.expect("unknown plugin must carry an error");
    assert!(
        error.contains("unknown native plugin"),
        "unexpected error: {error}"
    );
}
