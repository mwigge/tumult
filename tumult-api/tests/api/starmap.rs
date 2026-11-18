use crate::common::*;
use serde_json::{json, Value};
use tumult_lake::SpanRow;

// ---------------------------------------------------------------------------
// GET /api/starmap: the spine comes from resource attributes, evidence from
// the seeded runs, and a scoped principal never sees another environment.

struct Svc<'a> {
    service: &'a str,
    system: &'a str,
    env: &'a str,
    entity: &'a str,
    owner: Option<&'a str>,
    component: Option<&'a str>,
    parent: Option<&'a str>,
    age_s: i64,
}

impl Default for Svc<'_> {
    fn default() -> Self {
        Self {
            service: "",
            system: "",
            env: "demo",
            entity: "",
            owner: None,
            component: None,
            parent: None,
            age_s: 300,
        }
    }
}

/// One server span of an instrumented service, as the OTLP receiver stores
/// it: the target triple promoted to columns, the ICT spine left in
/// `resource_attrs`.
fn service_span(s: &Svc) -> SpanRow {
    let attr = |k: &str, v: &str| (k.to_string(), v.to_string());
    let mut resource_attrs = vec![
        attr("ict.schema.version", "1"),
        attr("ict.entity.id", s.entity),
        attr("ict.entity.tier", "critical"),
        attr("k8s.namespace.name", s.env),
        attr("k8s.deployment.name", s.service),
    ];
    resource_attrs.extend(s.owner.map(|o| attr("ict.entity.owner", o)));
    resource_attrs.extend(s.component.map(|c| attr("resilience.target.component", c)));
    SpanRow {
        ts_ns: now_ns() - s.age_s * NS,
        trace_id: format!("trace-{}", s.service),
        span_id: format!("span-{}", s.service),
        parent_span_id: s.parent.map(|p| format!("span-{p}")),
        span_name: "GET /".into(),
        span_kind: "Server".into(),
        duration_ns: NS / 100,
        status_code: "Ok".into(),
        status_message: String::new(),
        service_name: s.service.into(),
        service_version: Some("1.4.0".into()),
        experiment_id: None,
        experiment_name: None,
        outcome_status: None,
        fault_type: None,
        fault_subtype: None,
        fault_severity: None,
        blast_radius: None,
        target_system: Some(s.system.into()),
        target_technology: Some("http".into()),
        target_environment: Some(s.env.into()),
        plugin_name: None,
        hypothesis_met: None,
        recovery_time_s: None,
        span_attrs: vec![],
        resource_attrs,
        events: "[]".into(),
    }
}

/// `checkout` (demo, entity CIF-0042 owned by db-team) calls `ledger`
/// (demo); `billing` runs in prod; `archive` was last seen ten days ago.
async fn seed_services(srv: &TestServer) {
    let rows = vec![
        service_span(&Svc {
            service: "checkout",
            system: "shop",
            entity: "CIF-0042",
            owner: Some("db-team"),
            component: Some("api"),
            ..Svc::default()
        }),
        service_span(&Svc {
            service: "ledger",
            system: "ledger",
            entity: "cif-0043",
            parent: Some("checkout"),
            ..Svc::default()
        }),
        service_span(&Svc {
            service: "billing",
            system: "payments",
            env: "prod",
            entity: "cif-0099",
            ..Svc::default()
        }),
        service_span(&Svc {
            service: "archive",
            system: "archive",
            entity: "cif-0050",
            age_s: 10 * 86_400,
            ..Svc::default()
        }),
    ];
    exec_write(srv, move |w| {
        w.insert_spans(&rows).map_err(|e| e.to_string())
    })
    .await;
}

/// The root span of one completed run against `system` in `env`.
fn run_span(id: &str, experiment: &str, system: &str, env: &str) -> SpanRow {
    SpanRow {
        ts_ns: now_ns() - 900 * NS,
        trace_id: format!("trace-{id}"),
        span_id: format!("span-{id}-root"),
        parent_span_id: None,
        span_name: "resilience.experiment".into(),
        span_kind: "Internal".into(),
        duration_ns: 5 * NS,
        status_code: "Unset".into(),
        status_message: String::new(),
        service_name: "tumult".into(),
        service_version: Some("2.22.0".into()),
        experiment_id: Some(id.into()),
        experiment_name: Some(experiment.into()),
        outcome_status: Some("Completed".into()),
        fault_type: None,
        fault_subtype: None,
        fault_severity: None,
        blast_radius: None,
        target_system: Some(system.into()),
        target_technology: Some("http".into()),
        target_environment: Some(env.into()),
        plugin_name: None,
        hypothesis_met: None,
        recovery_time_s: None,
        span_attrs: vec![],
        resource_attrs: vec![],
        events: "[]".into(),
    }
}

/// `shared` is one system name in two environments: a chaos run in demo
/// and a disaster-recovery run in prod.
async fn seed_shared_system(srv: &TestServer) {
    let rows = vec![
        run_span("shared-demo", "shared-chaos", "shared", "demo"),
        run_span("shared-prod", "shared-dr", "shared", "prod"),
    ];
    exec_write(srv, move |w| {
        w.insert_spans(&rows).map_err(|e| e.to_string())?;
        w.execute(
            "INSERT INTO experiments (experiment_id, title, status, started_at_ns, \
             ended_at_ns, duration_ms, method_step_count, rollback_count, test_kind) \
             VALUES ('shared-prod', 'shared-dr', 'completed', 1, 2, 1, 1, 0, 'dr')",
            (),
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    })
    .await;
}

pub(crate) fn node<'a>(body: &'a Value, id: &str) -> Option<&'a Value> {
    body["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == id)
}

fn edge<'a>(body: &'a Value, id: &str) -> Option<&'a Value> {
    body["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == id)
}

fn ids(body: &Value) -> Vec<String> {
    body["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["id"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn starmap_builds_the_spine_from_resource_attributes() {
    let srv = spawn_server().await;
    seed_services(&srv).await;
    let (status, body) = get(&srv.base, "/api/starmap").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["range"], "7d");

    // Entity ids compare without regard to case, so the node id is the
    // lower-case form; the label is the id as reported, and one that
    // does not have the agreed form is flagged. The owner comes from
    // telemetry.
    assert!(node(&body, "ent:CIF-0042").is_none(), "one node per id");
    let entity = node(&body, "ent:cif-0042").expect("entity node");
    assert_eq!(entity["kind"], "entity");
    assert_eq!(entity["label"], "CIF-0042");
    assert_eq!(entity["flags"], json!(["malformed_id"]));
    assert_eq!(entity["tier"], "critical");
    assert_eq!(entity["parent"], "org:data/db-team");
    let owns = edge(&body, "org:data/db-team|owns|ent:cif-0042").expect("owns edge");
    assert_eq!(owns["source"], "telemetry");
    assert_eq!(node(&body, "ent:cif-0043").unwrap()["flags"], json!([]));
    assert_eq!(entity["rollup"]["systems"], 1);
    assert_eq!(entity["rollup"]["untested_systems"], 1);

    // The system is keyed on the promoted target column and environment.
    let system = node(&body, "sys:demo/shop").expect("system node");
    assert_eq!(system["kind"], "system");
    assert_eq!(system["env"], "demo");
    assert_eq!(system["parent"], "ent:cif-0042");
    assert_eq!(system["gap"], "missing", "nothing ever tested it");
    let component = node(&body, "cmp:demo/shop/api").expect("component node");
    assert_eq!(component["parent"], "sys:demo/shop");
    let service = node(&body, "svc:checkout").expect("service node");
    assert_eq!(service["parent"], "cmp:demo/shop/api");
    assert_eq!(service["conformance"], "spine");
    assert!(edge(&body, "svc:checkout|implements|ent:cif-0042").is_some());
    assert!(edge(&body, "ent:cif-0042|implements|sys:demo/shop").is_some());

    // Observed calls keep their traffic numbers.
    let calls = edge(&body, "svc:checkout|calls|svc:ledger").expect("calls edge");
    assert_eq!(calls["source"], "observed");
    assert_eq!(calls["weight"], 1);
    assert_eq!(calls["errors"], 0);

    // The seeded runs are evidence on the tested system and its definitions.
    let tested = node(&body, "sys:demo/database").expect("tested system");
    assert_eq!(tested["evidence"]["runs"], 2);
    assert_eq!(tested["evidence"]["failed"], 1);
    assert_eq!(tested["parent"], "org:data/db-team", "pg-* assignment");
    // A definition known only by name can never take a registry id.
    assert_eq!(node(&body, "def:name/pg-failover").unwrap()["kind"], "def");
    assert!(edge(&body, "sys:demo/database|tested_by|def:name/pg-failover").is_some());
    let broke = edge(&body, "def:name/cache-stampede|broke|sys:demo/database").expect("broke edge");
    assert_eq!(broke["source"], "evidence");
    assert!(
        edge(&body, "def:name/pg-failover|broke|sys:demo/database").is_none(),
        "a passing run breaks nothing"
    );
    // The org unit counts what hangs below it.
    let team = node(&body, "org:data/db-team").expect("team node");
    assert_eq!(team["rollup"]["systems"], 2, "{team}");
    assert_eq!(team["rollup"]["untested_systems"], 1, "{team}");
    assert_eq!(team["rollup"]["failed"], 1, "{team}");
    assert!(body["counts"]["risks_unplaced"].is_u64(), "{body}");

    // The runner does not implement the system it tests, and the old
    // plugin-path target ids are gone.
    assert_eq!(
        node(&body, "svc:tumult").unwrap()["parent"],
        "org:(unassigned)"
    );
    let all = ids(&body);
    assert!(all.iter().all(|id| !id.starts_with("tgt:")), "{all:?}");
    assert!(all.iter().all(|id| !id.starts_with("run:")), "{all:?}");
}

#[tokio::test]
async fn starmap_reports_unavailable_sources_as_null_blocks() {
    let srv = spawn_server().await;
    seed_services(&srv).await;
    let (status, body) = get(&srv.base, "/api/starmap").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["sources"]["slo"], "unavailable");
    assert_eq!(body["sources"]["dora"], "unavailable");
    assert_eq!(body["sources"]["maturity"], "unavailable");
    assert_eq!(body["sources"]["graph"], "ok");
    assert!(body["maturity"].is_null());
    let nodes = body["nodes"].as_array().unwrap();
    assert!(!nodes.is_empty());
    for n in nodes {
        let object = n.as_object().unwrap();
        for block in ["slo", "dora", "maturity"] {
            assert!(object.contains_key(block), "{block} missing on {n}");
            assert!(n[block].is_null(), "{block} on {n}");
        }
    }
    assert!(body["disclaimer"].as_str().is_some_and(|d| !d.is_empty()));
}

#[tokio::test]
async fn starmap_stays_under_the_node_budget() {
    let srv = spawn_server().await;
    seed_services(&srv).await;
    let (_, full) = get(&srv.base, "/api/starmap").await;
    let count = full["nodes"].as_array().unwrap().len();
    assert_eq!(full["counts"]["nodes"], count);
    assert_eq!(
        full["counts"]["edges"],
        full["edges"].as_array().unwrap().len()
    );
    assert!(count <= 1500);
    assert_eq!(full["counts"]["clustered"], 0);

    let (status, small) = get(&srv.base, "/api/starmap?budget=8").await;
    assert_eq!(status, 200, "{small}");
    let nodes = small["nodes"].as_array().unwrap();
    assert!(nodes.len() < count, "{} of {count}", nodes.len());
    assert!(small["counts"]["clustered"].as_u64().unwrap() > 0);
    assert!(nodes.iter().any(|n| n["kind"] == "cluster"));
    assert!(!small["clusters"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn starmap_prunes_by_range() {
    let srv = spawn_server().await;
    seed_services(&srv).await;
    let (_, week) = get(&srv.base, "/api/starmap?range=7d").await;
    assert!(node(&week, "svc:archive").is_none());
    assert!(node(&week, "svc:checkout").is_some());
    for range in ["14d", "30d"] {
        let (status, body) = get(&srv.base, &format!("/api/starmap?range={range}")).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["range"], range);
        assert!(node(&body, "svc:archive").is_some(), "{range}");
    }
    let (status, body) = get(&srv.base, "/api/starmap?range=1y").await;
    assert_eq!(status, 400, "{body}");
}

#[tokio::test]
async fn starmap_validates_its_parameters() {
    let srv = spawn_server().await;
    seed_services(&srv).await;
    for bad in [
        "focus=svc:checkout&depth=0",
        "focus=svc:checkout&depth=5",
        "kinds=service,planet",
        "budget=0",
        "budget=5001",
        "include=everything",
    ] {
        let (status, body) = get(&srv.base, &format!("/api/starmap?{bad}")).await;
        assert_eq!(status, 400, "{bad}: {body}");
        assert!(body["error"].is_string(), "{bad}: {body}");
    }

    let (status, body) = get(&srv.base, "/api/starmap?kinds=service,system").await;
    assert_eq!(status, 200, "{body}");
    let nodes = body["nodes"].as_array().unwrap();
    assert!(!nodes.is_empty());
    assert!(nodes
        .iter()
        .all(|n| n["kind"] == "service" || n["kind"] == "system"));

    let (status, body) = get(&srv.base, "/api/starmap?include=runs").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(node(&body, "run:exp-fail").unwrap()["kind"], "run");
    assert!(edge(&body, "run:exp-fail|broke|sys:demo/database").is_some());

    // Focus keeps the neighbourhood and the spine above it.
    let (status, body) = get(&srv.base, "/api/starmap?focus=svc:ledger&depth=1").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["focus"], "svc:ledger");
    assert_eq!(body["depth"], 1);
    assert!(node(&body, "svc:checkout").is_some());
    assert!(node(&body, "sys:demo/database").is_none());
    assert!(body["counts"]["pruned"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn starmap_refuses_an_environment_outside_the_scopes() {
    let srv = spawn_server().await;
    seed_services(&srv).await;
    let token = add_scoped_token(&srv, "demo-viewer", "viewer", &["demo"]).await;
    let (status, body) = get_auth(&srv.base, "/api/starmap?env=prod", &token).await;
    assert_eq!(status, 403, "{body}");
    let (status, body) = get_auth(&srv.base, "/api/starmap?env=demo", &token).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["env"], "demo");
}

#[tokio::test]
async fn scoped_principal_never_sees_another_environment() {
    let srv = spawn_server().await;
    seed_services(&srv).await;
    let token = add_scoped_token(&srv, "demo-only", "viewer", &["demo"]).await;
    let admin = add_scoped_token(&srv, "everything", "viewer", &[]).await;

    let (_, open) = get_auth(&srv.base, "/api/starmap", &admin).await;
    assert!(node(&open, "svc:billing").is_some(), "unscoped sees prod");
    assert!(node(&open, "sys:prod/payments").is_some());

    let leaked = |body: &Value| -> Vec<String> {
        let text = |v: &Value| v.to_string();
        let mut found: Vec<String> = body["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|n| n["env"] == "prod" || n["env"].is_null() && n["kind"] == "service")
            .map(text)
            .collect();
        let raw = body.to_string();
        for needle in ["billing", "payments", "cif-0099", "\"prod\"", "prod/"] {
            if raw.contains(needle) {
                found.push(needle.to_string());
            }
        }
        found
    };

    let (status, scoped) = get_auth(&srv.base, "/api/starmap", &token).await;
    assert_eq!(status, 200, "{scoped}");
    assert!(node(&scoped, "svc:checkout").is_some());
    assert!(node(&scoped, "sys:demo/database").is_some());
    assert_eq!(leaked(&scoped), Vec::<String>::new());

    // Focusing on a node of another environment reveals nothing, at any
    // depth and with runs included.
    for focus in ["svc:billing", "sys:prod/payments", "ent:cif-0099"] {
        let path = format!("/api/starmap?focus={focus}&depth=4&include=runs");
        let (status, body) = get_auth(&srv.base, &path, &token).await;
        assert_eq!(status, 200, "{body}");
        assert!(body["nodes"].as_array().unwrap().is_empty(), "{focus}");
        assert!(body["edges"].as_array().unwrap().is_empty(), "{focus}");
    }
    let (_, around) = get_auth(
        &srv.base,
        "/api/starmap?focus=org:&depth=4&include=runs",
        &token,
    )
    .await;
    assert!(node(&around, "svc:checkout").is_some());
    assert_eq!(leaked(&around), Vec::<String>::new());
}

#[tokio::test]
async fn a_run_that_was_aborted_or_interrupted_is_neither_a_run_nor_a_failure_of_its_system() {
    let srv = spawn_server().await;
    let ended = |id: &str, status: &str, age_s: i64| SpanRow {
        ts_ns: now_ns() - age_s * NS,
        outcome_status: Some(status.into()),
        ..run_span(id, "gate-check", "gate", "demo")
    };
    let rows = vec![
        ended("gate-1", "completed", 900),
        ended("gate-2", "aborted", 600),
        ended("gate-3", "Interrupted", 300),
    ];
    exec_write(&srv, move |w| {
        w.insert_spans(&rows).map_err(|e| e.to_string())
    })
    .await;

    let (status, body) = get(&srv.base, "/api/starmap").await;

    assert_eq!(status, 200, "{body}");
    let gate = node(&body, "sys:demo/gate").expect("tested system");
    assert_eq!(
        (&gate["evidence"]["runs"], &gate["evidence"]["failed"]),
        (&json!(1), &json!(0)),
        "{gate}"
    );
    assert!(edge(&body, "sys:demo/gate|tested_by|def:name/gate-check").is_some());
    assert!(
        edge(&body, "def:name/gate-check|broke|sys:demo/gate").is_none(),
        "{body}"
    );
}

#[tokio::test]
async fn one_system_name_in_two_environments_keeps_its_own_coverage() {
    let srv = spawn_server().await;
    seed_shared_system(&srv).await;
    let (status, body) = get(&srv.base, "/api/starmap").await;
    assert_eq!(status, 200, "{body}");
    let demo = node(&body, "sys:demo/shared").expect("demo system");
    let prod = node(&body, "sys:prod/shared").expect("prod system");
    let discipline = |n: &Value, kind: &str| n["evidence"]["by_discipline"][kind].clone();
    assert_eq!(discipline(demo, "chaos"), "covered", "{demo}");
    assert_eq!(discipline(demo, "robustness"), "missing", "{demo}");
    assert_eq!(discipline(prod, "robustness"), "covered", "{prod}");
    assert_eq!(discipline(prod, "chaos"), "missing", "{prod}");
    assert!(demo["gap"].is_null() && prod["gap"].is_null());
    assert_eq!(demo["evidence"]["runs"], 1);
    assert_eq!(prod["evidence"]["runs"], 1);
    for (system, own, other) in [
        ("sys:demo/shared", "shared-chaos", "shared-dr"),
        ("sys:prod/shared", "shared-dr", "shared-chaos"),
    ] {
        let tested = |name: &str| edge(&body, &format!("{system}|tested_by|def:name/{name}"));
        assert!(tested(own).is_some(), "{system} is tested by {own}");
        assert!(tested(other).is_none(), "{system} was never run by {other}");
    }

    // The risk scan and the posture keep one row per environment too.
    let envs_of = |rows: &Value| -> Vec<String> {
        let mut envs: Vec<String> = rows
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t["target"] == "shared")
            .map(|t| t["environment"].as_str().unwrap().to_string())
            .collect();
        envs.sort();
        envs
    };
    let (_, risks) = get(&srv.base, "/api/risks").await;
    assert_eq!(envs_of(&risks["coverage"]["targets"]), ["demo", "prod"]);
    let (_, posture) = get(&srv.base, "/api/posture").await;
    assert_eq!(envs_of(&posture["posture"]["targets"]), ["demo", "prod"]);
    let (_, scoped) = get(&srv.base, "/api/posture?env=prod").await;
    assert_eq!(envs_of(&scoped["posture"]["targets"]), ["prod"]);
}

/// `shop-team` claims every `CIF-*` entity, written the way people write
/// it; telemetry says `db-team` owns CIF-0042.
const ENTITY_ORG_YAML: &str = "
nodes:
  - {name: data, kind: domain}
  - {name: db-team, parent: data}
  - {name: shop-team, parent: data}
assignments:
  - team: db-team
    targets: [\"pg-*\"]
  - team: shop-team
    entities: [\"CIF-*\"]
";

#[tokio::test]
async fn org_patterns_assign_entities_whatever_their_case() {
    let srv = spawn_server_with_org(ENTITY_ORG_YAML).await;
    seed_services(&srv).await;
    let (status, body) = get(&srv.base, "/api/starmap").await;
    assert_eq!(status, 200, "{body}");

    // One matcher reads org.yaml: an upper-case pattern claims a
    // lower-case id, exactly as the scorecard's org tree does.
    let ledger = node(&body, "ent:cif-0043").expect("entity node");
    assert_eq!(ledger["parent"], "org:data/shop-team", "{ledger}");
    let owns = edge(&body, "org:data/shop-team|owns|ent:cif-0043").expect("owns edge");
    assert_eq!(owns["source"], "declared");
    assert_eq!(ledger["flags"], json!([]));

    // Telemetry still wins, and the disagreement is flagged.
    let shop = node(&body, "ent:cif-0042").expect("entity node");
    assert_eq!(shop["label"], "CIF-0042");
    assert_eq!(shop["parent"], "org:data/db-team", "{shop}");
    assert_eq!(shop["flags"], json!(["malformed_id", "owner_conflict"]));
}

#[tokio::test]
async fn starmap_answers_from_a_short_cache_with_an_etag() {
    let srv = spawn_server().await;
    seed_services(&srv).await;
    let (status, headers, first) = get_with(&srv.base, "/api/starmap", &[]).await;
    assert_eq!(status, 200, "{first}");
    let etag = headers["etag"].to_str().unwrap().to_string();
    assert!(etag.starts_with('"') && etag.ends_with('"'), "{etag}");
    assert_eq!(headers["cache-control"], "private, no-cache");

    // Inside the 30 s window the same request is answered from the cache:
    // byte-identical, the assembly time included.
    let (_, again_headers, again) = get_with(&srv.base, "/api/starmap", &[]).await;
    assert_eq!(again, first);
    assert_eq!(again_headers["etag"].to_str().unwrap(), etag);

    // A matching validator gets 304 and no body.
    let (status, headers, body) =
        get_with(&srv.base, "/api/starmap", &[("If-None-Match", &etag)]).await;
    assert_eq!(status, 304);
    assert_eq!(headers["etag"].to_str().unwrap(), etag);
    assert!(body.is_empty(), "{body}");
    let (status, _, _) =
        get_with(&srv.base, "/api/starmap", &[("If-None-Match", "\"other\"")]).await;
    assert_eq!(status, 200);

    // Another parameter set is another entry.
    let (_, _, small) = get_with(&srv.base, "/api/starmap?budget=8", &[]).await;
    assert_ne!(small, first);

    // `Cache-Control: no-cache` cannot force a lake read on every
    // request: a response younger than two seconds is good enough.
    let refresh = [("Cache-Control", "no-cache")];
    let (status, _, at_once) = get_with(&srv.base, "/api/starmap", &refresh).await;
    assert_eq!(status, 200);
    assert_eq!(at_once, first);

    // Past that floor it reads the lake again, and the result is what
    // plain requests are then served.
    srv.starmap_ahead_ms
        .store(2_100, std::sync::atomic::Ordering::SeqCst);
    let (status, _, fresh) = get_with(&srv.base, "/api/starmap", &refresh).await;
    assert_eq!(status, 200);
    let as_of = |raw: &str| serde_json::from_str::<Value>(raw).unwrap()["as_of_ns"].clone();
    assert_ne!(as_of(&fresh), as_of(&first));
    let (_, _, plain) = get_with(&srv.base, "/api/starmap", &[]).await;
    assert_eq!(plain, fresh);
}

#[tokio::test]
async fn the_cache_never_serves_one_principal_the_map_of_another() {
    let srv = spawn_server().await;
    seed_services(&srv).await;
    let admin = add_scoped_token(&srv, "everything", "viewer", &[]).await;
    let demo = add_scoped_token(&srv, "demo-only", "viewer", &["demo"]).await;
    let prod = add_scoped_token(&srv, "prod-only", "viewer", &["prod"]).await;

    // The unscoped principal fills the cache first, for the same URL.
    let (_, open) = get_auth(&srv.base, "/api/starmap", &admin).await;
    assert!(node(&open, "svc:billing").is_some());
    let (_, scoped) = get_auth(&srv.base, "/api/starmap", &demo).await;
    assert!(node(&scoped, "svc:billing").is_none(), "prod leaked");
    assert!(node(&scoped, "svc:checkout").is_some());
    let (_, other) = get_auth(&srv.base, "/api/starmap", &prod).await;
    assert!(node(&other, "svc:billing").is_some());
    assert!(node(&other, "svc:checkout").is_none(), "demo leaked");
}

#[tokio::test]
async fn a_suppression_shows_on_the_map_at_once() {
    let srv = spawn_server().await;
    let (_, before) = get(&srv.base, "/api/starmap").await;
    assert_eq!(before["counts"]["risks_suppressed"], 0, "{before}");
    let open = before["counts"]["risks_open"].as_u64().unwrap();
    assert!(open > 0, "{before}");

    let (_, risks) = get(&srv.base, "/api/risks").await;
    let id = risks["risks"][0]["id"].as_str().unwrap();
    let resp = reqwest::Client::new()
        .post(format!("{}/api/risks/{id}/suppress", srv.base))
        .json(&json!({"reason": "accepted", "actor": "ops"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 201);

    // The write drops the cached maps; nobody waits 30 s to see it.
    let (_, after) = get(&srv.base, "/api/starmap").await;
    let counts = &after["counts"];
    let placed = counts["risks_suppressed"].as_u64().unwrap();
    assert_eq!(
        counts["risks_open"].as_u64().unwrap() + placed,
        open,
        "{counts}"
    );
    assert_eq!(placed, 1, "{counts}");
}

#[tokio::test]
async fn a_focus_that_is_not_on_the_map_says_so() {
    let srv = spawn_server().await;
    seed_services(&srv).await;
    let (status, body) = get(&srv.base, "/api/starmap?focus=svc:nowhere").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["focus_found"], false);
    assert!(body["nodes"].as_array().unwrap().is_empty());
    let (_, body) = get(&srv.base, "/api/starmap?focus=svc:ledger").await;
    assert_eq!(body["focus_found"], true);
    let (_, body) = get(&srv.base, "/api/starmap").await;
    assert!(body["focus_found"].is_null());
}

/// A system and a service whose names carry the id separators.
async fn seed_awkward_names(srv: &TestServer) {
    let rows = vec![service_span(&Svc {
        service: "edge/proxy",
        system: "pay|ments/eu",
        env: "-",
        entity: "cif-0077",
        ..Svc::default()
    })];
    exec_write(srv, move |w| {
        w.insert_spans(&rows).map_err(|e| e.to_string())
    })
    .await;
}

#[tokio::test]
async fn topology_and_the_star_map_share_escaped_ids() {
    let srv = spawn_server().await;
    seed_awkward_names(&srv).await;
    let (_, map) = get(&srv.base, "/api/starmap").await;
    let system = node(&map, "sys:%2D/pay%7Cments%2Feu").expect("escaped system id");
    assert_eq!(system["label"], "pay|ments/eu");
    assert_eq!(system["env"], "-");
    assert!(node(&map, "svc:edge%2Fproxy").is_some(), "{:?}", ids(&map));
    for e in map["edges"].as_array().unwrap() {
        let id = e["id"].as_str().unwrap();
        assert_eq!(id.matches('|').count(), 2, "{id}");
    }

    let (status, topology) = get(&srv.base, "/api/topology").await;
    assert_eq!(status, 200, "{topology}");
    let found = ids(&topology);
    for id in ["sys:%2D/pay%7Cments%2Feu", "svc:edge%2Fproxy"] {
        assert!(found.iter().any(|f| f == id), "{id} not in {found:?}");
    }
}
