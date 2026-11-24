//! The DORA fields of a manual record over the API: a requirement id the
//! registry does not know is refused with 422 and the nearest registry id,
//! a TLPT record keeps its attestation, its test summary report and its
//! remediation plan with retests, and findings, actions and the tester are
//! classified.

use crate::common::*;
use serde_json::{json, Value};

/// A TLPT record with every field the DORA gaps added.
fn tlpt_body(by: &str) -> Value {
    json!({
        "experiment_name": "payments TLPT",
        "exercise_type": "tlpt",
        "executed_at_ns": now_ns() - 30 * 86_400 * NS,
        "hypothesis": "A red team cannot reach the ledger",
        "method": "Threat-led test of the payments chain",
        "outcome_status": "partial",
        "findings": "The red team reached the ledger by a stale VPN account",
        "findings_severity": "high",
        "target_system": "payments",
        "target_environment": "production",
        "entered_by": by,
        "attestation": "I attest this record reflects the test as executed.",
        "framework_refs": ["dora.art26.1"],
        "tester": {"name": "Red Team Ltd", "kind": "external"},
        "tlpt": {
            "attestation_ref": "TLPT-ATT-2026-07",
            "attested_at_ns": now_ns() - 86_400 * NS,
            "summary_report_ref": "TSR-2026-07"
        },
        "actions": [
            {"title": "Remove stale VPN accounts", "owner": "carol", "severity": "critical",
             "retest": {"kind": "run", "id": "exp-vpn-retest"}},
            {"title": "Alert on a ledger login from the VPN", "owner": "dave",
             "severity": "medium"}
        ]
    })
}

/// Send `body` to a manual path and give the status and the body as text.
async fn send(method: reqwest::Method, base: &str, path: &str, body: &Value) -> (u16, String) {
    let resp = reqwest::Client::new()
        .request(method, format!("{base}/api/manual/{path}"))
        .json(body)
        .send()
        .await
        .unwrap();
    (resp.status().as_u16(), resp.text().await.unwrap())
}

async fn post(base: &str, path: &str, body: &Value) -> (u16, String) {
    send(reqwest::Method::POST, base, path, body).await
}

#[tokio::test]
async fn an_unknown_requirement_id_is_refused_with_422_and_the_nearest_registry_id() {
    let srv = spawn_server().await;
    let base = srv.base.clone();
    let mut typo = tlpt_body("alice");
    typo["framework_refs"] = json!(["dora.art26.1", "dora.art24.7"]);
    let (status, text) = post(&base, "experiments", &typo).await;
    assert_eq!(status, 422, "{text}");
    assert!(
        text.contains(
            "'dora.art24.7' is not a requirement id of the DORA registry; \
             did you mean 'dora.art24.1'?"
        ),
        "{text}"
    );
    typo["framework_refs"] = json!(["Art. 12(1)"]);
    let (status, text) = post(&base, "experiments", &typo).await;
    assert_eq!(status, 422, "{text}");
    assert!(text.contains("did you mean 'dora.art12.1'?"), "{text}");

    let (status, text) = post(&base, "experiments", &tlpt_body("alice")).await;
    assert_eq!(status, 201, "{text}");
    let id = serde_json::from_str::<Value>(&text).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let path = format!("experiments/{id}");
    let (status, text) = send(reqwest::Method::PUT, &base, &path, &typo).await;
    assert_eq!(status, 422, "{text}");
    let batch = json!({"records": [tlpt_body("carol"), typo]});
    let (status, text) = post(&base, "import", &batch).await;
    assert_eq!(status, 422, "{text}");

    let (_, all) = get(&base, "/api/manual/experiments").await;
    let records = all["records"].as_array().unwrap();
    assert_eq!(records.len(), 1, "{all}");
    assert_eq!(records[0]["framework_refs"], json!(["dora.art26.1"]));
}

#[tokio::test]
async fn an_old_form_of_a_known_requirement_is_kept_as_written() {
    let srv = spawn_server().await;
    let base = srv.base.clone();
    let mut old = tlpt_body("alice");
    old["framework_refs"] = json!(["DORA Art. 26(1)", "ISO 27001 A.5.30"]);
    let (status, text) = post(&base, "experiments", &old).await;
    assert_eq!(status, 201, "{text}");
    let (_, all) = get(&base, "/api/manual/experiments").await;
    assert_eq!(
        all["records"][0]["framework_refs"],
        json!(["DORA Art. 26(1)", "ISO 27001 A.5.30"])
    );
}

#[tokio::test]
async fn a_tlpt_record_keeps_its_tlpt_block_remediation_plan_and_tester() {
    let srv = spawn_server().await;
    let base = srv.base.clone();
    let (status, text) = post(&base, "experiments", &tlpt_body("alice")).await;
    assert_eq!(status, 201, "{text}");
    let id = serde_json::from_str::<Value>(&text).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let (status, detail) = get(&base, &format!("/api/manual/experiments/{id}")).await;
    assert_eq!(status, 200, "{detail}");
    let row = &detail["experiment"];
    assert_eq!(row["findings_severity"], "high");
    assert_eq!(row["tester"], "Red Team Ltd");
    assert_eq!(row["tester_kind"], "external");
    assert_eq!(row["tlpt_attestation_ref"], "TLPT-ATT-2026-07");
    assert!(row["tlpt_attested_at_ns"].is_i64(), "{row}");
    assert_eq!(row["tlpt_summary_ref"], "TSR-2026-07");
    let actions = detail["actions"].as_array().unwrap();
    assert_eq!(actions.len(), 2);
    assert_eq!(actions[0]["severity"], "critical");
    assert_eq!(actions[0]["retest_kind"], "run");
    assert_eq!(actions[0]["retest_id"], "exp-vpn-retest");
    assert_eq!(actions[1]["severity"], "medium");
    assert_eq!(actions[1]["retest_id"], Value::Null);

    // The record verifies with its new fields sealed.
    let by = json!({"by": "alice"});
    let (status, _) = post(&base, &format!("experiments/{id}/submit"), &by).await;
    assert_eq!(status, 200);
    let reviewer = json!({"reviewer": "bob"});
    let (status, text) = post(&base, &format!("experiments/{id}/verify"), &reviewer).await;
    assert_eq!(status, 200, "{text}");
    let (_, verdict) = get(&base, &format!("/api/manual/experiments/{id}/audit/verify")).await;
    assert_eq!(verdict["ok"], true, "{verdict}");
}

#[tokio::test]
async fn a_bad_severity_tester_or_retest_kind_or_a_misplaced_tlpt_block_is_refused() {
    let srv = spawn_server().await;
    let base = srv.base.clone();
    let cases: [(&str, Value, &str); 5] = [
        (
            "findings_severity",
            json!("severe"),
            "unknown severity 'severe' (low|medium|high|critical)",
        ),
        (
            "tester",
            json!({"name": "x", "kind": "vendor"}),
            "unknown tester kind 'vendor' (internal|external)",
        ),
        (
            "actions",
            json!([{"title": "t", "owner": "o", "severity": "urgent"}]),
            "unknown severity 'urgent'",
        ),
        (
            "actions",
            json!([{"title": "t", "owner": "o", "retest": {"kind": "ticket", "id": "x"}}]),
            "unknown retest kind 'ticket' (run|record)",
        ),
        (
            "exercise_type",
            json!("pentest"),
            "the tlpt block is for a record of exercise_type 'tlpt' only",
        ),
    ];
    for (field, value, needle) in cases {
        let mut bad = tlpt_body("alice");
        bad[field] = value;
        let (status, text) = post(&base, "experiments", &bad).await;
        assert_eq!(status, 400, "{field}: {text}");
        assert!(text.contains(needle), "{field}: {text}");
    }
    let (_, all) = get(&base, "/api/manual/experiments").await;
    assert!(all["records"].as_array().unwrap().is_empty(), "{all}");
}

#[tokio::test]
async fn the_tester_tlpt_and_retest_bodies_refuse_a_field_they_do_not_know() {
    let srv = spawn_server().await;
    let base = srv.base.clone();
    let mut tester = tlpt_body("alice");
    tester["tester"]["organisation"] = json!("Red Team Ltd");
    let mut tlpt = tlpt_body("alice");
    tlpt["tlpt"]["authority"] = json!("central bank");
    let mut retest = tlpt_body("alice");
    retest["actions"][0]["retest"]["passed"] = json!(true);
    for (body, field) in [
        (tester, "organisation"),
        (tlpt, "authority"),
        (retest, "passed"),
    ] {
        let (status, text) = post(&base, "experiments", &body).await;
        assert_eq!(status, 422, "{field}: {text}");
        assert!(text.contains(&format!("unknown field `{field}`")), "{text}");
    }
}
