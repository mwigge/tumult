use super::*;
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
    drop(tumult_lake::AnalyticsStore::open(&db).unwrap());
    db
}

/// Seed a store with a guard-halted (broken) run: a DORA-mapped experiment
/// targeting `svc:db` that deviated, so the control is Broken on that
/// service (`maps_to_compliance` + targets, but no evidences edge).
fn seed_broken_store(dir: &std::path::Path) -> std::path::PathBuf {
    let db = dir.join("analytics.duckdb");
    let store = tumult_lake::AnalyticsStore::open(&db).unwrap();
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
            tester: None,
            scenarios: Vec::new(),
        }),
        ..Default::default()
    };
    let journal = Journal {
        target_version: None,
        target: None,
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
        kind: tumult_core::types::TestKind::default(),
        objectives: None,
        probe_samples: vec![],
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

/// Point script-plugin discovery at a temp catalog with one action so
/// recommendations have a non-empty catalog regardless of the host machine.
/// Discovery stays pinned there for as long as the returned guard lives.
fn with_plugin_catalog(dir: &std::path::Path) -> tumult_test_utils::EnvGuard {
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
    crate::tools::test_support::plugin_path_guard(&dir.join("catalog"))
}

#[test]
fn import_inline_reports_counts_and_service_ids() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = empty_store(dir.path());
    let report = topology_import(db.to_str().unwrap(), Some(TOPOLOGY_TOML), None).unwrap();
    assert_eq!(report.text, "imported 3 services, 2 dependencies\n");
    assert_eq!(report.structured["services"], 3);
    assert_eq!(report.structured["dependencies"], 2);
    assert_eq!(
        report.structured["service_ids"],
        serde_json::json!(["svc:api", "svc:db", "svc:gateway"])
    );

    // Re-import converges (idempotent).
    let again = topology_import(db.to_str().unwrap(), Some(TOPOLOGY_TOML), None).unwrap();
    assert_eq!(again.structured["services"], 3);
}

#[test]
fn map_text_lists_imported_services() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = empty_store(dir.path());
    topology_import(db.to_str().unwrap(), Some(TOPOLOGY_TOML), None).unwrap();
    let report = topology_map(db.to_str().unwrap(), None, None, None, None, None).unwrap();
    for svc in ["svc:gateway", "svc:api", "svc:db"] {
        assert!(report.text.contains(svc), "{svc} missing: {}", report.text);
    }
    assert!(report.text.starts_with("legend:"));
    assert_eq!(report.structured["format"], "text");
    assert_eq!(
        report.structured["map"]["services"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn map_mermaid_contains_classdefs_and_edges() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = empty_store(dir.path());
    topology_import(db.to_str().unwrap(), Some(TOPOLOGY_TOML), None).unwrap();
    let report = topology_map(
        db.to_str().unwrap(),
        None,
        None,
        Some("mermaid"),
        None,
        None,
    )
    .unwrap();
    assert!(report.text.starts_with("graph TD"));
    assert!(report.text.contains("classDef"));
    assert!(report.text.contains("svc_gateway --> svc_api"));
}

#[test]
fn map_json_carries_view_in_structured_content() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = empty_store(dir.path());
    topology_import(db.to_str().unwrap(), Some(TOPOLOGY_TOML), None).unwrap();
    let report = topology_map(
        db.to_str().unwrap(),
        Some("dora"),
        None,
        Some("json"),
        Some(false),
        None,
    )
    .unwrap();
    assert!(report.text.contains("full view in structured content"));
    assert!(report.structured["map"]["depends_on"].is_array());
}

#[test]
fn lineage_on_broken_run_reports_broken_with_cause() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = seed_broken_store(dir.path());
    topology_import(db.to_str().unwrap(), Some(TOPOLOGY_TOML), None).unwrap();

    let report = compliance_lineage(db.to_str().unwrap(), Some("dora"), None, None).unwrap();
    assert!(report.text.contains("BROKEN"), "{}", report.text);
    assert!(
        report.text.contains("guard: p95_latency"),
        "{}",
        report.text
    );
    assert!(report.structured["counts"]["broken"].as_u64().unwrap() >= 1);

    // Service filter narrows to the broken service's cells only.
    let filtered =
        compliance_lineage(db.to_str().unwrap(), Some("dora"), None, Some("db")).unwrap();
    let cells = filtered.structured["cells"].as_array().unwrap();
    assert!(!cells.is_empty());
    assert!(cells.iter().all(|c| c["service_id"] == "svc:db"));
    assert!(cells.iter().any(|c| c["status"] == "broken"));
}

#[test]
fn recommend_returns_ranked_explained_recommendations() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = seed_broken_store(dir.path());
    topology_import(db.to_str().unwrap(), Some(TOPOLOGY_TOML), None).unwrap();
    let _plugins = with_plugin_catalog(dir.path());

    let report = recommend_injection(db.to_str().unwrap(), Some("dora"), Some(3)).unwrap();
    let recs = report.structured["recommendations"].as_array().unwrap();
    assert!(!recs.is_empty(), "expected at least one recommendation");
    assert!(recs.len() <= 3);
    for rec in recs {
        assert!(!rec["reasons"].as_array().unwrap().is_empty());
    }
    assert!(report.text.contains("1. "), "{}", report.text);
}

/// A store with a broken run of a DORA-mapped drill on `svc:db` whose
/// action `start-thing` calls `p::do_thing`, and the declared topology.
fn seed_broken_thing_store(dir: &std::path::Path) -> String {
    let db = dir.join("analytics.duckdb");
    let store = tumult_lake::AnalyticsStore::open(&db).unwrap();
    let mut action = crate::tools::test_support::script_action("start-thing", "p", "do_thing");
    if let Provider::Script { arguments, .. } = &mut action.provider {
        arguments.insert("upstream".into(), "db:5432".into());
    }
    let drill = Experiment {
        title: "Thing drill".into(),
        method: vec![action],
        regulatory: Some(RegulatoryMapping {
            frameworks: vec!["DORA".into()],
            requirements: vec![RegulatoryRequirement {
                id: "Art. 25".into(),
                description: "Testing of ICT tools and systems".into(),
                evidence: "scenario-based fault injection".into(),
            }],
            tester: None,
            scenarios: Vec::new(),
        }),
        ..Default::default()
    };
    let journal = crate::tools::test_support::journal_of(
        "run-thing-1",
        "Thing drill",
        ExperimentStatus::Deviated,
        &["start-thing"],
        &[],
    );
    store
        .ingest_journal_with_experiment(&journal, Some(&drill))
        .unwrap();
    drop(store);
    let db = db.to_str().unwrap().to_string();
    topology_import(&db, Some(TOPOLOGY_TOML), None).unwrap();
    db
}

/// The `(plugin::action, reasons)` of each recommendation for DORA.
fn recommended(db: &str) -> Vec<(String, Vec<String>)> {
    let report = recommend_injection(db, Some("dora"), Some(3)).unwrap();
    let recs = report.structured["recommendations"].as_array().unwrap();
    recs.iter()
        .map(|rec| {
            let reasons = rec["reasons"].as_array().unwrap();
            (
                format!(
                    "{}::{}",
                    rec["plugin"].as_str().unwrap(),
                    rec["action"].as_str().unwrap()
                ),
                reasons
                    .iter()
                    .map(|r| r.as_str().unwrap().to_string())
                    .collect(),
            )
        })
        .collect()
}

#[test]
fn a_fault_a_stored_run_tested_earns_no_novelty_bonus() {
    let dir = tempfile::TempDir::new().unwrap();
    let _env = crate::tools::test_support::catalog_guard(dir.path(), &[("p", &["do_thing"])]);
    let db = seed_broken_thing_store(dir.path());

    let recs = recommended(&db);
    assert!(!recs.is_empty());
    for (action, reasons) in &recs {
        assert_eq!(action, "p::do_thing");
        assert!(
            !reasons.iter().any(|r| r.contains("never tested")),
            "{reasons:?}"
        );
    }
}

#[test]
fn the_pick_prefers_a_plugin_function_no_stored_run_tested() {
    let dir = tempfile::TempDir::new().unwrap();
    let _env = crate::tools::test_support::catalog_guard(
        dir.path(),
        &[("p", &["do_thing", "other_thing"])],
    );
    let db = seed_broken_thing_store(dir.path());

    let recs = recommended(&db);
    assert!(!recs.is_empty());
    for (action, reasons) in &recs {
        assert_eq!(action, "p::other_thing");
        assert!(
            reasons.contains(&"action p::other_thing never tested".to_string()),
            "{reasons:?}"
        );
    }
}

#[test]
fn import_via_nonexistent_path_errors_cleanly() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = empty_store(dir.path());
    let err = topology_import(
        db.to_str().unwrap(),
        None,
        Some("/nonexistent/topology.toml"),
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("cannot read topology file"),
        "{err}"
    );
}

#[test]
fn import_requires_exactly_one_source() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = empty_store(dir.path());
    for (content, path) in [(None, None), (Some(TOPOLOGY_TOML), Some("x.toml"))] {
        let err = topology_import(db.to_str().unwrap(), content, path).unwrap_err();
        assert!(err.to_string().contains("exactly one"), "{err}");
    }
}

#[test]
fn invalid_toml_and_unknown_scopes_are_rejected() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = empty_store(dir.path());
    let err = topology_import(db.to_str().unwrap(), Some("not toml {{{"), None).unwrap_err();
    assert!(err.to_string().contains("parse"), "{err}");

    let err =
        topology_map(db.to_str().unwrap(), Some("hipaa"), None, None, None, None).unwrap_err();
    assert!(err.to_string().contains("hipaa"), "{err}");

    let err = topology_map(db.to_str().unwrap(), None, None, Some("dot"), None, None).unwrap_err();
    assert!(err.to_string().contains("mermaid"), "{err}");
}

#[test]
fn missing_store_is_a_clean_error_on_every_tool() {
    let missing = "/nonexistent/tumult-topology.duckdb";
    // Import creates a missing store (it is legitimately the first write a
    // fresh deployment performs) but still rejects a missing *directory* —
    // the typo guard.
    let err = topology_import(missing, Some(TOPOLOGY_TOML), None).unwrap_err();
    assert!(
        err.to_string().contains("store directory not found"),
        "{err}"
    );
    for err in [
        topology_map(missing, None, None, None, None, None).unwrap_err(),
        compliance_lineage(missing, None, None, None).unwrap_err(),
        recommend_injection(missing, None, None).unwrap_err(),
    ] {
        assert!(err.to_string().contains("store not found"), "{err}");
    }
}
