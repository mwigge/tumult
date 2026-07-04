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

    let executor = ProviderExecutor;
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

    let executor = ProviderExecutor;
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

    let executor = ProviderExecutor;
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

    let executor = ProviderExecutor;
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

    let executor = ProviderExecutor;
    let outcome = executor.execute(&activity);

    assert!(!outcome.success);
    let error = outcome.error.as_ref().unwrap();
    assert!(error.contains("unknown tumult-ssh function"));
    assert!(error.contains("execute"), "should list available functions");
}
