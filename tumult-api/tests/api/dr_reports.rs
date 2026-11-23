//! `POST /api/reports/v2/generate` for the DR reports: the DR test report
//! of a target over a period (`dr-test`), the DR exercise report of one
//! manual record (`dr-exercise`), and the DORA matrix of the evidence pack.
//!
//! Each test seeds one lake through the writer of the server: a DR run of
//! `ledger` stored as its journal, and a failover of `ledger` entered by
//! hand and verified. Then it asks the router for the report.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tumult_core::types::{
    Experiment, ExperimentStatus, ExperimentTarget, ObjectiveOutcome, PostResult, TestKind,
};
use tumult_lake::{ExerciseDetail, ExerciseType, ManualOutcome, NewManualExperiment};

use crate::common::*;

const DAY: i64 = 86_400 * NS;

/// A lake with the DR run `ledger-dr`, 10 days ago, back 4 s after the
/// outage against an RTO of 10 s, and a verified failover of `ledger`,
/// 5 days ago. Returns the id of the failover.
async fn seed(srv: &TestServer) -> String {
    let now = now_ns();
    let started = now - 10 * DAY;
    let mut journal =
        tumult_lake::duckdb_store::sample_journal("ledger-dr", ExperimentStatus::Completed);
    journal.kind = TestKind::Dr;
    journal.experiment_title = "ledger promote standby".into();
    journal.started_at_ns = started;
    journal.ended_at_ns = started + 60 * NS;
    journal.objectives = Some(ObjectiveOutcome {
        rto_target_s: Some(10.0),
        rto_measured_s: Some(4.0),
        ..ObjectiveOutcome::default()
    });
    journal.post_result = Some(PostResult {
        started_at_ns: started + 20 * NS,
        ended_at_ns: started + 50 * NS,
        duration_s: 30.0,
        samples: 30,
        probes: vec![],
        recovery_time_s: 3.0,
        full_recovery: true,
        mttr_s: Some(4.0),
        impact_observed: Some(true),
        outage_started_at_ns: Some(started + 19 * NS),
        recovered_at_ns: Some(started + 23 * NS),
    });
    let definition = Experiment {
        title: journal.experiment_title.clone(),
        target: Some(ExperimentTarget {
            system: "ledger".into(),
            technology: Some("postgresql".into()),
            environment: Some("prod".into()),
            criticality: None,
            namespace: None,
            version: None,
        }),
        ..Experiment::default()
    };
    let failover = NewManualExperiment {
        experiment_name: "ledger failover".into(),
        exercise_type: ExerciseType::Failover,
        executed_at_ns: now - 5 * DAY,
        hypothesis: "The standby takes the writes within its RTO".into(),
        method: "Stop the primary and promote the standby".into(),
        outcome: ManualOutcome::Passed,
        hypothesis_met: Some(true),
        findings: None,
        action_items: vec![],
        target_system: Some("ledger".into()),
        target_environment: Some("prod".into()),
        blast_radius: None,
        recovery_time_s: Some(600.0),
        duration_s: None,
        entered_by: "carol".into(),
        attestation: "I attest this record reflects the exercise as executed.".into(),
        renewal_due_ns: None,
        framework_refs: vec![],
        exercise: ExerciseDetail {
            rto_target_s: Some(900.0),
            rpo_target_s: Some(60.0),
            ..ExerciseDetail::default()
        },
    };
    let id = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let stored = std::sync::Arc::clone(&id);
    exec_write(srv, move |w| {
        w.ingest_journal(&journal, Some(&definition))
            .map_err(|e| e.to_string())?;
        let id = w
            .create_manual_draft(&failover)
            .map_err(|e| e.to_string())?;
        w.submit_manual(&id, None, "carol")
            .map_err(|e| e.to_string())?;
        w.verify_manual(&id, "bob", None)
            .map_err(|e| e.to_string())?;
        *stored.lock().unwrap() = id;
        Ok(())
    })
    .await;
    let id = id.lock().unwrap().clone();
    id
}

async fn generate(base: &str, body: Value) -> (u16, Value) {
    let resp = reqwest::Client::new()
        .post(format!("{base}/api/reports/v2/generate"))
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap())
}

async fn html_of(base: &str, id: &str) -> String {
    let resp = reqwest::get(format!("{base}/api/reports/v2/{id}/html"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    resp.text().await.unwrap()
}

#[tokio::test]
async fn a_dr_test_report_is_made_for_a_target_and_states_the_sha256_of_its_pdf() {
    let srv = spawn_server().await;
    seed(&srv).await;

    let (status, meta) = generate(&srv.base, json!({"type": "dr-test", "target": "ledger"})).await;

    assert_eq!(status, 200, "{meta}");
    let id = meta["doc_id"].as_str().unwrap();
    assert!(id.starts_with("KRK-R4-"), "{id}");
    assert_eq!(meta["type"], "dr-test");
    assert_eq!(meta["title"], "DR test report: ledger");
    assert_eq!(meta["params"]["target"], "ledger");
    assert!(meta["created_ns"].as_i64().unwrap() > 0, "{meta}");
    let pdf = reqwest::get(format!("{}/api/reports/v2/{id}/pdf", srv.base))
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert!(pdf.starts_with(b"%PDF"));
    let sha256: String = Sha256::digest(&pdf)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(meta["sha256"], sha256, "the hash of the PDF it serves");
    let html = html_of(&srv.base, id).await;
    assert!(html.contains("ledger promote standby"), "{html}");
    assert!(html.contains("ledger failover"), "{html}");
    assert!(html.contains("DR owner"), "{html}");
    let (_, list) = get(&srv.base, "/api/reports/v2").await;
    assert!(
        list["reports"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["doc_id"] == id),
        "{list}"
    );
}

#[tokio::test]
async fn a_dr_test_report_needs_a_target_and_a_period_that_ends_after_it_starts() {
    let srv = spawn_server().await;

    let (status, body) = generate(&srv.base, json!({"type": "dr-test"})).await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"], "dr-test requires target");
    let (status, body) = generate(
        &srv.base,
        json!({"type": "dr-test", "target": "ledger", "from": 20, "to": 10}),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"], "from must not be later than to");
}

#[tokio::test]
async fn a_dr_test_report_of_a_scoped_user_reads_only_its_environments() {
    let srv = spawn_server().await;
    seed(&srv).await;
    let token = add_scoped_token(&srv, "stg-only", "operator", &["staging"]).await;
    let body = json!({"type": "dr-test", "target": "ledger"});

    let (status, meta) = post_auth(&srv.base, "/api/reports/v2/generate", &token, body).await;
    assert_eq!(status, 200, "{meta}");
    let id = meta["doc_id"].as_str().unwrap();
    let resp = reqwest::Client::new()
        .get(format!("{}/api/reports/v2/{id}/html", srv.base))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let html = resp.text().await.unwrap();
    assert!(
        html.contains("No DR test of ledger in this period."),
        "{html}"
    );
    assert!(!html.contains("ledger promote standby"), "{html}");

    let body = json!({"type": "dr-test", "target": "ledger", "environment": "prod"});
    let (status, body) = post_auth(&srv.base, "/api/reports/v2/generate", &token, body).await;
    assert_eq!(status, 403, "{body}");
}

#[tokio::test]
async fn a_dr_exercise_report_is_made_of_one_dr_record() {
    let srv = spawn_server().await;
    let record = seed(&srv).await;

    let (status, body) = generate(&srv.base, json!({"type": "dr-exercise"})).await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"], "dr-exercise requires record_id");
    let (status, body) = generate(
        &srv.base,
        json!({"type": "dr-exercise", "record_id": "no-such-record"}),
    )
    .await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(
        body["error"],
        "record_id names no submitted or verified DR exercise in your environments"
    );

    let (status, meta) = generate(
        &srv.base,
        json!({"type": "dr-exercise", "record_id": record}),
    )
    .await;

    assert_eq!(status, 200, "{meta}");
    let id = meta["doc_id"].as_str().unwrap();
    assert!(id.starts_with("KRK-R5-"), "{id}");
    assert_eq!(meta["params"]["record_id"], record.as_str());
    assert_eq!(meta["sha256"].as_str().unwrap().len(), 64);
    let html = html_of(&srv.base, id).await;
    assert!(html.contains(&record), "{html}");
    assert!(html.contains("The audit verify found no problem"), "{html}");
}

#[tokio::test]
async fn the_dora_evidence_pack_lists_the_rows_of_the_requirement_registry() {
    let srv = spawn_server().await;
    seed(&srv).await;

    let (status, meta) = generate(
        &srv.base,
        json!({"type": "evidence-pack", "framework": "dora"}),
    )
    .await;

    assert_eq!(status, 200, "{meta}");
    let html = html_of(&srv.base, meta["doc_id"].as_str().unwrap()).await;
    for cite in tumult_core::compliance::REQUIREMENTS.iter().map(|r| r.cite) {
        assert!(html.contains(&format!("<td>{cite}</td>")), "{cite}");
    }
    assert!(!html.contains("Unmapped"), "{html}");
    assert!(html.contains("<th>RTO actual</th>"), "{html}");
}

/// The DR reports and the evidence pack read the house of the daemon. A
/// house that counts a tabletop as DR has its tabletop in the DR test
/// report and gives its DR detail in the appendix of the evidence pack.
#[tokio::test]
async fn the_dr_reports_read_the_dr_types_of_the_house() {
    let house = tempfile::TempDir::new().unwrap();
    let org_file = house.path().join("org.yaml");
    std::fs::write(&org_file, "score:\n  manual_kinds: {tabletop: dr}\n").unwrap();
    let srv = spawn_server_with_score_file(org_file).await;
    let tabletop = NewManualExperiment {
        experiment_name: "ledger tabletop".into(),
        exercise_type: ExerciseType::Tabletop,
        executed_at_ns: now_ns() - 2 * DAY,
        hypothesis: "The team can restore the ledger from the runbook".into(),
        method: "Walk through the restore runbook".into(),
        outcome: ManualOutcome::Passed,
        hypothesis_met: Some(true),
        findings: None,
        action_items: vec![],
        target_system: Some("ledger".into()),
        target_environment: Some("prod".into()),
        blast_radius: None,
        recovery_time_s: Some(1200.0),
        duration_s: None,
        entered_by: "carol".into(),
        attestation: "I attest this record reflects the exercise as executed.".into(),
        renewal_due_ns: None,
        framework_refs: vec![],
        exercise: ExerciseDetail {
            rto_target_s: Some(1800.0),
            ..ExerciseDetail::default()
        },
    };
    exec_write(&srv, move |w| {
        let id = w
            .create_manual_draft(&tabletop)
            .map_err(|e| e.to_string())?;
        w.submit_manual(&id, None, "carol")
            .map_err(|e| e.to_string())?;
        w.verify_manual(&id, "bob", None).map_err(|e| e.to_string())
    })
    .await;

    let (status, meta) = generate(&srv.base, json!({"type": "dr-test", "target": "ledger"})).await;
    assert_eq!(status, 200, "{meta}");
    let html = html_of(&srv.base, meta["doc_id"].as_str().unwrap()).await;
    assert!(html.contains("ledger tabletop (manual tabletop)"), "{html}");

    let (status, meta) = generate(
        &srv.base,
        json!({"type": "evidence-pack", "framework": "dora"}),
    )
    .await;
    assert_eq!(status, 200, "{meta}");
    let html = html_of(&srv.base, meta["doc_id"].as_str().unwrap()).await;
    assert!(
        html.contains("<th>RTO planned / actual</th><td>1800.0 s / 1200.0 s</td>"),
        "{html}"
    );
}
