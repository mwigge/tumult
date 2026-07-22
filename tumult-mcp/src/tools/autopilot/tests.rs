use super::*;

use crate::error::ToolError;
use tumult_analytics::DecisionRecord;
use tumult_core::types::{
    Activity, ActivityType, Experiment, ExperimentStatus, HaltRecord, Journal, Provider,
    RegulatoryMapping, RegulatoryRequirement,
};

const TOPOLOGY_TOML: &str = r#"
    [[service]]
    name = "gateway"
    depends_on = ["api"]
    tier = "edge"

    [[service]]
    name = "api"
    depends_on = ["db"]
    owner = "team-core"

    [[service]]
    name = "db"
    tier = "data"
"#;

/// Create an empty analytics store and return its path.
fn empty_store(dir: &std::path::Path) -> std::path::PathBuf {
    let db = dir.join("analytics.duckdb");
    drop(tumult_analytics::AnalyticsStore::open(&db).unwrap());
    db
}

/// Seed a store with a guard-halted (broken) run: a DORA-mapped experiment
/// targeting `svc:db` that deviated, so the control is Broken on that
/// service and the lineage yields recommendation candidates (same recipe as
/// the topology tool tests).
fn seed_broken_store(dir: &std::path::Path) -> std::path::PathBuf {
    let db = dir.join("analytics.duckdb");
    let store = tumult_analytics::AnalyticsStore::open(&db).unwrap();
    let exp = Experiment {
        title: "DB failover drill".into(),
        method: vec![Activity {
            name: "kill-primary".into(),
            activity_type: ActivityType::Action,
            provider: Provider::Native {
                plugin: "tumult-db".into(),
                function: "kill_primary".into(),
                arguments: std::collections::HashMap::from([(
                    "upstream".into(),
                    serde_json::Value::String("db:5432".into()),
                )]),
            },
            ..Default::default()
        }],
        regulatory: Some(RegulatoryMapping {
            frameworks: vec!["DORA".into()],
            requirements: vec![RegulatoryRequirement {
                id: "Art. 25".into(),
                description: "Testing of ICT tools and systems".into(),
                evidence: "scenario-based fault injection".into(),
            }],
        }),
        ..Default::default()
    };
    let journal = Journal {
        experiment_title: "DB failover drill".into(),
        experiment_id: "run-broken-1".into(),
        status: ExperimentStatus::Deviated,
        started_at_ns: 1,
        ended_at_ns: 2,
        duration_ms: 1,
        steady_state_before: None,
        steady_state_after: None,
        method_results: vec![],
        rollback_results: vec![],
        rollback_failures: 0,
        estimate: None,
        baseline_result: None,
        during_result: None,
        post_result: None,
        load_result: None,
        analysis: None,
        regulatory: None,
        halt: Some(HaltRecord {
            guard_name: "p95_latency".into(),
            observed: Some("2.5s".into()),
            safe_condition: "range [0, 1s]".into(),
            breach_count: 3,
            breached_at_ns: 2,
            time_to_halt_ms: 800,
            rollback_ms: 87,
        }),
        blast_radius: None,
    };
    store
        .ingest_journal_with_experiment(&journal, Some(&exp))
        .unwrap();
    db
}

/// Point script-plugin discovery at a temp catalog with one action so the
/// recommender has a non-empty catalog regardless of the host machine.
fn with_plugin_catalog(dir: &std::path::Path) {
    let plugin_dir = dir.join("catalog").join("test-topology-plugin");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    let manifest = tumult_plugin::manifest::ScriptPluginManifest {
        name: "test-topology-plugin".into(),
        version: "0.1.0".into(),
        description: "test catalog".into(),
        actions: vec![tumult_plugin::manifest::ScriptAction {
            name: "test-inject".into(),
            script: "actions/test-inject.sh".into(),
            description: "test action".into(),
        }],
        probes: vec![],
    };
    let toon = toon_format::encode_default(&manifest).unwrap();
    std::fs::write(plugin_dir.join("plugin.toon"), toon).unwrap();
    std::env::set_var("TUMULT_PLUGIN_PATH", dir.join("catalog"));
}

/// Write an enabled policy binding the test catalog's action to a trivial
/// echo experiment (playbook). Returns the policy path.
fn write_enabled_policy(dir: &std::path::Path) -> std::path::PathBuf {
    let playbook = crate::tools::test_support::write_valid_experiment(dir);
    let policy_path = dir.join("autopilot.toml");
    std::fs::write(
        &policy_path,
        format!(
            "[autopilot]\nenabled = true\n\n\
             [[autopilot.playbook]]\n\
             plugin = \"test-topology-plugin\"\n\
             action = \"test-inject\"\n\
             experiment = \"{playbook}\"\n"
        ),
    )
    .unwrap();
    policy_path
}

/// Minimal decision row for seeding the status/respond/export tests
/// directly (no recommender in the loop).
fn decision(id: &str, verdict: &str) -> DecisionRecord {
    DecisionRecord {
        id: id.into(),
        decided_at_ns: 1_000,
        trigger: "staleness".into(),
        service_id: "svc:db".into(),
        tier: Some("data".into()),
        plugin: "tumult-db".into(),
        action: "kill-primary".into(),
        article_id: "compliance:DORA/Art. 25".into(),
        score: 1.5,
        reasons: serde_json::json!(["seeded"]),
        confidence: "high".into(),
        playbook: None,
        validator: serde_json::json!({}),
        verdict: verdict.into(),
        gate_rules: serde_json::json!([]),
        gate_detail: serde_json::json!({}),
        policy_hash: "test-hash".into(),
        autonomy_score: None,
    }
}

#[test]
fn once_without_execute_records_decisions_and_graph_lineage() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = seed_broken_store(dir.path());
    crate::tools::topology_import(db.to_str().unwrap(), Some(TOPOLOGY_TOML), None).unwrap();
    with_plugin_catalog(dir.path());
    let policy_path = write_enabled_policy(dir.path());

    let report = autopilot_once(
        db.to_str().unwrap(),
        policy_path.to_str().unwrap(),
        false,
        None,
    )
    .unwrap();

    // Every gated candidate is a decision with a verdict; nothing ran.
    let decisions = report.structured["decisions"].as_array().unwrap();
    assert!(!decisions.is_empty(), "expected at least one decision");
    for d in decisions {
        let verdict = d["verdict"].as_str().unwrap();
        assert!(
            matches!(verdict, "enact" | "downgrade" | "propose" | "veto"),
            "unexpected verdict {verdict}"
        );
        assert!(d["run_status"].is_null(), "execute=false must not run");
    }
    assert_eq!(report.structured["executed"], false);
    assert_eq!(report.structured["enacted"], 0);
    assert!(report.text.contains("autopilot pass:"), "{}", report.text);

    // Audit-before-act: rows persisted and mirrored into the graph.
    let store = tumult_analytics::AnalyticsStore::open(&db).unwrap();
    let rows = store.autopilot_decisions(None, 100).unwrap();
    assert_eq!(rows.len(), decisions.len(), "every decision must persist");
    let recs = store.graph_query("recommendation", None).unwrap();
    assert!(
        !recs.is_empty(),
        "each decision must mirror a recommendation node"
    );
}

#[test]
fn once_with_disabled_policy_is_an_error() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = empty_store(dir.path());
    let policy_path = dir.path().join("autopilot.toml");
    std::fs::write(&policy_path, "[autopilot]\nenabled = false\n").unwrap();

    let err = autopilot_once(
        db.to_str().unwrap(),
        policy_path.to_str().unwrap(),
        false,
        None,
    )
    .unwrap_err();
    assert!(err.to_string().contains("disabled"), "{err}");
}

#[test]
fn status_filters_by_verdict_and_honors_limit() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = empty_store(dir.path());
    {
        let store = tumult_analytics::AnalyticsStore::open(&db).unwrap();
        store
            .insert_autopilot_decision(&decision("d-prop", "propose"))
            .unwrap();
        store
            .insert_autopilot_decision(&decision("d-veto", "veto"))
            .unwrap();
    }

    let all = autopilot_status(db.to_str().unwrap(), None, None).unwrap();
    assert_eq!(all.structured["count"], 2);

    let proposed = autopilot_status(db.to_str().unwrap(), Some("propose"), None).unwrap();
    assert_eq!(proposed.structured["count"], 1);
    let items = proposed.structured["decisions"].as_array().unwrap();
    assert_eq!(items[0]["id"], "d-prop");
    assert_eq!(items[0]["verdict"], "propose");
    assert!(proposed.text.contains("[propose]"), "{}", proposed.text);

    let limited = autopilot_status(db.to_str().unwrap(), None, Some(1)).unwrap();
    assert_eq!(limited.structured["count"], 1);
}

#[test]
fn respond_deny_appends_the_veto_feedback_event() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = empty_store(dir.path());
    {
        let store = tumult_analytics::AnalyticsStore::open(&db).unwrap();
        store
            .insert_autopilot_decision(&decision("d-prop", "propose"))
            .unwrap();
    }

    let report =
        autopilot_respond(db.to_str().unwrap(), "d-prop", false, Some("too risky")).unwrap();
    assert_eq!(report.structured["decision_id"], "d-prop");
    assert_eq!(report.structured["action"], "human_denied");

    let store = tumult_analytics::AnalyticsStore::open(&db).unwrap();
    let status = store.autopilot_decision("d-prop").unwrap().unwrap();
    assert_eq!(status.last_event.as_deref(), Some("human_denied"));
    drop(store);

    // A decision takes exactly one human response.
    let err = autopilot_respond(db.to_str().unwrap(), "d-prop", false, None).unwrap_err();
    assert!(err.to_string().contains("already resolved"), "{err}");
}

#[test]
fn respond_on_unknown_decision_is_not_found() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = empty_store(dir.path());
    let err = autopilot_respond(db.to_str().unwrap(), "no-such-id", true, None).unwrap_err();
    assert!(matches!(err, ToolError::NotFound(_)), "{err}");
}

#[test]
fn export_writes_both_parquet_tables() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = empty_store(dir.path());
    {
        let store = tumult_analytics::AnalyticsStore::open(&db).unwrap();
        store
            .insert_autopilot_decision(&decision("d-exp", "enact"))
            .unwrap();
    }

    let out = dir.path().join("archive");
    let report = autopilot_export(db.to_str().unwrap(), out.to_str().unwrap()).unwrap();
    assert_eq!(report.structured["dir"], out.to_str().unwrap());
    assert!(out.join("autopilot_decisions.parquet").exists());
    assert!(out.join("autopilot_events.parquet").exists());
}
