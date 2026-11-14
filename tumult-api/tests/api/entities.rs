use crate::common::*;
use serde_json::{json, Value};
use tumult_lake::{EntityEventKind, EntityEventRow, WireForm};

// ---------------------------------------------------------------------------
// GET /api/entities and GET /api/entities/{key}/history: what the entity
// events left in the store, per environment.

const POD_A: &str = "k8s.pod|k8s.pod.uid=a";
/// `POD_A` as one URL path segment.
const POD_A_PATH: &str = "k8s.pod%7Ck8s.pod.uid=a";

fn state(entity_type: &str, id: (&str, &str), env: &str, ts_ns: i64) -> EntityEventRow {
    EntityEventRow {
        kind: EntityEventKind::State,
        ts_ns,
        received_at_ns: ts_ns,
        entity_type: entity_type.into(),
        entity_id: vec![(id.0.into(), id.1.into())],
        description: json!({"k8s.pod.name": "checkout-1"}),
        relationships: json!([]),
        interval_s: 0,
        delete_reason: None,
        wire_form: WireForm::Spec,
        observer: "token:default".into(),
        schema_url: None,
        environment: env.into(),
    }
}

fn delete(mut of: EntityEventRow, ts_ns: i64, reason: &str) -> EntityEventRow {
    of.kind = EntityEventKind::Delete;
    of.ts_ns = ts_ns;
    of.received_at_ns = ts_ns;
    of.delete_reason = Some(reason.into());
    of
}

async fn store(srv: &TestServer, events: Vec<EntityEventRow>) {
    exec_write(srv, move |w| {
        w.apply_entity_events(&events)
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
    .await;
}

/// Three entities in `prod` (one live, one expired, one gone), one live
/// host in `staging` and one pod that names no environment.
async fn seed_entities(srv: &TestServer) {
    let now = now_ns();
    let mut silent = state("k8s.pod", ("k8s.pod.uid", "b"), "prod", now - 100 * NS);
    silent.interval_s = 10;
    let removed = state("k8s.pod", ("k8s.pod.uid", "c"), "prod", now - 50 * NS);
    let mut host = state("host", ("host.id", "h-1"), "staging", now - 5 * NS);
    host.wire_form = WireForm::Legacy;
    host.interval_s = 3_600;
    store(
        srv,
        vec![
            state("k8s.pod", ("k8s.pod.uid", "a"), "prod", now - 20 * NS),
            silent,
            removed.clone(),
            delete(removed, now - 40 * NS, "scaled_down"),
            host,
            state("k8s.pod", ("k8s.pod.uid", "d"), "", now - 10 * NS),
        ],
    )
    .await;
}

fn keys(body: &Value) -> Vec<&str> {
    body["entities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["entity_key"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn a_store_without_entities_lists_none() {
    let srv = spawn_server().await;
    let (status, body) = get(&srv.base, "/api/entities").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, json!({"count": 0, "entities": []}));

    let (status, body) = get(&srv.base, &format!("/api/entities/{POD_A_PATH}/history")).await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(body, json!({"error": "entity not found"}));
}

/// `body` with every array put in one order. `/api/topology` lists nodes
/// with the same number of runs, and their edges, in no fixed order.
fn in_one_order(body: Value) -> Value {
    match body {
        Value::Array(items) => {
            let mut items: Vec<Value> = items.into_iter().map(in_one_order).collect();
            items.sort_by_key(Value::to_string);
            Value::Array(items)
        }
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .map(|(name, value)| (name, in_one_order(value)))
                .collect(),
        ),
        other => other,
    }
}

/// One read for the comparison below: status and body, the body in one
/// order where the endpoint promises none.
async fn read(base: &str, path: &str) -> (u16, Value) {
    let (status, body) = get(base, path).await;
    if path == "/api/topology" {
        return (status, in_one_order(body));
    }
    (status, body)
}

/// Entity rows are read only by the entity endpoints: the telemetry reads
/// answer the same with them as without.
#[tokio::test]
async fn entity_rows_change_no_other_read() {
    let srv = spawn_server().await;
    let paths = [
        "/api/logs?range=30d",
        "/api/traces?range=30d",
        "/api/experiments?range=30d",
        "/api/topology",
        "/api/metrics/catalog",
    ];
    let mut before = Vec::new();
    for path in paths {
        before.push(read(&srv.base, path).await);
    }
    seed_entities(&srv).await;
    for (path, expected) in paths.iter().zip(&before) {
        assert_eq!(expected.0, 200, "{path}: {}", expected.1);
        assert_eq!(&read(&srv.base, path).await, expected, "{path}");
    }
}

#[tokio::test]
async fn entities_are_listed_newest_first_with_their_liveness() {
    let srv = spawn_server().await;
    seed_entities(&srv).await;
    let (status, body) = get(&srv.base, "/api/entities").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["count"], 5, "{body}");
    assert_eq!(
        keys(&body),
        [
            "host|host.id=h-1",
            "k8s.pod|k8s.pod.uid=d",
            POD_A,
            "k8s.pod|k8s.pod.uid=c",
            "k8s.pod|k8s.pod.uid=b",
        ]
    );
    let host = &body["entities"][0];
    assert_eq!(host["entity_type"], "host");
    assert_eq!(host["entity_id"], json!({"host.id": "h-1"}));
    assert_eq!(host["environment"], "staging");
    assert_eq!(host["liveness"], "live");
    assert_eq!(host["wire_form"], "legacy");
    assert_eq!(host["observer"], "token:default");
    assert_eq!(host["interval_s"], 3_600);
    assert_eq!(host["description"], json!({"k8s.pod.name": "checkout-1"}));
    assert_eq!(host["relationships"], json!([]));
    assert_eq!(host["deleted_at_ns"], Value::Null);
    assert_eq!(body["entities"][3]["liveness"], "gone");
    assert_eq!(body["entities"][3]["delete_reason"], "scaled_down");
    assert_eq!(body["entities"][4]["liveness"], "expired");
}

#[tokio::test]
async fn the_entity_list_filters_by_type_liveness_and_range() {
    let srv = spawn_server().await;
    seed_entities(&srv).await;
    let old = state(
        "k8s.node",
        ("k8s.node.uid", "n"),
        "prod",
        now_ns() - 3 * 86_400 * NS,
    );
    store(&srv, vec![old]).await;

    let (_, body) = get(&srv.base, "/api/entities?type=host").await;
    assert_eq!(keys(&body), ["host|host.id=h-1"]);
    let (_, body) = get(&srv.base, "/api/entities?liveness=gone").await;
    assert_eq!(keys(&body), ["k8s.pod|k8s.pod.uid=c"]);
    let (_, body) = get(&srv.base, "/api/entities?liveness=expired").await;
    assert_eq!(keys(&body), ["k8s.pod|k8s.pod.uid=b"]);
    let (_, body) = get(&srv.base, "/api/entities?type=k8s.pod&liveness=live").await;
    assert_eq!(keys(&body), ["k8s.pod|k8s.pod.uid=d", POD_A]);

    // Without a range every entity is listed; with one, only those last
    // heard of inside it.
    let (_, body) = get(&srv.base, "/api/entities?type=k8s.node").await;
    assert_eq!(body["count"], 1, "{body}");
    let (_, body) = get(&srv.base, "/api/entities?type=k8s.node&range=24h").await;
    assert_eq!(body["count"], 0, "{body}");
    let (_, body) = get(&srv.base, "/api/entities?type=k8s.node&range=7d").await;
    assert_eq!(body["count"], 1, "{body}");

    // A type is matched as a value, never as SQL.
    let (status, body) = get(&srv.base, "/api/entities?type=host'%20OR%20'1'='1").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["count"], 0, "{body}");
}

#[tokio::test]
async fn the_entity_list_refuses_unknown_filters_and_is_bounded() {
    let srv = spawn_server().await;
    seed_entities(&srv).await;
    let (status, body) = get(&srv.base, "/api/entities?liveness=dead").await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(
        body["error"],
        "invalid liveness \"dead\"; expected live|expired|gone"
    );
    let (status, body) = get(&srv.base, "/api/entities?range=1y").await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(
        body["error"],
        "invalid range \"1y\"; expected 24h|7d|14d|30d"
    );

    let (_, body) = get(&srv.base, "/api/entities?limit=2").await;
    assert_eq!(body["count"], 2, "{body}");
    let (status, body) = get(&srv.base, "/api/entities?limit=0").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["count"], 1, "a limit below one is one: {body}");

    let now = now_ns();
    let many: Vec<EntityEventRow> = (0..1_005)
        .map(|n| state("proc", ("process.pid", &n.to_string()), "prod", now - NS))
        .collect();
    store(&srv, many).await;
    let (_, body) = get(&srv.base, "/api/entities?limit=100000").await;
    assert_eq!(body["count"], 1_000, "never more than a thousand");
    let (_, body) = get(&srv.base, "/api/entities").await;
    assert_eq!(body["count"], 200, "two hundred unless asked");
}

/// A scoped principal sees the entities of its environments only; an
/// entity that names no environment belongs to none of them.
#[tokio::test]
async fn entity_reads_are_confined_to_the_principals_environments() {
    let srv = spawn_server().await;
    seed_entities(&srv).await;
    let staging = add_scoped_token(&srv, "stg-viewer", "viewer", &["staging"]).await;
    let all = add_scoped_token(&srv, "all-viewer", "viewer", &[]).await;

    let (status, body) = get_auth(&srv.base, "/api/entities", &staging).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(keys(&body), ["host|host.id=h-1"]);
    let (_, body) = get_auth(&srv.base, "/api/entities", &all).await;
    assert_eq!(body["count"], 5, "{body}");

    let history = format!("/api/entities/{POD_A_PATH}/history");
    let (status, body) = get_auth(&srv.base, &history, &staging).await;
    assert_eq!(status, 404, "an entity out of scope is not found: {body}");
    assert_eq!(body, json!({"error": "entity not found"}));
    let (status, body) = get_auth(&srv.base, &history, &all).await;
    assert_eq!(status, 200, "{body}");

    // With users in the store an anonymous read is refused.
    let anonymous = reqwest::get(format!("{}/api/entities", srv.base))
        .await
        .unwrap();
    assert_eq!(anonymous.status(), 401);
    let anonymous = reqwest::get(format!("{}{history}", srv.base))
        .await
        .unwrap();
    assert_eq!(anonymous.status(), 401);
}

#[tokio::test]
async fn the_history_lists_every_version_newest_first() {
    let srv = spawn_server().await;
    let now = now_ns();
    let first = state("k8s.pod", ("k8s.pod.uid", "a"), "prod", now - 30 * NS);
    let mut second = first.clone();
    second.ts_ns = now - 20 * NS;
    second.description = json!({"k8s.pod.name": "checkout-1", "k8s.pod.phase": "Running"});
    let gone = delete(second.clone(), now - 10 * NS, "evicted");
    let elsewhere = state("k8s.pod", ("k8s.pod.uid", "a"), "staging", now - 5 * NS);
    store(&srv, vec![first, second, gone, elsewhere]).await;

    let (status, body) = get(&srv.base, &format!("/api/entities/{POD_A_PATH}/history")).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["entity_key"], POD_A);
    assert_eq!(body["count"], 3, "{body}");
    let versions = body["versions"].as_array().unwrap();
    assert_eq!(versions[0]["environment"], "staging");
    assert_eq!(versions[0]["valid_from_ns"], now - 5 * NS);
    assert_eq!(versions[1]["environment"], "prod");
    assert_eq!(versions[1]["valid_from_ns"], now - 20 * NS);
    assert_eq!(versions[1]["deleted_at_ns"], now - 10 * NS);
    assert_eq!(versions[1]["delete_reason"], "evicted");
    assert_eq!(
        versions[1]["description"],
        json!({"k8s.pod.name": "checkout-1", "k8s.pod.phase": "Running"})
    );
    assert_eq!(versions[2]["valid_from_ns"], now - 30 * NS);
    assert_eq!(versions[2]["deleted_at_ns"], Value::Null);
    assert_eq!(versions[2]["entity_id"], json!({"k8s.pod.uid": "a"}));

    let (_, body) = get(
        &srv.base,
        &format!("/api/entities/{POD_A_PATH}/history?limit=1"),
    )
    .await;
    assert_eq!(body["count"], 1, "{body}");
    assert_eq!(body["versions"][0]["environment"], "staging");

    let (status, body) = get(&srv.base, "/api/entities/k8s.pod%7Ck8s.pod.uid=zz/history").await;
    assert_eq!(status, 404, "{body}");
}
