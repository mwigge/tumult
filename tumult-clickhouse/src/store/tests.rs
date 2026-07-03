use super::rows::{ActivityRow, ExperimentRow};
use super::*;

#[test]
fn retry_config_has_correct_defaults() {
    let config = RetryConfig::default();
    assert_eq!(config.max_attempts, 3);
    assert_eq!(config.backoff_durations.len(), 3);
    assert_eq!(
        config.backoff_durations[0],
        std::time::Duration::from_secs(2)
    );
    assert_eq!(
        config.backoff_durations[1],
        std::time::Duration::from_secs(4)
    );
    assert_eq!(
        config.backoff_durations[2],
        std::time::Duration::from_secs(8)
    );
}

#[test]
fn config_creates_valid_client() {
    let config = ClickHouseConfig::default();
    let _client = Client::default()
        .with_url(&config.url)
        .with_user(&config.user)
        .with_password(&config.password)
        .with_database(&config.database);
}

#[test]
fn schema_version_constant_is_valid() {
    const _: () = assert!(SCHEMA_VERSION >= 1);
}

#[test]
fn experiment_row_serializable() {
    let row = ExperimentRow {
        experiment_id: "e-001".into(),
        title: "test".into(),
        status: "Completed".into(),
        started_at_ns: 1_774_980_000_000_000_000,
        ended_at_ns: 1_774_980_060_000_000_000,
        duration_ms: 60_000,
        method_step_count: 1,
        rollback_count: 0,
        hypothesis_before_met: Some(1),
        hypothesis_after_met: None,
        estimate_accuracy: Some(0.95),
        resilience_score: None,
    };
    // Verify serde serialization works
    let json = serde_json::to_string(&row).unwrap();
    assert!(json.contains("e-001"));
}

#[test]
fn activity_row_serializable() {
    let row = ActivityRow {
        experiment_id: "e-001".into(),
        name: "test-action".into(),
        activity_type: "Action".into(),
        status: "Succeeded".into(),
        started_at_ns: 1_774_980_000_000_000_000,
        duration_ms: 500,
        output: Some("ok".into()),
        error: None,
        phase: "method".into(),
    };
    let json = serde_json::to_string(&row).unwrap();
    assert!(json.contains("test-action"));
}

/// Verifies that the synchronous `AnalyticsBackend` wrapper methods use
/// `block_in_place` rather than a bare `block_on`, which would panic when
/// called from inside an already-running multi-threaded Tokio context.
///
/// The test spawns a real multi-thread runtime and then, from within an
/// async task, calls the synchronous trait methods via a `ClickHouseStore`
/// configured with an unreachable URL.  The calls are expected to return
/// an `Err` (connection refused / timeout), but must NOT panic.
#[test]
fn analytics_backend_sync_methods_do_not_panic_inside_tokio_task() {
    use tumult_analytics::backend::AnalyticsBackend as _;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("build runtime");

    rt.block_on(async {
        // Spawn a task — this is the "inside a Tokio task" scenario that
        // previously triggered a panic from bare Handle::current().block_on().
        tokio::task::spawn(async {
            // Build a store pointing at an unreachable URL — no real
            // ClickHouse needed; we only care that the call doesn't panic.
            let store = ClickHouseStore {
                client: Client::default().with_url("http://127.0.0.1:1"),
                database: "test".into(),
                query_timeout: std::time::Duration::from_millis(100),
            };

            // Each of these must not panic; errors are expected and OK.
            let _ = store.experiment_count();
            let _ = store.stats();
            let _ = store.schema_version();
        })
        .await
        .expect("task should not panic");
    });
}
