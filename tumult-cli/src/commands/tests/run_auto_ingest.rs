//! Tests for `cmd_run` error boundaries (oversized input, non-completed
//! experiments) and the auto-ingest happy path into a temp analytics store.

use super::super::*;
use super::helpers::{use_temp_store, write_valid_experiment, ENV_LOCK};
use tempfile::TempDir;
use tumult_core::execution::RollbackStrategy;
use tumult_core::types::{Activity, ActivityType, Experiment, Provider};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_rejects_oversized_experiment_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("huge.toon");
    // Just over the 10MB limit enforced before parsing.
    std::fs::write(&path, "x".repeat(11 * 1024 * 1024)).unwrap();

    let err = cmd_run(
        &path,
        &dir.path().join("out.toon"),
        false,
        false,
        RollbackStrategy::OnDeviation,
        false,
        std::collections::HashMap::new(),
        None,
    )
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("experiment file too large"),
        "{err}"
    );
}

// The env guard must cover the awaited run (env vars are read inside it).
// Only tests serialized on ENV_LOCK can block on it, so holding a std guard
// across the await cannot deadlock the test runtime.
#[allow(clippy::await_holding_lock)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_auto_ingests_completed_journal_into_store() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    let db = use_temp_store(dir.path());
    std::env::remove_var("TUMULT_CLICKHOUSE_URL");
    std::env::remove_var("TUMULT_DAEMON_URL");
    let exp_path = write_valid_experiment(dir.path());
    let journal_path = dir.path().join("out.toon");

    cmd_run(
        &exp_path,
        &journal_path,
        false,
        false,
        RollbackStrategy::OnDeviation,
        true,
        std::collections::HashMap::new(),
        None,
    )
    .await
    .unwrap();

    std::env::remove_var("TUMULT_LAKE_PATH");

    let store = tumult_lake::AnalyticsStore::open_read_only(&db).unwrap();
    assert_eq!(store.experiment_count().unwrap(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_bails_when_experiment_does_not_complete() {
    let dir = TempDir::new().unwrap();

    // A method action that fails puts the journal in a non-completed status,
    // which `cmd_run` surfaces as an error exit.
    let experiment = Experiment {
        version: "v1".into(),
        title: "failing action experiment".into(),
        description: None,
        tags: vec![],
        configuration: indexmap::IndexMap::new(),
        secrets: indexmap::IndexMap::new(),
        controls: vec![],
        steady_state_hypothesis: None,
        method: vec![Activity {
            name: "failing-action".into(),
            activity_type: ActivityType::Action,
            provider: Provider::Process {
                path: "sh".into(),
                arguments: vec!["-c".into(), "exit 1".into()],
                env: std::collections::HashMap::new(),
                timeout_s: Some(5.0),
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
            label_selector: None,
        }],
        rollbacks: vec![],
        estimate: None,
        baseline: None,
        load: None,
        regulatory: None,
        guards: vec![],
        blast_radius: None,
        max_concurrent_faults: None,
    };
    let exp_path = dir.path().join("failing.toon");
    std::fs::write(&exp_path, toon_format::encode_default(&experiment).unwrap()).unwrap();
    let journal_path = dir.path().join("out.toon");

    let err = cmd_run(
        &exp_path,
        &journal_path,
        false,
        false,
        RollbackStrategy::Never,
        false,
        std::collections::HashMap::new(),
        None,
    )
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("experiment finished with status"),
        "{err}"
    );
    // The journal is still written: the failure evidence is preserved.
    assert!(journal_path.exists());
}
