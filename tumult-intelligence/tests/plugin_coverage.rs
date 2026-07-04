//! Coverage-math test for the heuristic report with a fully controlled
//! plugin catalog.
//!
//! Plugin discovery scans `./plugins` (absent in this crate), `$HOME/.tumult/
//! plugins`, and `$TUMULT_PLUGIN_PATH`. This file overrides both environment
//! variables to point at temp directories, making the catalog exactly the
//! one manifest written below. Env vars are process-wide, so this binary
//! deliberately contains a single test.

use tumult_analytics::AnalyticsStore;
use tumult_core::types::{
    ActivityResult, ActivityStatus, ActivityType, ExperimentStatus, Journal, SpanId, TraceId,
};
use tumult_intelligence::heuristic_report;
use tumult_plugin::manifest::{ScriptAction, ScriptPluginManifest};

#[test]
fn coverage_counts_tested_actions_and_lists_only_untested_ones() {
    // Hermetic discovery: empty $HOME, catalog only via TUMULT_PLUGIN_PATH.
    let home = tempfile::TempDir::new().unwrap();
    let plugin_root = tempfile::TempDir::new().unwrap();
    std::env::set_var("HOME", home.path());
    std::env::set_var("TUMULT_PLUGIN_PATH", plugin_root.path());

    let manifest = ScriptPluginManifest {
        name: "ztest-plugin".to_string(),
        version: "0.0.1".to_string(),
        description: "synthetic plugin for coverage tests".to_string(),
        actions: vec![
            ScriptAction {
                name: "covered-action".to_string(),
                script: "covered.sh".into(),
                description: "already exercised".to_string(),
            },
            ScriptAction {
                name: "uncovered-action".to_string(),
                script: "uncovered.sh".into(),
                description: "never exercised".to_string(),
            },
        ],
        probes: vec![],
    };
    let plugin_dir = plugin_root.path().join("ztest-plugin");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("plugin.toon"),
        toon_format::encode_default(&manifest).expect("encode manifest"),
    )
    .unwrap();

    // History exercising exactly one of the two actions.
    let store_dir = tempfile::TempDir::new().unwrap();
    let store_path = store_dir.path().join("analytics.duckdb");
    {
        let store = AnalyticsStore::open(&store_path).expect("create store");
        let journal = Journal {
            experiment_title: "exercise covered action".into(),
            experiment_id: "exp-1".into(),
            status: ExperimentStatus::Completed,
            started_at_ns: 1_700_000_000_000_000_000,
            ended_at_ns: 1_700_000_060_000_000_000,
            duration_ms: 60_000,
            steady_state_before: None,
            steady_state_after: None,
            method_results: vec![ActivityResult {
                name: "covered-action".into(),
                activity_type: ActivityType::Action,
                status: ActivityStatus::Succeeded,
                started_at_ns: 1_700_000_000_000_000_000,
                duration_ms: 10,
                output: None,
                error: None,
                trace_id: TraceId("00000000000000000000000000000000".into()),
                span_id: SpanId("0000000000000000".into()),
            }],
            rollback_results: vec![],
            rollback_failures: 0,
            estimate: None,
            baseline_result: None,
            during_result: None,
            post_result: None,
            load_result: None,
            analysis: None,
            regulatory: None,
        };
        assert!(store.ingest_journal(&journal).expect("ingest"));
    }

    let report = heuristic_report(&store_path);

    assert!(
        report.contains("Coverage: 1/2 actions tested (50%)"),
        "tested action must count against the catalog: {report}"
    );
    assert!(
        report.contains("Untested actions (1):"),
        "exactly one action is untested: {report}"
    );
    assert!(
        report.contains("- ztest-plugin::uncovered-action"),
        "the untested action must be named: {report}"
    );
    assert!(
        !report.contains("- ztest-plugin::covered-action"),
        "tested actions must not be listed as untested: {report}"
    );
}
