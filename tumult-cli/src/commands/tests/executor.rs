//! Tests for the `ProviderExecutor`.

use super::super::*;
use tumult_core::runner::ActivityExecutor;
use tumult_core::types::{Activity, ActivityType};

// ── ProviderExecutor tests ────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn process_executor_runs_echo() {
    let activity = Activity {
        name: "echo-test".into(),
        activity_type: ActivityType::Action,
        provider: Provider::Process {
            path: "echo".into(),
            arguments: vec!["hello world".into()],
            env: std::collections::HashMap::new(),
            timeout_s: Some(5.0),
        },
        tolerance: None,
        pause_before_s: None,
        pause_after_s: None,
        background: false,
        label_selector: None,
    };

    let executor = ProviderExecutor::new();
    let outcome = executor.execute(&activity);

    assert!(outcome.success);
    assert_eq!(outcome.output.as_deref(), Some("hello world"));
}

#[tokio::test(flavor = "multi_thread")]
async fn process_executor_captures_failure() {
    let activity = Activity {
        name: "false-test".into(),
        activity_type: ActivityType::Action,
        provider: Provider::Process {
            path: "false".into(),
            arguments: vec![],
            env: std::collections::HashMap::new(),
            timeout_s: Some(5.0),
        },
        tolerance: None,
        pause_before_s: None,
        pause_after_s: None,
        background: false,
        label_selector: None,
    };

    let executor = ProviderExecutor::new();
    let outcome = executor.execute(&activity);

    assert!(!outcome.success);
}

#[tokio::test(flavor = "multi_thread")]
async fn process_executor_nonexistent_returns_error() {
    let activity = Activity {
        name: "bad-cmd".into(),
        activity_type: ActivityType::Action,
        provider: Provider::Process {
            path: "/nonexistent/binary".into(),
            arguments: vec![],
            env: std::collections::HashMap::new(),
            timeout_s: None,
        },
        tolerance: None,
        pause_before_s: None,
        pause_after_s: None,
        background: false,
        label_selector: None,
    };

    let executor = ProviderExecutor::new();
    let outcome = executor.execute(&activity);

    assert!(!outcome.success);
    assert!(outcome.error.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn native_provider_rejects_unknown_plugin() {
    let activity = Activity {
        name: "native-test".into(),
        activity_type: ActivityType::Action,
        provider: Provider::Native {
            plugin: "unknown-plugin".into(),
            function: "test-fn".into(),
            arguments: std::collections::HashMap::new(),
        },
        tolerance: None,
        pause_before_s: None,
        pause_after_s: None,
        background: false,
        label_selector: None,
    };

    let executor = ProviderExecutor::new();
    let outcome = executor.execute(&activity);

    assert!(!outcome.success);
    let error = outcome.error.as_ref().unwrap();
    assert!(error.contains("unknown native plugin"));
    // The typed error lists the registered plugins for discoverability.
    assert!(error.contains("tumult-kubernetes"));
    assert!(error.contains("tumult-net"));
    assert!(error.contains("tumult-ssh"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn native_provider_rejects_unknown_function() {
    let activity = Activity {
        name: "native-test".into(),
        activity_type: ActivityType::Action,
        provider: Provider::Native {
            plugin: "tumult-ssh".into(),
            function: "command-exeucte".into(), // typo must not silently run anything
            arguments: std::collections::HashMap::new(),
        },
        tolerance: None,
        pause_before_s: None,
        pause_after_s: None,
        background: false,
        label_selector: None,
    };

    let executor = ProviderExecutor::new();
    let outcome = executor.execute(&activity);

    assert!(!outcome.success);
    let error = outcome.error.as_ref().unwrap();
    assert!(error.contains("unknown tumult-ssh function"));
    assert!(error.contains("execute"), "should list available functions");
}

// ── Pipe draining / timeout-kill tests ──────────────────────

fn process_activity(path: &str, arguments: Vec<String>, timeout_s: Option<f64>) -> Activity {
    Activity {
        name: "drain-test".into(),
        activity_type: ActivityType::Action,
        provider: Provider::Process {
            path: path.into(),
            arguments,
            env: std::collections::HashMap::new(),
            timeout_s,
        },
        tolerance: None,
        pause_before_s: None,
        pause_after_s: None,
        background: false,
        label_selector: None,
    }
}

/// A child emitting far more than the ~64 KiB OS pipe buffer must not be
/// killed as a false timeout: the executor drains the pipes while waiting.
#[tokio::test(flavor = "multi_thread")]
async fn async_executor_drains_large_output_without_false_timeout() {
    // ~1.4 MB of stdout — many times the pipe buffer.
    let activity = process_activity("seq", vec!["1".into(), "200000".into()], Some(10.0));

    let outcome = ProviderExecutor::new().execute(&activity);

    assert!(
        outcome.success,
        "large output must not time out: {outcome:?}"
    );
    let output = outcome.output.expect("output must be captured");
    assert!(
        output.ends_with("200000"),
        "output should be drained to completion"
    );
}

/// Same as above, but through the synchronous fallback path (no runtime).
#[test]
fn sync_executor_drains_large_output_without_false_timeout() {
    let activity = process_activity("seq", vec!["1".into(), "200000".into()], Some(10.0));

    let outcome = ProviderExecutor::new().execute(&activity);

    assert!(
        outcome.success,
        "large output must not time out: {outcome:?}"
    );
    let output = outcome.output.expect("output must be captured");
    assert!(output.ends_with("200000"));
}

/// Output beyond the 8 MiB capture cap is truncated with a note rather than
/// captured in full, but the child still runs to completion.
#[test]
fn sync_executor_truncates_output_beyond_cap() {
    // ~12 MB of stdout — beyond the 8 MiB capture cap.
    let activity = process_activity("seq", vec!["1".into(), "1500000".into()], Some(30.0));

    let outcome = ProviderExecutor::new().execute(&activity);

    assert!(outcome.success);
    let output = outcome.output.expect("output must be captured");
    assert!(
        output.ends_with("[output truncated at 8 MiB]"),
        "truncated output must carry the truncation note"
    );
}

/// On timeout the executor kills the child's whole process group, so a
/// grandchild spawned by a wrapper script cannot outlive the timeout.
#[cfg(unix)]
#[test]
fn sync_executor_timeout_kills_process_group() {
    let marker_dir = tempfile::tempdir().expect("tempdir");
    let marker = marker_dir.path().join("grandchild-ran");
    // The grandchild creates the marker file 2s in; the wrapper is killed at
    // 0.5s. If only the direct child were killed, the grandchild would
    // survive and create the marker.
    let script = format!("(sleep 2; touch {}) & wait", marker.display());
    let activity = process_activity("sh", vec!["-c".into(), script], Some(0.5));

    let outcome = ProviderExecutor::new().execute(&activity);

    assert!(!outcome.success, "timed-out process must fail");
    let error = outcome.error.expect("timeout must be reported");
    assert!(error.contains("timed out"), "unexpected error: {error}");

    // Give the grandchild ample time to (wrongly) create the marker.
    std::thread::sleep(std::time::Duration::from_secs(3));
    assert!(
        !marker.exists(),
        "grandchild survived the timeout — process group was not killed"
    );
}
