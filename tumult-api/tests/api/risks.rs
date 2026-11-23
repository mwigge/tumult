//! `GET /api/risks`, `GET /api/posture` and `POST /api/risks/{id}/suppress`
//! — the passive risk scan over the seeded store.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use tumult_lake::{ExerciseType, ManualOutcome, NewManualExperiment, SpanRow};

use crate::common::*;

async fn post(base: &str, path: &str, body: Value) -> (u16, Value) {
    let resp = reqwest::Client::new()
        .post(format!("{base}{path}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let body = resp.json::<Value>().await.unwrap_or(Value::Null);
    (status, body)
}

/// Seed: `database` has pg-failover (passed) and cache-stampede (failed),
/// both chaos. The scan flags the failed latest run and the six missing
/// disciplines, ranked highest first, and covers the target in the matrix.
#[tokio::test]
async fn risks_lists_ranked_findings_for_seeded_target() {
    let srv = spawn_server().await;
    let (status, body) = get(&srv.base, "/api/risks").await;
    assert_eq!(status, 200, "{body}");
    let risks = body["risks"].as_array().unwrap();
    assert!(!risks.is_empty(), "{body}");
    let ranks: Vec<u64> = risks.iter().map(|r| r["rank"].as_u64().unwrap()).collect();
    let mut sorted = ranks.clone();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    assert_eq!(ranks, sorted, "ranked highest first");
    assert!(risks
        .iter()
        .all(|r| r["id"].as_str().unwrap().starts_with("r_")));
    assert!(
        risks.iter().any(|r| r["rule"] == "latest_run_failed"
            && r["target"] == "database"
            && r["detail"].as_str().unwrap().contains("cache-stampede")),
        "{body}"
    );
    assert!(
        risks
            .iter()
            .any(|r| r["rule"] == "discipline_gap" && r["target"] == "database"),
        "{body}"
    );
    assert_eq!(body["summary"]["total"], risks.len());
    assert_eq!(body["summary"]["suppressed"], 0);
    let coverage = &body["coverage"];
    assert!(
        coverage["targets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["target"] == "database"),
        "{body}"
    );
    assert_eq!(coverage["kinds"].as_array().unwrap().len(), 4);
    assert!(body["as_of_ns"].is_number());
}

#[tokio::test]
async fn posture_returns_score_summary_and_top_risks() {
    let srv = spawn_server().await;
    let (status, body) = get(&srv.base, "/api/posture").await;
    assert_eq!(status, 200, "{body}");
    let score = body["posture"]["score"].as_f64().unwrap();
    assert!((0.0..=100.0).contains(&score), "{body}");
    assert!(body["posture"]["band"].is_string());
    assert!(
        body["posture"]["targets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["target"] == "database"),
        "{body}"
    );
    let top = body["top_risks"].as_array().unwrap();
    assert!(!top.is_empty() && top.len() <= 5, "{body}");
    assert_eq!(
        body["summary"]["total"],
        body["summary"]["total"].as_u64().unwrap()
    );
    assert!(body["coverage"]["targets"].is_array());
    assert!(body["nfrs"].is_array(), "{body}");
}

/// Suppressing a finding moves it from `risks` to `suppressed` with the
/// reason attached; the row comes back hash-chained.
#[tokio::test]
async fn suppress_moves_a_risk_aside() {
    let srv = spawn_server().await;
    let (_, before) = get(&srv.base, "/api/risks").await;
    let id = before["risks"][0]["id"].as_str().unwrap().to_string();
    let total = before["summary"]["total"].as_u64().unwrap();

    let (status, row) = post(
        &srv.base,
        &format!("/api/risks/{id}/suppress"),
        json!({"reason": "accepted until Q1", "expires_in_days": 30, "actor": "ops"}),
    )
    .await;
    assert_eq!(status, 201, "{row}");
    assert_eq!(row["risk_id"], id);
    assert_eq!(row["actor"], "ops");
    assert!(row["expires_at_ns"].as_i64().unwrap() > row["at_ns"].as_i64().unwrap());
    assert!(row["prev_hash"].is_null());
    assert!(row["new_hash"].is_string());

    let (_, after) = get(&srv.base, "/api/risks").await;
    assert_eq!(after["summary"]["total"], total - 1, "{after}");
    assert_eq!(after["summary"]["suppressed"], 1);
    assert!(after["risks"]
        .as_array()
        .unwrap()
        .iter()
        .all(|r| r["id"] != id));
    let hidden = &after["suppressed"][0];
    assert_eq!(hidden["id"], id);
    assert_eq!(hidden["suppression"]["reason"], "accepted until Q1");

    // A second suppression of the same finding chains onto the first.
    let (status, again) = post(
        &srv.base,
        &format!("/api/risks/{id}/suppress"),
        json!({"reason": "extended", "actor": "ops"}),
    )
    .await;
    assert_eq!(status, 201, "{again}");
    assert_eq!(again["prev_hash"], row["new_hash"]);
}

#[tokio::test]
async fn suppress_validates_input_and_unknown_ids() {
    let srv = spawn_server().await;
    let (status, body) = post(
        &srv.base,
        "/api/risks/r_nope/suppress",
        json!({"reason": "x", "actor": "ops"}),
    )
    .await;
    assert_eq!(status, 404, "{body}");
    let (_, before) = get(&srv.base, "/api/risks").await;
    let id = before["risks"][0]["id"].as_str().unwrap();
    let (status, body) = post(
        &srv.base,
        &format!("/api/risks/{id}/suppress"),
        json!({"reason": "  ", "actor": "ops"}),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    let (status, body) = post(
        &srv.base,
        &format!("/api/risks/{id}/suppress"),
        json!({"reason": "x", "expires_in_days": 0, "actor": "ops"}),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    let (status, body) = post(
        &srv.base,
        &format!("/api/risks/{id}/suppress"),
        json!({"reason": "x"}),
    )
    .await;
    assert_eq!(status, 400, "actor is required while auth is open: {body}");
}

/// Scoped principals: an `env` outside the scopes is a 403; a viewer may
/// read the scan but not suppress.
#[tokio::test]
async fn risks_respect_scopes_and_roles() {
    let srv = spawn_server().await;
    let viewer = add_scoped_token(&srv, "risk-viewer", "viewer", &["demo"]).await;
    let (status, body) = get_auth(&srv.base, "/api/risks?env=prod", &viewer).await;
    assert_eq!(status, 403, "{body}");
    let (status, body) = get_auth(&srv.base, "/api/posture?env=demo", &viewer).await;
    assert_eq!(status, 200, "{body}");
    let (_, risks) = get_auth(&srv.base, "/api/risks", &viewer).await;
    let id = risks["risks"][0]["id"].as_str().unwrap();
    let (status, body) = post_auth(
        &srv.base,
        &format!("/api/risks/{id}/suppress"),
        &viewer,
        json!({"reason": "nope"}),
    )
    .await;
    assert_eq!(status, 403, "{body}");
    let operator = add_scoped_token(&srv, "risk-operator", "operator", &["demo"]).await;
    let (status, body) = post_auth(
        &srv.base,
        &format!("/api/risks/{id}/suppress"),
        &operator,
        json!({"reason": "accepted"}),
    )
    .await;
    assert_eq!(status, 201, "{body}");
    assert_eq!(body["actor"], "risk-operator");
}

fn risk_id(report: &Value, rule: &str, kind: Option<&str>) -> String {
    report["risks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["rule"] == rule && kind.is_none_or(|k| r["recommended_kind"] == k))
        .unwrap_or_else(|| panic!("no {rule} risk in {report}"))["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// A discipline gap becomes a registered, scaffolded experiment carrying
/// the discipline and the target, plus an autopilot candidate with a
/// `risk` trigger. Asking again dedupes on the content hash.
#[tokio::test]
async fn recommend_scaffolds_registers_and_proposes_a_candidate() {
    let _env = point_discovery_at_workspace_plugins();
    let srv = spawn_server().await;
    let (_, report) = get(&srv.base, "/api/risks").await;
    let id = risk_id(&report, "discipline_gap", Some("robustness"));
    let (status, body) = post(&srv.base, &format!("/api/risks/{id}/recommend"), json!({})).await;
    assert_eq!(status, 201, "{body}");
    assert_eq!(body["risk_id"], id);
    assert_eq!(body["recommendation"]["type"], "scaffold");
    assert_eq!(body["recommendation"]["kind"], "robustness");
    assert_eq!(body["recommendation"]["target"], "database");
    let exp = &body["experiment"];
    assert!(
        exp["registry_id"].as_str().is_some_and(|s| !s.is_empty()),
        "{body}"
    );
    assert_eq!(exp["registered"], true);
    // The registered definition is one a run can read: it names the kind
    // once, and the target of the finding.
    let toon = exp["toon"].as_str().unwrap();
    let definition =
        tumult_core::engine::parse_experiment(toon).unwrap_or_else(|e| panic!("{e}:\n{toon}"));
    assert_eq!(definition.kind, tumult_core::types::TestKind::Robustness);
    let target = definition.target.expect("the definition names its target");
    assert_eq!(target.system, "database");
    assert_eq!(target.environment.as_deref(), Some("demo"));
    let cand = &body["candidate"];
    assert_eq!(cand["trigger"]["type"], "risk");
    assert_eq!(cand["trigger"]["risk_id"], id);
    assert_eq!(cand["playbook_experiment"], exp["name"]);
    assert_eq!(cand["plugin"], "tumult-stress");
    assert_eq!(cand["action"], "cpu-stress");
    assert!(body["validator"]["enactable"].is_boolean(), "{body}");
    // No autopilot policy on the test server: validated, not gated.
    assert!(body["decision"].is_null(), "{body}");

    let (_, registry) = get(&srv.base, "/api/registry").await;
    let listed = registry["definitions"]
        .as_array()
        .or_else(|| registry.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(
        listed.iter().any(|d| d["id"] == exp["registry_id"]),
        "{registry}"
    );
    let (status, again) = post(&srv.base, &format!("/api/risks/{id}/recommend"), json!({})).await;
    assert_eq!(status, 201, "{again}");
    assert_eq!(again["experiment"]["registry_id"], exp["registry_id"]);
    assert_eq!(
        again["experiment"]["registered"], false,
        "dedup by content hash"
    );
}

/// A failed-run finding asks for a rerun of an experiment that was never
/// registered (404); viewers are refused (403); unknown ids are 404. The
/// 422 for disciplines with no automated fault (security) is covered by
/// the handler's unit tests; the seed has no critical target to raise it.
#[tokio::test]
async fn recommend_refuses_what_it_cannot_automate() {
    let _env = point_discovery_at_workspace_plugins();
    let srv = spawn_server().await;
    let (_, report) = get(&srv.base, "/api/risks").await;
    let failed = risk_id(&report, "latest_run_failed", None);
    let (status, body) = post(
        &srv.base,
        &format!("/api/risks/{failed}/recommend"),
        json!({}),
    )
    .await;
    assert_eq!(status, 404, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("cache-stampede"),
        "{body}"
    );
    let (status, _) = post(&srv.base, "/api/risks/r_nope/recommend", json!({})).await;
    assert_eq!(status, 404);
    let viewer = add_scoped_token(&srv, "rec-viewer", "viewer", &["demo"]).await;
    let dr = risk_id(&report, "discipline_gap", Some("robustness"));
    let (status, body) = post_auth(
        &srv.base,
        &format!("/api/risks/{dr}/recommend"),
        &viewer,
        json!({}),
    )
    .await;
    assert_eq!(status, 403, "{body}");
}

/// The seed's root spans carry the target but the action spans do not, so
/// the `tumult` service reports level `none` with the target keys missing;
/// the contract and the summary come with it.
#[tokio::test]
async fn otel_conformance_reports_per_service_contract_coverage() {
    let srv = spawn_server().await;
    let (status, body) = get(&srv.base, "/api/otel/conformance?range=24h").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["contract"]["target"].as_array().unwrap().len(), 3);
    assert_eq!(body["contract"]["spine"].as_array().unwrap().len(), 4);
    let services = body["services"].as_array().unwrap();
    let tumult = services
        .iter()
        .find(|s| s["service"] == "tumult")
        .unwrap_or_else(|| panic!("{body}"));
    assert!(tumult["spans"].as_u64().unwrap() >= 2);
    assert_eq!(tumult["level"], "none");
    let missing: Vec<&str> = tumult["missing"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m.as_str().unwrap())
        .collect();
    assert!(missing.contains(&"resilience.target.system"), "{tumult}");
    assert!(missing.contains(&"ict.entity.id"), "{tumult}");
    let keys = tumult["keys"].as_array().unwrap();
    let system = keys
        .iter()
        .find(|k| k["key"] == "resilience.target.system")
        .unwrap();
    assert!(
        system["present"].as_u64().unwrap() >= 1,
        "root spans carry it: {tumult}"
    );
    assert_eq!(body["summary"]["services"], services.len());
    let (status, body) = get(&srv.base, "/api/otel/conformance?range=bogus").await;
    assert_eq!(status, 400, "{body}");
}

/// A record of `exercise` named `name` against `target` in `demo`, ended
/// with `outcome` an hour ago and entered by alice.
fn manual_record(
    name: &str,
    exercise: ExerciseType,
    target: &str,
    outcome: ManualOutcome,
) -> NewManualExperiment {
    NewManualExperiment {
        experiment_name: name.into(),
        exercise_type: exercise,
        executed_at_ns: now_ns() - 3600 * NS,
        hypothesis: "the service holds".into(),
        method: "by hand".into(),
        outcome,
        hypothesis_met: Some(outcome == ManualOutcome::Passed),
        findings: None,
        action_items: vec![],
        target_system: Some(target.into()),
        target_environment: Some("demo".into()),
        blast_radius: None,
        recovery_time_s: None,
        duration_s: None,
        entered_by: "alice".into(),
        attestation: "attested".into(),
        renewal_due_ns: None,
        framework_refs: vec![],
        exercise: tumult_lake::ExerciseDetail::default(),
    }
}

/// Enter `record` and have a second person verify it.
async fn add_verified(srv: &TestServer, record: NewManualExperiment) {
    exec_write(srv, move |writer| {
        let id = writer
            .create_manual_draft(&record)
            .map_err(|e| e.to_string())?;
        writer
            .submit_manual(&id, None, "alice")
            .map_err(|e| e.to_string())?;
        writer
            .verify_manual(&id, "bob", None)
            .map_err(|e| e.to_string())
    })
    .await;
}

/// `discipline:status` of every coverage cell of `target`.
fn coverage_of(report: &Value, target: &str) -> Vec<String> {
    let row = report["coverage"]["targets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["target"] == target)
        .unwrap_or_else(|| panic!("no coverage row of {target} in {report}"));
    row["cells"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            let kind = c["kind"].as_str().unwrap();
            format!("{kind}:{}", c["status"].as_str().unwrap())
        })
        .collect()
}

/// `rule:subject` of every finding about `target`, sorted.
fn findings_of(report: &Value, target: &str) -> Vec<String> {
    let mut rows: Vec<String> = report["risks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["target"] == target)
        .map(|r| {
            let rule = r["rule"].as_str().unwrap();
            format!("{rule}:{}", r["subject"].as_str().unwrap())
        })
        .collect();
    rows.sort();
    rows
}

/// A server span of `service` in the checkout trace, the child of
/// `parent` when one is given.
fn call_span(service: &str, span_id: &str, parent: Option<&str>) -> SpanRow {
    SpanRow {
        ts_ns: now_ns() - 60 * NS,
        trace_id: "trace-checkout".into(),
        span_id: span_id.into(),
        parent_span_id: parent.map(str::to_string),
        span_name: "GET /".into(),
        span_kind: "Server".into(),
        service_name: service.into(),
        events: "[]".into(),
        ..SpanRow::default()
    }
}

/// The disciplines of the evidence rows of `experiment` in a DORA
/// article document.
fn evidence_kinds(doc: &Value, experiment: &str) -> BTreeSet<Option<String>> {
    doc["requirements"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|r| r["entities"].as_array().unwrap())
        .flat_map(|e| e["evidence"].as_array().unwrap())
        .filter(|row| row["experiment"] == experiment)
        .map(|row| row["kind"].as_str().map(str::to_string))
        .collect()
}

/// A pentest is security evidence in the risk view, as it is in the
/// DORA score: its target is covered in security and not in chaos.
#[tokio::test]
async fn a_verified_pentest_covers_its_target_in_security_and_not_in_chaos() {
    let srv = spawn_server().await;
    let pentest = manual_record(
        "edge-pentest",
        ExerciseType::Pentest,
        "edge",
        ManualOutcome::Passed,
    );
    add_verified(&srv, pentest).await;

    let (status, body) = get(&srv.base, "/api/risks").await;

    assert_eq!(status, 200, "{body}");
    assert_eq!(
        coverage_of(&body, "edge"),
        [
            "chaos:missing",
            "performance:missing",
            "robustness:missing",
            "security:covered"
        ]
    );
}

/// A tabletop is no test under the default house parameters: it counts
/// towards no discipline. A target whose only evidence is a tabletop is
/// covered in nothing, has a gap in every core discipline, has no
/// freshness and scores 0, and as a service that others call it is an
/// untested dependency. The record stays in the list of the target, so
/// a failed tabletop still raises its own finding, which names no
/// discipline to test next.
#[tokio::test]
async fn a_target_whose_only_evidence_is_a_tabletop_has_no_discipline_covered() {
    let srv = spawn_server().await;
    // checkout calls ledger.
    exec_write(&srv, |writer| {
        writer
            .insert_spans(&[
                call_span("checkout", "span-checkout", None),
                call_span("ledger", "span-ledger", Some("span-checkout")),
            ])
            .map_err(|e| e.to_string())
    })
    .await;
    let tabletop = manual_record(
        "ledger-tabletop",
        ExerciseType::Tabletop,
        "ledger",
        ManualOutcome::Failed,
    );
    add_verified(&srv, tabletop).await;

    let (status, risks) = get(&srv.base, "/api/risks").await;
    assert_eq!(status, 200, "{risks}");
    assert_eq!(
        coverage_of(&risks, "ledger"),
        [
            "chaos:missing",
            "performance:missing",
            "robustness:missing",
            "security:missing"
        ]
    );
    assert_eq!(
        findings_of(&risks, "ledger"),
        [
            "discipline_gap:chaos",
            "discipline_gap:performance",
            "discipline_gap:robustness",
            "latest_run_failed:ledger-tabletop",
            "untested_dependency:dependency"
        ]
    );
    let failed = risks["risks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["rule"] == "latest_run_failed" && r["subject"] == "ledger-tabletop")
        .unwrap();
    assert_eq!(failed["recommended_kind"], Value::Null, "{failed}");

    let (status, posture) = get(&srv.base, "/api/posture").await;
    assert_eq!(status, 200, "{posture}");
    let ledger = posture["posture"]["targets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["target"] == "ledger")
        .unwrap_or_else(|| panic!("no posture of ledger in {posture}"));
    assert_eq!(ledger["components"]["coverage"], 0.0, "{ledger}");
    assert_eq!(ledger["components"]["freshness"], Value::Null, "{ledger}");
    assert_eq!(ledger["score"], 0.0, "{ledger}");
    assert_eq!(ledger["band"], "poor", "{ledger}");
}

/// The house parameters are the one source of the discipline of a manual
/// exercise. A house that counts a tabletop as chaos, and lists no other
/// type, gets the tabletop as chaos evidence in the risk view, in the
/// DORA score and on the Star Map, and a drill as evidence of no
/// discipline in all three.
#[tokio::test]
async fn a_house_that_counts_a_tabletop_as_chaos_moves_every_view_alike() {
    let house = tempfile::TempDir::new().unwrap();
    let org_file = house.path().join("org.yaml");
    let manual_kinds = "score:\n  manual_kinds: {tabletop: chaos}\n";
    std::fs::write(
        &org_file,
        format!("{}{manual_kinds}", crate::dora::REGISTER),
    )
    .unwrap();
    let srv = spawn_server_with_score_file(org_file).await;
    crate::dora::register(&srv).await;
    let tabletop = manual_record(
        "api-tabletop",
        ExerciseType::Tabletop,
        "payments-api",
        ManualOutcome::Passed,
    );
    let drill = manual_record(
        "archive-drill",
        ExerciseType::Drill,
        "ledger-archive",
        ManualOutcome::Passed,
    );
    add_verified(&srv, tabletop).await;
    add_verified(&srv, drill).await;

    let (status, risks) = get(&srv.base, "/api/risks").await;
    assert_eq!(status, 200, "{risks}");
    assert_eq!(
        coverage_of(&risks, "payments-api"),
        [
            "chaos:covered",
            "performance:missing",
            "robustness:missing",
            "security:missing"
        ]
    );
    assert_eq!(
        coverage_of(&risks, "ledger-archive"),
        [
            "chaos:missing",
            "performance:missing",
            "robustness:missing",
            "security:missing"
        ]
    );

    let (status, dora) = get(&srv.base, "/api/dora/24?entity=cif-0001").await;
    assert_eq!(status, 200, "{dora}");
    assert_eq!(dora["params"]["source"], "org_file", "{dora}");
    assert_eq!(
        evidence_kinds(&dora, "api-tabletop"),
        BTreeSet::from([Some("chaos".to_string())]),
        "{dora}"
    );
    let drill_kinds = evidence_kinds(&dora, "archive-drill");
    assert!(drill_kinds.iter().all(Option::is_none), "{drill_kinds:?}");

    let (status, map) = get(&srv.base, "/api/starmap").await;
    assert_eq!(status, 200, "{map}");
    let on_the_map = |system: &str| {
        let node = crate::starmap::node(&map, system);
        node.unwrap_or_else(|| panic!("no {system} in {map}"))["evidence"]["by_discipline"].clone()
    };
    assert_eq!(on_the_map("sys:demo/payments-api")["chaos"], "covered");
    assert_eq!(
        on_the_map("sys:demo/ledger-archive")["robustness"],
        "missing"
    );
}
