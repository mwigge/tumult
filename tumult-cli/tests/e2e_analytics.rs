//! E2E test: run experiment → analyze journal → export parquet
//! Validates the full data pipeline: TOON → Arrow → `DuckDB` → Parquet

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;

use indexmap::IndexMap;
use tempfile::TempDir;
use tumult_core::types::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
// This test covers the full analytics pipeline in a single function by design;
// splitting it would obscure the end-to-end narrative.
#[allow(clippy::too_many_lines)]
async fn e2e_run_analyze_export() {
    let dir = TempDir::new().unwrap();

    // Create a simple experiment with process provider
    let probe_script = dir.path().join("probe.sh");
    std::fs::write(&probe_script, "#!/bin/sh\necho 200\nexit 0\n").unwrap();
    std::fs::set_permissions(&probe_script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let action_script = dir.path().join("action.sh");
    std::fs::write(&action_script, "#!/bin/sh\necho injected\nexit 0\n").unwrap();
    std::fs::set_permissions(&action_script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let experiment = Experiment {
        version: "v1".into(),
        title: "Analytics E2E test".into(),
        description: Some("Validates DuckDB analytics pipeline".into()),
        tags: vec!["e2e".into(), "analytics".into()],
        configuration: IndexMap::new(),
        secrets: IndexMap::new(),
        controls: vec![],
        steady_state_hypothesis: Some(Hypothesis {
            title: "Probe returns 200".into(),
            probes: vec![Activity {
                name: "health-probe".into(),
                activity_type: ActivityType::Probe,
                provider: Provider::Process {
                    path: probe_script.to_str().unwrap().into(),
                    arguments: vec![],
                    env: HashMap::new(),
                    timeout_s: Some(5.0),
                },
                tolerance: Some(Tolerance::Exact {
                    value: serde_json::Value::Number(200.into()),
                }),
                pause_before_s: None,
                pause_after_s: None,
                background: false,
                label_selector: None,
            }],
        }),
        method: vec![Activity {
            name: "inject-fault".into(),
            activity_type: ActivityType::Action,
            provider: Provider::Process {
                path: action_script.to_str().unwrap().into(),
                arguments: vec![],
                env: HashMap::new(),
                timeout_s: Some(10.0),
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
            label_selector: None,
        }],
        rollbacks: vec![],
        estimate: Some(Estimate {
            expected_outcome: ExpectedOutcome::Recovered,
            expected_recovery_s: Some(5.0),
            expected_degradation: None,
            expected_data_loss: None,
            confidence: Some(Confidence::High),
            rationale: None,
            prior_runs: None,
        }),
        baseline: None,
        load: None,
        regulatory: None,
    };

    // Run experiment
    let executor: std::sync::Arc<dyn tumult_core::runner::ActivityExecutor> =
        std::sync::Arc::new(tumult_cli::commands::ProviderExecutor);
    let controls = std::sync::Arc::new(tumult_core::controls::ControlRegistry::new());
    let config = tumult_core::runner::RunConfig::default();
    let journal =
        tumult_core::runner::run_experiment(&experiment, &executor, &controls, &config).unwrap();

    assert_eq!(journal.status, ExperimentStatus::Completed);

    // Write journal to file
    let journal_path = dir.path().join("journal.toon");
    tumult_core::journal::write_journal(&journal, &journal_path).unwrap();

    // Ingest into DuckDB and query
    let store = tumult_analytics::AnalyticsStore::in_memory().unwrap();
    store.ingest_journal(&journal).unwrap();

    // Verify experiments table
    let rows = store
        .query("SELECT experiment_id, title, status FROM experiments")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][1], "Analytics E2E test");
    assert_eq!(rows[0][2], "completed");

    // Verify activity_results table
    let act_rows = store
        .query("SELECT name, phase, status FROM activity_results ORDER BY phase, name")
        .unwrap();
    assert!(act_rows.len() >= 2); // At least hypothesis probe + method action

    // Verify method phase exists
    let method_rows = store
        .query("SELECT name FROM activity_results WHERE phase = 'method'")
        .unwrap();
    assert_eq!(method_rows.len(), 1);
    assert_eq!(method_rows[0][0], "inject-fault");

    // Export to Parquet
    let parquet_path = dir.path().join("test.parquet");
    let (exp_batch, _) =
        tumult_analytics::journal_to_record_batch(std::slice::from_ref(&journal)).unwrap();
    tumult_analytics::export_parquet(&exp_batch, &parquet_path).unwrap();
    assert!(parquet_path.exists());
    assert!(std::fs::metadata(&parquet_path).unwrap().len() > 0);

    // Export to CSV
    let csv_path = dir.path().join("test.csv");
    tumult_analytics::export_csv(&exp_batch, &csv_path).unwrap();
    let csv_content = std::fs::read_to_string(&csv_path).unwrap();
    assert!(csv_content.contains("Analytics E2E test"));

    // Export to Arrow IPC
    let arrow_path = dir.path().join("test.arrow");
    tumult_analytics::export_arrow_ipc(&exp_batch, &arrow_path).unwrap();
    assert!(arrow_path.exists());
}
