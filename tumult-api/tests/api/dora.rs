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
pub(crate) const REGISTER: &str = "
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

pub(crate) async fn register(srv: &TestServer) {
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
    // Declared in the org file, and nothing of it ingested: not connected.
    assert_eq!(body["readiness"]["register"]["connected"], false);
    let feed = list(&body["feeds"])
        .iter()
        .find(|f| f["name"] == "register")
        .unwrap_or_else(|| panic!("no register feed in {body}"));
    assert_eq!(feed["declared"], true);
    assert_eq!(feed["connected"], false);
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
    let headline = body["headline"].as_str().unwrap();
    assert!(headline.contains("of target 3 (simplified)"), "{headline}");
    assert!(headline.ends_with("% unknown"), "{headline}");
    assert!(body["unknown_share"].is_number());
    assert_eq!(body["unknown_share"], body["rollup"]["unknown_share"]);
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

/// Every score document says which house parameters it was scored with
/// and where the function register stands, so a lowered gate or a
/// refused org file is never silent.
#[tokio::test]
async fn every_score_document_names_its_parameters_and_the_register_state() {
    let srv = spawn_server().await;
    register(&srv).await;
    for path in ["/api/dora", "/api/dora/24", "/api/maturity"] {
        let (status, body) = get(&srv.base, path).await;
        assert_eq!(status, 200, "{path}: {body}");
        assert_eq!(body["params"]["source"], "defaults", "{path}");
        assert_eq!(body["params"]["fallback"], false, "{path}");
        assert_eq!(body["params"]["reason"], Value::Null, "{path}");
        assert_eq!(body["params"]["overridden"], serde_json::json!([]));
        // The test server has no org file.
        assert_eq!(body["register_state"], "none", "{path}");
        assert!(body["register_reason"].is_string(), "{path}");
    }
}

/// A past quarter is what the daily job stored for it and nothing else.
#[tokio::test]
async fn maturity_history_shows_stored_days_and_no_data_for_the_rest() {
    let srv = spawn_server().await;
    register(&srv).await;
    let (_, body) = get(&srv.base, "/api/maturity").await;
    let history = list(&body["history"]);
    assert_eq!(history.len(), 5, "{body}");
    for past in &history[..4] {
        assert_eq!(past["state"], "no_data", "{past}");
        assert_eq!(past["level"], Value::Null, "{past}");
        assert_eq!(past["index"], Value::Null, "{past}");
    }
    assert_eq!(history[4]["state"], "today");

    // The job stored the last day of the previous quarter.
    let ends = tumult_compliance::dora::report::quarter_ends(now_ns(), 4);
    let day = ends.last().unwrap().div_euclid(DAY);
    let row = tumult_lake::MaturityDailyRow {
        as_of_day: day,
        lei: LEI.into(),
        level: 2,
        index: 2.4,
        unknown_share: 0.25,
        source: "snapshot".into(),
    };
    let at = now_ns();
    exec_write(&srv, move |w| {
        w.upsert_maturity_daily(&[row], at)
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
    .await;
    let (_, body) = get(&srv.base, "/api/maturity").await;
    let history = list(&body["history"]);
    assert_eq!(history[3]["state"], "stored", "{body}");
    assert_eq!(history[3]["level"], 2);
    assert_eq!(history[3]["index"], 2.4);
    assert_eq!(history[3]["unknown_share"], 0.25);
    assert_eq!(history[2]["state"], "no_data");

    // Stored days are of the registered profile: another one has none.
    let (_, full) = get(&srv.base, "/api/maturity?profile=full").await;
    assert_eq!(full["profile_overridden"], true);
    let past = &list(&full["history"])[3];
    assert_eq!(past["state"], "no_data", "{past}");
    assert!(past["reason"].is_string(), "{past}");
    assert_eq!(body["profile_overridden"], false);
}

/// A principal limited to some environments gets the functions its
/// telemetry binds to, a document marked partial, and nothing of the
/// register beyond that.
#[tokio::test]
async fn a_scoped_token_gets_only_its_functions_and_a_document_marked_partial() {
    let srv = spawn_server().await;
    register(&srv).await;
    insert_spans(
        &srv,
        vec![spine_span("payments-api", "cif-0001", now_ns() - DAY)],
    )
    .await;
    let ids = |body: &Value| -> Vec<String> {
        list(&body["functions"])
            .iter()
            .filter_map(|f| f["entity_id"].as_str().map(str::to_owned))
            .collect()
    };
    let (_, open) = get(&srv.base, "/api/dora").await;
    assert_eq!(ids(&open), ["cif-0001", "cif-0002"]);
    assert_eq!(open["scoped"], false);
    assert_eq!(open["scope_note"], Value::Null);
    assert_eq!(open["readiness"]["register"]["rows"], 2);

    let demo = add_scoped_token(&srv, "demo-viewer", "viewer", &["demo"]).await;
    let (status, scoped) = get_auth(&srv.base, "/api/dora", &demo).await;
    assert_eq!(status, 200, "{scoped}");
    assert_eq!(ids(&scoped), ["cif-0001"]);
    assert_eq!(scoped["scoped"], true);
    assert!(scoped["scope_note"].is_string());
    assert_eq!(scoped["readiness"]["register"]["rows"], 0, "{scoped}");
    let (_, level) = get_auth(&srv.base, "/api/maturity", &demo).await;
    assert_eq!(level["scoped"], true);
    assert!(list(&level["history"])[..4]
        .iter()
        .all(|past| past["state"] == "no_data"));

    let other = add_scoped_token(&srv, "other-viewer", "viewer", &["elsewhere"]).await;
    let (status, none) = get_auth(&srv.base, "/api/dora", &other).await;
    assert_eq!(status, 200, "{none}");
    assert_eq!(ids(&none), Vec::<String>::new());
    assert_eq!(none["scoped"], true);
}

/// A scoped caller that names a legal entity it has no telemetry of
/// gets what an unregistered LEI gets: no name, no profile, no feeds.
#[tokio::test]
async fn a_scoped_token_cannot_probe_the_register_by_lei() {
    const OTHER: &str = "506700GE1G29325QX363";
    const NOWHERE: &str = "5493001KJTIIGC8Y1R12";
    let two = REGISTER.replace(
        "functions:",
        "  - {lei: 506700GE1G29325QX363, name: Hidden Bank, profile: full, feeds: [incident]}\n\
         functions:",
    ) + "  - {id: cif-0003, lei: 506700GE1G29325QX363, name: Custody, tier: critical}\n";
    let two = two
        .replace(
            "tier: critical\n",
            &format!("tier: critical\n    lei: {LEI}\n"),
        )
        .replace(
            "tier: important\n",
            &format!("tier: important\n    lei: {LEI}\n"),
        );
    let srv = spawn_server().await;
    let declared = tumult_compliance::dora::register::parse(&two).unwrap();
    assert_eq!(declared.ict.legal_entities.len(), 2);
    let at = now_ns();
    exec_write(&srv, move |w| {
        w.sync_ict_register(&declared.ict, at)
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
    .await;
    insert_spans(
        &srv,
        vec![spine_span("payments-api", "cif-0001", now_ns() - DAY)],
    )
    .await;
    let (status, open) = get(&srv.base, &format!("/api/dora?lei={OTHER}")).await;
    assert_eq!(status, 200, "{open}");
    assert_eq!(open["entity_name"], "Hidden Bank");

    let demo = add_scoped_token(&srv, "demo-viewer", "viewer", &["demo"]).await;
    for path in ["/api/dora", "/api/dora/24", "/api/maturity"] {
        for extra in ["", "&profile=full", "&profile=no-such"] {
            let ask = |lei: &str| format!("{path}?lei={lei}{extra}");
            let (hidden_status, hidden) = get_auth(&srv.base, &ask(OTHER), &demo).await;
            let (unknown_status, unknown) = get_auth(&srv.base, &ask(NOWHERE), &demo).await;
            assert_eq!(hidden_status, 400, "{path}{extra}: {hidden}");
            assert_eq!(unknown_status, 400, "{path}{extra}: {unknown}");
            let hidden = hidden.to_string();
            assert_eq!(
                hidden.replace(OTHER, "x"),
                unknown.to_string().replace(NOWHERE, "x"),
                "{path}{extra}"
            );
            for secret in ["Hidden Bank", "feeds", "incident", "profile_label"] {
                assert!(!hidden.contains(secret), "{path}{extra}: {hidden}");
            }
        }
    }
    // Its own entity still answers by LEI.
    let (status, own) = get_auth(&srv.base, &format!("/api/dora?lei={LEI}"), &demo).await;
    assert_eq!(status, 200, "{own}");
    assert_eq!(own["lei"], LEI);
}

/// A day the proof tool stored for a past date is no stored day on a
/// daemon without the proof opt-in, and the response says it was left
/// out.
#[tokio::test]
async fn proof_days_are_left_out_and_said_so_without_the_opt_in() {
    let srv = spawn_server().await;
    register(&srv).await;
    let ends = tumult_compliance::dora::report::quarter_ends(now_ns(), 4);
    let day = ends.last().unwrap().div_euclid(DAY);
    let today = now_ns().div_euclid(DAY);
    let level = tumult_lake::MaturityDailyRow {
        as_of_day: day,
        lei: LEI.into(),
        level: 5,
        index: 4.9,
        unknown_share: 0.0,
        source: "proof".into(),
    };
    let status = |day: i64| tumult_lake::RequirementStatusRow {
        as_of_day: day,
        function_id: "cif-0001".into(),
        requirement_id: "dora.art24.1".into(),
        status: "existing".into(),
        value: Some(1.0),
        source: "proof".into(),
    };
    let rows = vec![status(today - 1), status(today - 2)];
    let at = now_ns();
    exec_write(&srv, move |w| {
        w.upsert_maturity_daily(&[level], at)
            .and_then(|_| w.upsert_requirement_status(&rows, at))
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
    .await;
    let (_, body) = get(&srv.base, "/api/maturity").await;
    let history = list(&body["history"]);
    assert_eq!(history[3]["state"], "no_data", "{body}");
    assert_eq!(history[3]["level"], Value::Null);
    assert_eq!(
        body["proof_days"],
        serde_json::json!({"enabled": false, "counted": 0, "ignored": 2, "note": null})
    );
    for path in ["/api/dora", "/api/dora/24"] {
        let (_, body) = get(&srv.base, path).await;
        assert_eq!(body["proof_days"]["enabled"], false, "{path}");
        assert_eq!(body["proof_days"]["ignored"], 2, "{path}");
    }
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

/// The list of malformed ids is bounded: a stored id of any length is
/// reported cut and marked, and more rows than the limit are cut and
/// the document says so.
#[tokio::test]
async fn conformance_bounds_the_malformed_ids_in_length_and_in_number() {
    let srv = spawn_server().await;
    let now = now_ns();
    let long = "X".repeat(5_000);
    insert_spans(&srv, vec![spine_span("old-gateway", &long, now - DAY / 3)]).await;
    let (status, body) = get(&srv.base, "/api/otel/conformance?range=24h").await;
    assert_eq!(status, 200, "{body}");
    let listed = list(&body["malformed_entity_ids"]);
    assert_eq!(listed.len(), 1, "{body}");
    let shown = listed[0]["entity_id"].as_str().unwrap();
    assert_eq!(
        shown,
        "X".repeat(64),
        "cut to one past the longest valid id"
    );
    assert_eq!(listed[0]["truncated"], true);
    assert_eq!(listed[0]["reason"], "longer than 63 characters");
    assert_eq!(body["malformed_entity_ids_truncated"], false);
    assert_eq!(body["malformed_entity_ids_limit"], 500);

    let many: Vec<_> = (0..501)
        .map(|i| spine_span("noisy", &format!("Bad Id {i:04}"), now - DAY / 5))
        .collect();
    insert_spans(&srv, many).await;
    let (_, body) = get(&srv.base, "/api/otel/conformance?range=24h").await;
    assert_eq!(list(&body["malformed_entity_ids"]).len(), 500, "the limit");
    assert_eq!(body["summary"]["malformed_entity_ids"], 500);
    assert_eq!(body["malformed_entity_ids_truncated"], true);
    assert_eq!(list(&body["malformed_entity_ids"])[0]["truncated"], false);
}
