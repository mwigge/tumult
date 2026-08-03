use super::*;

use crate::error::ToolError;
use tumult_core::types::{
    Activity, ActivityType, Experiment, ExperimentStatus, HaltRecord, Journal, Provider,
    RegulatoryMapping, RegulatoryRequirement,
};
use tumult_lake::DecisionRecord;

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
/// service and the lineage yields recommendation candidates (same recipe as
/// the topology tool tests).
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

/// A seeded decision bound to a concrete playbook file and policy hash — the
/// shape the approval re-gate needs to re-evaluate.
fn decision_bound(id: &str, verdict: &str, playbook: &str, policy_hash: &str) -> DecisionRecord {
    let mut record = decision(id, verdict);
    record.playbook = Some(playbook.into());
    record.policy_hash = policy_hash.into();
    record
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
        0,
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
    let store = tumult_lake::AnalyticsStore::open(&db).unwrap();
    let rows = tumult_query::autopilot_decisions(&store, None, 100).unwrap();
    assert_eq!(rows.len(), decisions.len(), "every decision must persist");
    let recs = tumult_query::graph_query(&store, "recommendation", None).unwrap();
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
        0,
    )
    .unwrap_err();
    assert!(err.to_string().contains("disabled"), "{err}");
}

#[test]
fn status_filters_by_verdict_and_honors_limit() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = empty_store(dir.path());
    {
        let store = tumult_lake::AnalyticsStore::open(&db).unwrap();
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
        let store = tumult_lake::AnalyticsStore::open(&db).unwrap();
        store
            .insert_autopilot_decision(&decision("d-prop", "propose"))
            .unwrap();
    }

    let report = autopilot_respond(
        db.to_str().unwrap(),
        "d-prop",
        false,
        Some("too risky"),
        None,
        0,
    )
    .unwrap();
    assert_eq!(report.structured["decision_id"], "d-prop");
    assert_eq!(report.structured["action"], "human_denied");

    let store = tumult_lake::AnalyticsStore::open(&db).unwrap();
    let status = tumult_query::autopilot_decision(&store, "d-prop")
        .unwrap()
        .unwrap();
    assert_eq!(status.last_event.as_deref(), Some("human_denied"));
    drop(store);

    // A decision takes exactly one human response.
    let err = autopilot_respond(db.to_str().unwrap(), "d-prop", false, None, None, 0).unwrap_err();
    assert!(err.to_string().contains("already resolved"), "{err}");
}

#[test]
fn respond_on_unknown_decision_is_not_found() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = empty_store(dir.path());
    let err =
        autopilot_respond(db.to_str().unwrap(), "no-such-id", true, None, None, 0).unwrap_err();
    assert!(matches!(err, ToolError::NotFound(_)), "{err}");
}

#[test]
fn export_writes_both_parquet_tables() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = empty_store(dir.path());
    {
        let store = tumult_lake::AnalyticsStore::open(&db).unwrap();
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

// ── Concurrency veto (#3) ──────────────────────────────────────

#[test]
fn once_with_enactment_in_flight_vetoes_enact_verdicts() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = seed_broken_store(dir.path());
    crate::tools::topology_import(db.to_str().unwrap(), Some(TOPOLOGY_TOML), None).unwrap();
    with_plugin_catalog(dir.path());
    let policy_path = write_enabled_policy(dir.path());

    // The same pass that yields decisions with no enactment in flight must
    // veto every enact verdict once the ledger reads 1 — and with
    // execute=true nothing may run.
    let report = autopilot_once(
        db.to_str().unwrap(),
        policy_path.to_str().unwrap(),
        true,
        None,
        1,
    )
    .unwrap();
    let decisions = report.structured["decisions"].as_array().unwrap();
    assert!(!decisions.is_empty(), "expected at least one decision");
    assert_eq!(
        report.structured["enacted"], 0,
        "a concurrent enactment must veto every enact verdict"
    );
    assert!(
        decisions.iter().all(|d| d["run_status"].is_null()),
        "vetoed decisions must not run: {decisions:?}"
    );
    let vetoed_on_concurrency = decisions.iter().any(|d| {
        d["verdict"] == "veto"
            && d["detail"]
                .as_str()
                .unwrap_or_default()
                .contains("no_concurrent")
    });
    assert!(
        vetoed_on_concurrency,
        "at least one decision must veto on ambient.no_concurrent_experiment: {decisions:?}"
    );
    assert!(
        !dir.path().join("autopilot-journals").exists(),
        "no playbook journal may be written when the gate vetoes"
    );
}

// ── Approval re-gate (#2) ──────────────────────────────────────

#[test]
fn respond_approve_requires_policy_path() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = empty_store(dir.path());
    {
        let store = tumult_lake::AnalyticsStore::open(&db).unwrap();
        store
            .insert_autopilot_decision(&decision("d-prop", "propose"))
            .unwrap();
    }

    let err = autopilot_respond(db.to_str().unwrap(), "d-prop", true, None, None, 0).unwrap_err();
    assert!(err.to_string().contains("policy_path is required"), "{err}");

    // The usage error is validated before any event is appended: the
    // decision must remain unanswered.
    let store = tumult_lake::AnalyticsStore::open(&db).unwrap();
    let status = tumult_query::autopilot_decision(&store, "d-prop")
        .unwrap()
        .unwrap();
    assert!(
        !matches!(
            status.last_event.as_deref(),
            Some("human_approved" | "human_denied")
        ),
        "no response event may be recorded: {:?}",
        status.last_event
    );
}

#[test]
fn respond_approve_refused_when_policy_changed() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = empty_store(dir.path());
    let playbook = crate::tools::test_support::write_valid_experiment(dir.path());
    {
        let store = tumult_lake::AnalyticsStore::open(&db).unwrap();
        store
            .insert_autopilot_decision(&decision_bound("d-prop", "propose", &playbook, "old-hash"))
            .unwrap();
    }
    let policy_path = write_enabled_policy(dir.path());

    let err = autopilot_respond(
        db.to_str().unwrap(),
        "d-prop",
        true,
        None,
        Some(policy_path.to_str().unwrap()),
        0,
    )
    .unwrap_err();
    assert!(err.to_string().contains("approval refused"), "{err}");
    assert!(err.to_string().contains("policy changed"), "{err}");

    // The refusal is part of the audit trail, after the human response.
    let store = tumult_lake::AnalyticsStore::open(&db).unwrap();
    let status = tumult_query::autopilot_decision(&store, "d-prop")
        .unwrap()
        .unwrap();
    assert_eq!(status.last_event.as_deref(), Some("re_gate_refused"));
    drop(store);
    assert!(
        !dir.path().join("autopilot-journals").exists(),
        "a refused approval must not run the playbook"
    );
}

#[test]
fn respond_approve_refused_when_gate_now_vetoes() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = empty_store(dir.path());
    let playbook = crate::tools::test_support::write_valid_experiment(dir.path());
    let policy_path = write_enabled_policy(dir.path());
    let hash = tumult_autopilot::policy_hash(&std::fs::read_to_string(&policy_path).unwrap());
    {
        let store = tumult_lake::AnalyticsStore::open(&db).unwrap();
        store
            .insert_autopilot_decision(&decision_bound("d-prop", "propose", &playbook, &hash))
            .unwrap();
    }

    // The policy hash matches, but the enactment ledger now reads 1: the
    // re-gate must veto the stale approval instead of executing it.
    let err = autopilot_respond(
        db.to_str().unwrap(),
        "d-prop",
        true,
        Some("looks good"),
        Some(policy_path.to_str().unwrap()),
        1,
    )
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("approval refused by gate re-evaluation"),
        "{err}"
    );
    assert!(
        err.to_string().contains("no_concurrent_experiment"),
        "{err}"
    );

    let store = tumult_lake::AnalyticsStore::open(&db).unwrap();
    let status = tumult_query::autopilot_decision(&store, "d-prop")
        .unwrap()
        .unwrap();
    assert_eq!(status.last_event.as_deref(), Some("re_gate_refused"));
    drop(store);
    assert!(
        !dir.path().join("autopilot-journals").exists(),
        "a vetoed re-gate must not run the playbook"
    );
}

#[test]
fn load_policy_parse_error_relays_no_file_content() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = empty_store(dir.path());
    let policy_path = dir.path().join("autopilot.toml");
    // A secret-looking value in a malformed file must not be echoed back.
    std::fs::write(&policy_path, "[autopilot]\nenabled = \"hunter2-secret\n").unwrap();

    let err = autopilot_once(
        db.to_str().unwrap(),
        policy_path.to_str().unwrap(),
        false,
        None,
        0,
    )
    .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("invalid TOML"), "{msg}");
    assert!(
        !msg.contains("hunter2-secret"),
        "parse error must not relay file content: {msg}"
    );
}

/// An echo-backed probe whose output satisfies its tolerance — the guard
/// telemetry pre-flight runs it once and sees the safe condition hold.
fn echo_probe(name: &str) -> Activity {
    Activity {
        name: name.into(),
        activity_type: ActivityType::Probe,
        provider: Provider::Process {
            path: "echo".into(),
            arguments: vec!["hello".into()],
            env: std::collections::HashMap::new(),
            timeout_s: Some(5.0),
        },
        tolerance: Some(tumult_core::types::Tolerance::Exact {
            value: serde_json::Value::String("hello".into()),
        }),
        pause_before_s: None,
        pause_after_s: None,
        background: false,
        label_selector: None,
    }
}

/// Write an enact-eligible playbook: a steady-state probe, exactly one
/// fault, a rollback, and a guard whose pre-flight passes. Returns the path.
fn write_guarded_playbook(dir: &std::path::Path) -> String {
    let fault = Activity {
        name: "inject".into(),
        activity_type: ActivityType::Action,
        ..echo_probe("inject")
    };
    let rollback = Activity {
        name: "rollback".into(),
        activity_type: ActivityType::Action,
        tolerance: None,
        ..echo_probe("rollback")
    };
    let exp = Experiment {
        title: "enact-eligible playbook".into(),
        steady_state_hypothesis: Some(tumult_core::types::Hypothesis {
            title: "target observable".into(),
            probes: vec![echo_probe("steady-probe")],
        }),
        method: vec![fault],
        guards: vec![tumult_core::types::Guard {
            name: "guard".into(),
            probe: echo_probe("guard-probe"),
            min_breaches: 1,
        }],
        rollbacks: vec![rollback],
        ..Default::default()
    };
    let toon = toon_format::encode_default(&exp).unwrap();
    let path = dir.join("playbook.toon");
    std::fs::write(&path, toon).unwrap();
    path.to_str().unwrap().to_string()
}

#[test]
fn respond_approve_with_gate_passing_reruns_gate_and_executes() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = empty_store(dir.path());
    let playbook = write_guarded_playbook(dir.path());
    // A policy under which the decision's class is enact-eligible: tier
    // listed, class pretrusted (no autonomy record needed), guard required
    // and present with a passing telemetry pre-flight.
    let policy_path = dir.path().join("autopilot.toml");
    std::fs::write(
        &policy_path,
        format!(
            "[autopilot]\nenabled = true\nenact_tiers = [\"data\"]\n\n\
             [[autopilot.playbook]]\nplugin = \"tumult-db\"\naction = \"kill-primary\"\n\
             experiment = \"{playbook}\"\n\n\
             [[autopilot.pretrusted]]\nplugin = \"tumult-db\"\naction = \"kill-primary\"\n\
             tier = \"data\"\n"
        ),
    )
    .unwrap();
    let hash = tumult_autopilot::policy_hash(&std::fs::read_to_string(&policy_path).unwrap());
    {
        let store = tumult_lake::AnalyticsStore::open(&db).unwrap();
        store
            .insert_autopilot_decision(&decision_bound("d-prop", "propose", &playbook, &hash))
            .unwrap();
    }

    let report = autopilot_respond(
        db.to_str().unwrap(),
        "d-prop",
        true,
        Some("approved for enact"),
        Some(policy_path.to_str().unwrap()),
        0,
    )
    .unwrap();
    assert_eq!(report.structured["action"], "human_approved");
    assert!(report.text.contains("ran "), "{}", report.text);

    // Audit order: approved → re-gate passed → run completed.
    let store = tumult_lake::AnalyticsStore::open(&db).unwrap();
    let status = tumult_query::autopilot_decision(&store, "d-prop")
        .unwrap()
        .unwrap();
    assert_eq!(status.last_event.as_deref(), Some("run_completed"));
    drop(store);
    assert!(
        dir.path()
            .join("autopilot-journals")
            .join("d-prop.journal.toon")
            .exists(),
        "an approved, gate-passing decision must run its playbook"
    );
}

// ── Engine helpers ─────────────────────────────────────────────

use tumult_autopilot::{Candidate, ConfidenceTier, GateDecision, Trigger, Verdict};

#[test]
fn elapsed_days_measures_whole_days_and_saturates() {
    const DAY: i64 = 86_400_000_000_000;
    assert_eq!(engine::elapsed_days(2 * DAY, DAY), 1);
    assert_eq!(engine::elapsed_days(DAY - 1, 0), 0);
    // The largest representable age (i64::MAX ns ≈ 292 years) fits a u32.
    assert_eq!(engine::elapsed_days(i64::MAX, 0), 106_751);
    // A negative age (a clock reading before the evidence) maps to the
    // u32::MAX sentinel: treated as maximally stale.
    assert_eq!(engine::elapsed_days(0, DAY), u32::MAX);
}

#[test]
fn elapsed_hours_measures_fractional_hours_and_saturates() {
    const HOUR: i64 = 3_600_000_000_000;
    let hours = engine::elapsed_hours(5 * HOUR, 2 * HOUR);
    assert!((hours - 3.0).abs() < 1e-9, "got {hours}");
    let saturated = engine::elapsed_hours(0, HOUR);
    assert!(
        saturated.abs() < 1e-9,
        "now before then saturates to zero: {saturated}"
    );
}

#[test]
fn confidence_is_high_for_broken_controls_and_strong_scores() {
    assert_eq!(engine::confidence_for(0.1, true), ConfidenceTier::High);
    assert_eq!(engine::confidence_for(1.0, false), ConfidenceTier::High);
    assert_eq!(
        engine::confidence_for(0.99, false),
        ConfidenceTier::Directional
    );
}

#[test]
fn inspect_experiment_reports_structural_facts() {
    let dir = tempfile::TempDir::new().unwrap();

    let missing = dir.path().join("missing.toon");
    assert_eq!(
        engine::inspect_experiment(missing.to_str().unwrap()),
        (false, false, false, 0),
        "an unreadable file reads as no facts"
    );
    let bad = dir.path().join("bad.toon");
    std::fs::write(&bad, "title: [unterminated").unwrap();
    assert_eq!(
        engine::inspect_experiment(bad.to_str().unwrap()),
        (false, false, false, 0),
        "an unparseable file reads as no facts"
    );

    // Plain experiment: one fault, no hypothesis, rollback, or guard.
    let plain = crate::tools::test_support::write_valid_experiment(dir.path());
    assert_eq!(engine::inspect_experiment(&plain), (false, false, false, 1));

    // Enact-eligible playbook: steady-state, rollback + guard, one fault.
    let guarded = write_guarded_playbook(dir.path());
    assert_eq!(engine::inspect_experiment(&guarded), (true, true, true, 1));
}

/// Write a playbook whose only guard runs `probe`; returns the path.
fn write_playbook_with_guard(dir: &std::path::Path, probe: Activity) -> String {
    let exp = Experiment {
        title: "guard preflight playbook".into(),
        guards: vec![tumult_core::types::Guard {
            name: "guard".into(),
            probe,
            min_breaches: 1,
        }],
        ..Default::default()
    };
    let toon = toon_format::encode_default(&exp).unwrap();
    let path = dir.join("guard-preflight.toon");
    std::fs::write(&path, toon).unwrap();
    path.to_str().unwrap().to_string()
}

#[test]
fn preflight_guard_telemetry_is_none_without_a_guard() {
    let dir = tempfile::TempDir::new().unwrap();
    let missing = dir.path().join("missing.toon");
    assert_eq!(
        engine::preflight_guard_telemetry(missing.to_str().unwrap()),
        None,
        "an unreadable playbook cannot be pre-flighted"
    );
    let plain = crate::tools::test_support::write_valid_experiment(dir.path());
    assert_eq!(
        engine::preflight_guard_telemetry(&plain),
        None,
        "no guard means no telemetry to verify"
    );
}

#[test]
fn preflight_guard_telemetry_passes_when_the_probe_meets_its_tolerance() {
    let dir = tempfile::TempDir::new().unwrap();
    let playbook = write_playbook_with_guard(dir.path(), echo_probe("guard-probe"));
    assert_eq!(engine::preflight_guard_telemetry(&playbook), Some(true));
}

#[test]
fn preflight_guard_telemetry_fails_when_the_probe_fails() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut failing = echo_probe("guard-probe");
    failing.provider = Provider::Process {
        path: "false".into(),
        arguments: vec![],
        env: std::collections::HashMap::new(),
        timeout_s: Some(5.0),
    };
    let playbook = write_playbook_with_guard(dir.path(), failing);
    assert_eq!(
        engine::preflight_guard_telemetry(&playbook),
        Some(false),
        "a failing guard probe means the run would be blind"
    );
}

#[test]
fn preflight_guard_telemetry_fails_when_the_probe_has_no_tolerance() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut no_tolerance = echo_probe("guard-probe");
    no_tolerance.tolerance = None;
    let playbook = write_playbook_with_guard(dir.path(), no_tolerance);
    assert_eq!(
        engine::preflight_guard_telemetry(&playbook),
        Some(false),
        "a guard without a tolerance cannot be judged"
    );
}

#[test]
fn latest_evidence_ns_picks_the_newest_matching_edge() {
    let edge = |src: &str, rel: &str, dst: &str, ts: i64| tumult_graph::EdgeRecord {
        src: src.into(),
        rel: rel.into(),
        dst: dst.into(),
        run_id: "run".into(),
        ts,
        attrs: "{}".into(),
    };
    let inputs = crate::tools::topology::inputs::TopologyInputs {
        edges: vec![
            edge("exp-1", "evidences", "art", 100),
            edge("exp-1", "evidences", "art", 300),
            // Wrong rel, wrong dst, and an experiment outside the cell.
            edge("exp-1", "targets", "art", 900),
            edge("exp-1", "evidences", "other-art", 900),
            edge("exp-2", "evidences", "art", 900),
        ],
        services: vec![],
        services_with_attrs: vec![],
        articles: vec![],
        deviation_attrs: std::collections::HashMap::new(),
        depends_on: vec![],
    };
    let cell = tumult_graph::lineage::LineageCell {
        article_id: "art".into(),
        service_id: "svc:a".into(),
        status: tumult_graph::lineage::ControlServiceStatus::Evidenced,
        evidence_strength: None,
        cause: None,
        experiments: vec!["exp-1".into()],
    };
    assert_eq!(engine::latest_evidence_ns(&inputs, &cell), Some(300));

    let never_evidenced = tumult_graph::lineage::LineageCell {
        experiments: vec![],
        ..cell
    };
    assert_eq!(engine::latest_evidence_ns(&inputs, &never_evidenced), None);
}

#[test]
fn playbook_article_resolves_the_first_regulatory_citation() {
    let dir = tempfile::TempDir::new().unwrap();

    let exp = Experiment {
        title: "regulated playbook".into(),
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
    let path = dir.path().join("regulated.toon");
    std::fs::write(&path, toon_format::encode_default(&exp).unwrap()).unwrap();
    let article = engine::playbook_article(path.to_str().unwrap())
        .expect("a DORA requirement must resolve to an article");
    assert!(article.starts_with("compliance:DORA/"), "{article}");
    assert!(article.contains("Art"), "{article}");

    let missing = dir.path().join("missing.toon");
    assert_eq!(engine::playbook_article(missing.to_str().unwrap()), None);

    let plain = crate::tools::test_support::write_valid_experiment(dir.path());
    assert_eq!(
        engine::playbook_article(&plain),
        None,
        "an experiment without regulatory mapping evidences no article"
    );
}

fn candidate_with(id: &str, trigger: Trigger) -> Candidate {
    Candidate {
        id: id.into(),
        service_id: "svc:db".into(),
        tier: Some("data".into()),
        plugin: "tumult-db".into(),
        action: "kill-primary".into(),
        article_id: "compliance:DORA/Art.25".into(),
        score: 1.5,
        reasons: vec!["seeded".into()],
        confidence: ConfidenceTier::High,
        playbook_experiment: None,
        experiment_has_guard: false,
        experiment_has_rollback: false,
        experiment_has_steady_state: false,
        experiment_fault_count: 0,
        trigger,
    }
}

#[test]
fn persist_decision_records_every_verdict_and_trigger_shape() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = empty_store(dir.path());
    let store = tumult_lake::AnalyticsStore::open(&db).unwrap();
    let policy = LoadedPolicy::parse("[autopilot]\nenabled = true\n").unwrap();

    let cases: [(Trigger, Verdict, &str, &str); 4] = [
        (
            Trigger::Staleness {
                article_id: "compliance:DORA/Art.25".into(),
                age_days: 42,
            },
            Verdict::Enact,
            "staleness",
            "enact",
        ),
        (
            Trigger::BrokenControl {
                article_id: "compliance:DORA/Art.25".into(),
            },
            Verdict::Veto {
                rule: "ambient.no_concurrent_experiment".into(),
            },
            "broken_control",
            "veto",
        ),
        (
            Trigger::Manual,
            Verdict::Downgrade {
                reasons: vec!["cooldown active".into()],
            },
            "manual",
            "downgrade",
        ),
        (
            Trigger::ChangeEvent {
                source: "deploy".into(),
                detail: None,
            },
            Verdict::Propose {
                reasons: vec!["no playbook".into()],
            },
            "change_event",
            "propose",
        ),
    ];

    for (i, (trigger, verdict, want_trigger, want_verdict)) in cases.into_iter().enumerate() {
        let id = format!("d-{i}");
        let assembled = engine::Assembled {
            candidate: candidate_with(&id, trigger),
            decision: GateDecision {
                verdict,
                rules_evaluated: vec![("rule.a".into(), true)],
            },
            autonomy_score: Some(0.75),
        };
        #[allow(clippy::cast_possible_wrap)]
        engine::persist_decision(&store, &policy, &assembled, 1_000 + i as i64).unwrap();

        let status = tumult_query::autopilot_decision(&store, &id)
            .unwrap()
            .expect("the decision must be persisted before any action");
        let record = status.record;
        assert_eq!(record.trigger, want_trigger);
        assert_eq!(record.verdict, want_verdict);
        assert_eq!(record.autonomy_score, Some(0.75));
        assert_eq!(record.policy_hash, policy.policy_hash());
        match want_verdict {
            "veto" => assert_eq!(
                record.gate_detail["rule"], "ambient.no_concurrent_experiment",
                "a veto records the violated rule"
            ),
            "downgrade" => assert_eq!(
                record.gate_detail["reasons"],
                serde_json::json!(["cooldown active"]),
                "a downgrade records its reasons"
            ),
            "propose" => assert_eq!(
                record.gate_detail["reasons"],
                serde_json::json!(["no playbook"])
            ),
            _ => assert_eq!(record.gate_detail, serde_json::json!({})),
        }

        // Every decision mirrors a `rec:<id>` recommendation node into the graph.
        let nodes = tumult_query::graph_query(&store, "recommendation", None).unwrap();
        assert!(
            nodes.iter().any(|n| n.id == format!("rec:{id}")),
            "decision {id} must mirror a graph node: {:?}",
            nodes.iter().map(|n| n.id.as_str()).collect::<Vec<_>>()
        );
    }
}

#[test]
fn change_event_yields_a_revalidation_candidate_for_the_changed_service() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = empty_store(dir.path());

    // A playbook bound to service `db` whose experiment carries a DORA
    // regulatory mapping — the article the change event revalidates.
    let exp = Experiment {
        title: "regulated playbook".into(),
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
    let playbook = dir.path().join("regulated.toon");
    std::fs::write(&playbook, toon_format::encode_default(&exp).unwrap()).unwrap();

    let policy_text = format!(
        "[autopilot]\nenabled = true\n\n\
         [[autopilot.playbook]]\nplugin = \"tumult-db\"\naction = \"kill-primary\"\n\
         service = \"db\"\nexperiment = \"{}\"\n",
        playbook.display()
    );
    let policy = LoadedPolicy::parse(&policy_text).unwrap();

    let store = tumult_lake::AnalyticsStore::open(&db).unwrap();
    let now = now_ns();
    store
        .record_change_event("db", now - 1_000, "deploy", Some("v2 rollout"))
        .unwrap();

    let out = engine::assemble_candidates(&store, &policy, now, true, 3, 0).unwrap();
    let matching: Vec<_> = out
        .iter()
        .filter(|a| a.candidate.service_id == "svc:db")
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "one change-event candidate per changed service: {:?}",
        out.iter()
            .map(|a| a.candidate.service_id.as_str())
            .collect::<Vec<_>>()
    );
    let candidate = &matching[0].candidate;
    assert!(
        matches!(candidate.trigger, Trigger::ChangeEvent { .. }),
        "expected a change-event trigger: {:?}",
        candidate.trigger
    );
    assert_eq!(candidate.confidence, ConfidenceTier::High);
    assert!(
        candidate.article_id.starts_with("compliance:DORA/"),
        "the candidate revalidates the playbook's article: {}",
        candidate.article_id
    );
    assert!(
        candidate.reasons.iter().any(|r| r.contains("deploy")),
        "the reason names the change source: {:?}",
        candidate.reasons
    );
}

#[test]
fn notify_change_records_the_event_and_rejects_a_missing_store() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = empty_store(dir.path());

    let report = autopilot_notify_change(db.to_str().unwrap(), "db", "deploy", Some("v2")).unwrap();
    assert_eq!(report.structured["service"], "db");
    assert_eq!(report.structured["source"], "deploy");
    assert!(
        report.text.contains("change event recorded"),
        "{}",
        report.text
    );

    let err = autopilot_notify_change(
        dir.path().join("missing.duckdb").to_str().unwrap(),
        "db",
        "deploy",
        None,
    )
    .expect_err("a missing store must be reported");
    assert!(matches!(err, ToolError::NotFound(_)), "got: {err}");
}

#[test]
fn respond_rejects_a_decision_that_is_not_proposable() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = empty_store(dir.path());
    {
        let store = tumult_lake::AnalyticsStore::open(&db).unwrap();
        store
            .insert_autopilot_decision(&decision("d-enact", "enact"))
            .unwrap();
    }

    let err = autopilot_respond(db.to_str().unwrap(), "d-enact", false, None, None, 0)
        .expect_err("an enacted decision takes no human response");
    let msg = err.to_string();
    assert!(msg.contains("enact"), "must name the verdict: {msg}");
    assert!(
        msg.contains("only propose/downgrade take a human response"),
        "{msg}"
    );
}

#[test]
fn once_with_a_missing_policy_file_is_not_found() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = empty_store(dir.path());
    let missing = dir.path().join("no-such-policy.toml");

    let err = autopilot_once(
        db.to_str().unwrap(),
        missing.to_str().unwrap(),
        false,
        None,
        0,
    )
    .expect_err("a missing policy file must fail before any store work");
    assert!(matches!(err, ToolError::NotFound(_)), "got: {err}");
    assert!(err.to_string().contains("no-such-policy.toml"), "{err}");
}

#[test]
fn respond_approve_with_a_missing_policy_file_fails_before_any_event() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = empty_store(dir.path());
    {
        let store = tumult_lake::AnalyticsStore::open(&db).unwrap();
        store
            .insert_autopilot_decision(&decision("d-prop", "propose"))
            .unwrap();
    }
    let missing = dir.path().join("no-such-policy.toml");

    let err = autopilot_respond(
        db.to_str().unwrap(),
        "d-prop",
        true,
        None,
        Some(missing.to_str().unwrap()),
        0,
    )
    .expect_err("an unloadable policy must refuse the approval");
    assert!(err.to_string().contains("no-such-policy.toml"), "{err}");

    // The usage error must not burn the decision's one human response.
    let store = tumult_lake::AnalyticsStore::open(&db).unwrap();
    let status = tumult_query::autopilot_decision(&store, "d-prop")
        .unwrap()
        .unwrap();
    assert!(
        !matches!(
            status.last_event.as_deref(),
            Some("human_approved" | "human_denied")
        ),
        "no human event may be appended on a usage error: {:?}",
        status.last_event
    );
}

#[test]
fn status_on_a_missing_store_is_not_found() {
    let dir = tempfile::TempDir::new().unwrap();
    let missing = dir.path().join("missing.duckdb");
    let err = autopilot_status(missing.to_str().unwrap(), None, None)
        .expect_err("a missing store must be reported");
    assert!(matches!(err, ToolError::NotFound(_)), "got: {err}");
}
