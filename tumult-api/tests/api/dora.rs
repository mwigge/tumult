//! `GET /api/dora`, `GET /api/dora/{article}` and `GET /api/maturity` —
//! coverage, evidence status and the maturity roll-up over a store with a
//! function register.

use serde_json::Value;
use tumult_lake::SpanRow;

use crate::common::*;

const LEI: &str = "529900T8BM49AURSDO55";
const DAY: i64 = 86_400 * NS;

/// A small firm under the simplified framework: one critical function
/// with a declared service that sends nothing, one important function.
const REGISTER: &str = "
nodes:
  - {name: platform, kind: domain}
legal_entities:
  - lei: 529900T8BM49AURSDO55
    name: Demo Firm
    profile: simplified
    feeds: [register]
functions:
  - id: cif-0001
    name: Payments
    tier: critical
    org_unit: platform
    rto_s: 3600
    rpo_s: 300
    last_assessed: 2026-10-01
    licensed_activities: [payment-services]
    services: [payments-api, ledger-archive]
  - id: cif-0002
    name: Reporting
    tier: important
    org_unit: platform
    services: [reporting]
";

async fn register(srv: &TestServer) {
    let declared = tumult_compliance::dora::register::parse(REGISTER).unwrap();
    let at = now_ns();
    exec_write(srv, move |w| {
        w.sync_ict_register(&declared.ict, at)
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
    .await;
}

/// An application span of `service` carrying the whole ICT spine.
fn spine_span(service: &str, entity: &str, ts: i64) -> SpanRow {
    let pair = |k: &str, v: &str| (k.to_string(), v.to_string());
    SpanRow {
        ts_ns: ts,
        trace_id: format!("t-{service}-{ts}"),
        span_id: format!("s-{service}-{ts}"),
        span_name: "GET /".into(),
        span_kind: "Server".into(),
        service_name: service.into(),
        target_system: Some("payments".into()),
        target_technology: Some("rust".into()),
        target_environment: Some("demo".into()),
        resource_attrs: vec![
            pair("ict.schema.version", "1"),
            pair("ict.entity.id", entity),
            pair("ict.entity.tier", "critical"),
            pair("ict.entity.owner", "payments"),
        ],
        events: "[]".into(),
        ..SpanRow::default()
    }
}

async fn insert_spans(srv: &TestServer, spans: Vec<SpanRow>) {
    exec_write(srv, move |w| {
        w.insert_spans(&spans).map_err(|e| e.to_string())
    })
    .await;
}

fn list(value: &Value) -> &Vec<Value> {
    value
        .as_array()
        .unwrap_or_else(|| panic!("{value} is not a list"))
}

fn article_numbers(body: &Value) -> Vec<u32> {
    list(&body["articles"])
        .iter()
        .filter_map(|a| a["article"].as_str()?.parse().ok())
        .collect()
}

#[tokio::test]
async fn dora_without_a_register_answers_with_unknowns_not_zeros() {
    let srv = spawn_server().await;
    let (status, body) = get(&srv.base, "/api/dora").await;
    assert_eq!(status, 200, "{body}");
    assert!(body["as_of_ns"].is_number());
    assert!(body["registry_version"].is_string());
    assert!(body["disclaimer"].is_string());
    assert_eq!(body["readiness"]["registered"], 0);
    assert!(body["portfolio"]["unknown"].is_number(), "{body}");
    for article in list(&body["articles"]) {
        assert!(article["unknown"].is_number(), "{article}");
    }
}

#[tokio::test]
async fn the_simplified_profile_returns_no_article_5_to_15_rows() {
    let srv = spawn_server().await;
    register(&srv).await;

    let (status, body) = get(&srv.base, "/api/dora").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["lei"], LEI);
    assert_eq!(body["profile"], "simplified");
    let listed = article_numbers(&body);
    assert!(listed.contains(&16), "{listed:?}");
    assert!(listed.iter().all(|n| !(5..=15).contains(n)), "{listed:?}");

    // The same lake scored as a full-framework entity.
    let (status, full) = get(&srv.base, "/api/dora?profile=full").await;
    assert_eq!(status, 200, "{full}");
    assert_eq!(full["profile"], "full");
    assert!(article_numbers(&full).contains(&11));

    let (status, simplified) = get(&srv.base, "/api/dora?profile=simplified").await;
    assert_eq!(status, 200);
    assert_eq!(article_numbers(&simplified), listed);
}

#[tokio::test]
async fn dora_lists_the_functions_and_their_cells() {
    let srv = spawn_server().await;
    register(&srv).await;
    let (_, body) = get(&srv.base, &format!("/api/dora?lei={LEI}")).await;
    let ids: Vec<&str> = list(&body["functions"])
        .iter()
        .filter_map(|f| f["entity_id"].as_str())
        .collect();
    assert_eq!(ids, ["cif-0001", "cif-0002"]);
    assert_eq!(body["functions"][0]["register_codes"][0], "F1");
    assert_eq!(body["readiness"]["functions"], 2);
    assert_eq!(body["readiness"]["with_rto"], 1);
    assert_eq!(body["readiness"]["register"]["connected"], true);
    for cell in list(&body["cells"]) {
        assert!(cell["unknown"].is_number(), "{cell}");
        assert!(cell["state"].is_string(), "{cell}");
    }
    // The incident feed is not declared: Art. 19 is unknown, never zero.
    let art19 = list(&body["cells"])
        .iter()
        .find(|c| c["article"] == "19" && c["entity_id"] == "cif-0001")
        .unwrap();
    assert_eq!(art19["state"], "unknown");
    assert_eq!(art19["coverage"], Value::Null);

    let (status, by_org) = get(&srv.base, "/api/dora?by=org").await;
    assert_eq!(status, 200, "{by_org}");
    let rows: Vec<&str> = list(&by_org["rows"])
        .iter()
        .filter_map(|r| r["entity_id"].as_str())
        .collect();
    assert_eq!(rows, ["org:", "org:platform"]);
}

#[tokio::test]
async fn bad_dora_parameters_are_client_errors() {
    let srv = spawn_server().await;
    register(&srv).await;
    for path in [
        "/api/dora?profile=bogus",
        "/api/dora?lei=NOT-A-LEI",
        "/api/dora?by=node",
        "/api/maturity?by=env",
        "/api/maturity?profile=bogus",
    ] {
        let (status, body) = get(&srv.base, path).await;
        assert_eq!(status, 400, "{path}: {body}");
        assert!(body["error"].is_string(), "{path}: {body}");
    }
}

#[tokio::test]
async fn the_article_document_drills_into_one_function() {
    let srv = spawn_server().await;
    register(&srv).await;
    let (status, body) = get(&srv.base, "/api/dora/24?entity=cif-0001").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["article"], "24");
    assert_eq!(body["entity"], "cif-0001");
    let rows = list(&body["requirements"]);
    assert!(!rows.is_empty());
    for row in rows {
        let entities = list(&row["entities"]);
        assert!(entities.len() <= 1, "{row}");
        for entity in entities {
            assert!(entity["evidence"].is_array(), "{entity}");
        }
    }
    let (status, _) = get(&srv.base, "/api/dora/99").await;
    assert_eq!(status, 404);
    let (status, _) = get(&srv.base, "/api/dora/24?entity=cif-9999").await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn maturity_names_the_scale_internal_and_lists_nodes_by_kind() {
    let srv = spawn_server().await;
    register(&srv).await;
    let (status, body) = get(&srv.base, "/api/maturity").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["scale"]["internal"], true);
    assert!((1..=5).contains(&body["level"].as_u64().unwrap()));
    assert_eq!(body["target"], 3);
    assert!(body["headline"]
        .as_str()
        .unwrap()
        .ends_with("of target 3 (simplified)"));
    assert!(body["unknown_share"].is_number());
    assert_eq!(list(&body["gates"]).len(), 4);
    assert!(list(&body["nodes"]).is_empty());
    let history = list(&body["history"]);
    assert_eq!(history.last().unwrap()["level"], body["level"]);

    let kinds = |body: &Value| -> Vec<String> {
        let mut kinds: Vec<String> = list(&body["nodes"])
            .iter()
            .map(|n| n["kind"].as_str().unwrap().to_string())
            .collect();
        kinds.sort();
        kinds.dedup();
        kinds
    };
    let (_, by_entity) = get(&srv.base, "/api/maturity?by=entity").await;
    assert_eq!(kinds(&by_entity), ["entity"]);
    let ids: Vec<&str> = list(&by_entity["nodes"])
        .iter()
        .filter_map(|n| n["id"].as_str())
        .collect();
    assert_eq!(ids, ["ent:cif-0001", "ent:cif-0002"]);
    let (_, by_org) = get(&srv.base, "/api/maturity?by=org").await;
    assert_eq!(kinds(&by_org), ["org", "organisation"]);
    let (_, by_node) = get(&srv.base, "/api/maturity?by=node").await;
    assert!(kinds(&by_node).contains(&"service".to_string()));
    for node in list(&by_node["nodes"]) {
        assert!(node["cap"]["rule"].is_string(), "{node}");
        assert!(node["unknown_share"].is_number(), "{node}");
    }
}

/// The declared service `ledger-archive` of the critical function sends
/// nothing: it is listed as unknown at the floor and lowers every
/// ancestor instead of being skipped. Once it reports with the spine it
/// is known, and the organisation's index rises.
#[tokio::test]
async fn an_uninstrumented_critical_service_counts_at_the_floor_until_it_reports() {
    let srv = spawn_server().await;
    register(&srv).await;
    let now = now_ns();
    insert_spans(
        &srv,
        vec![spine_span("payments-api", "cif-0001", now - DAY)],
    )
    .await;
    let node = |body: &Value, id: &str| -> Value {
        list(&body["nodes"])
            .iter()
            .find(|n| n["id"] == id)
            .unwrap_or_else(|| panic!("no node {id} in {body}"))
            .clone()
    };

    let (_, silent) = get(&srv.base, "/api/maturity?by=node").await;
    let archive = node(&silent, "svc:ledger-archive");
    assert_eq!(archive["unknown"], true, "{archive}");
    assert_eq!(archive["critical"], true, "{archive}");
    assert_eq!(archive["level"], 1);
    assert_eq!(archive["index"], 1.0);
    assert_eq!(archive["parent"], "ent:cif-0001");
    let root = &silent["rollup"];
    assert_eq!(root["id"], "org:");
    assert_eq!(root["level"], 1, "{root}");
    assert!(root["unknown_share"].as_f64().unwrap() > 0.0, "{root}");

    insert_spans(
        &srv,
        vec![spine_span("ledger-archive", "cif-0001", now - DAY / 2)],
    )
    .await;
    let (_, reporting) = get(&srv.base, "/api/maturity?by=node").await;
    let archive = node(&reporting, "svc:ledger-archive");
    assert_eq!(archive["unknown"], false, "{archive}");
    assert!(archive["index"].as_f64().unwrap() > 1.0, "{archive}");
    let index = |body: &Value| body["rollup"]["index"].as_f64().unwrap();
    assert!(index(&reporting) > index(&silent));
    let unknown = |body: &Value| body["rollup"]["unknown_share"].as_f64().unwrap();
    assert!(unknown(&reporting) < unknown(&silent));
}

#[tokio::test]
async fn a_scoped_principal_cannot_score_another_environment() {
    let srv = spawn_server().await;
    register(&srv).await;
    let token = add_scoped_token(&srv, "dora-viewer", "viewer", &["demo"]).await;
    for path in ["/api/dora", "/api/dora/24", "/api/maturity"] {
        let (status, body) = get_auth(&srv.base, &format!("{path}?env=prod"), &token).await;
        assert_eq!(status, 403, "{path}: {body}");
        let (status, body) = get_auth(&srv.base, &format!("{path}?env=demo"), &token).await;
        assert_eq!(status, 200, "{path}: {body}");
    }
}

/// Malformed `ict.entity.id` values are listed by the conformance report
/// exactly as they were received; canonical ids are not.
#[tokio::test]
async fn conformance_lists_malformed_entity_ids_as_received() {
    let srv = spawn_server().await;
    let now = now_ns();
    let (_, clean) = get(&srv.base, "/api/otel/conformance?range=24h").await;
    assert_eq!(clean["malformed_entity_ids"], serde_json::json!([]));
    assert_eq!(clean["summary"]["malformed_entity_ids"], 0);

    let mut flagged = spine_span("legacy-batch", "CF-0042", now - DAY / 4);
    flagged
        .resource_attrs
        .push(("tumult.ict.entity.id.malformed".into(), "true".into()));
    // Stored before the ingest check existed: no flag, still listed.
    let unflagged = spine_span("old-gateway", "Core Banking", now - DAY / 3);
    insert_spans(
        &srv,
        vec![
            spine_span("payments-api", "cif-0001", now - DAY / 2),
            flagged,
            unflagged,
        ],
    )
    .await;

    let (status, body) = get(&srv.base, "/api/otel/conformance?range=24h").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["entity_id_pattern"], "^[a-z][a-z0-9]*(-[a-z0-9]+)+$");
    assert_eq!(body["summary"]["malformed_entity_ids"], 2);
    let listed = list(&body["malformed_entity_ids"]);
    assert_eq!(listed.len(), 2, "{body}");
    assert_eq!(listed[0]["service"], "legacy-batch");
    assert_eq!(listed[0]["entity_id"], "CF-0042");
    assert_eq!(listed[0]["spans"], 1);
    assert_eq!(listed[0]["flagged_at_ingest"], true);
    assert!(listed[0]["reason"].is_string());
    assert_eq!(listed[1]["service"], "old-gateway");
    assert_eq!(listed[1]["entity_id"], "Core Banking");
    assert_eq!(listed[1]["flagged_at_ingest"], false);
}
