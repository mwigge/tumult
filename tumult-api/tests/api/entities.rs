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
        interval_s: Some(0),
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
        w.apply_entity_events(&events, &tumult_lake::EntityLimits::default())
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
    silent.interval_s = Some(10);
    let removed = state("k8s.pod", ("k8s.pod.uid", "c"), "prod", now - 50 * NS);
    let mut host = state("host", ("host.id", "h-1"), "staging", now - 5 * NS);
    host.wire_form = WireForm::Legacy;
    host.interval_s = Some(3_600);
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
    assert_eq!(
        body,
        json!({
            "count": 0,
            "total": 0,
            "truncated": false,
            "next_cursor": null,
            "entities": [],
        })
    );

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
    assert_eq!(body["total"], 1_010, "{}", body["total"]);
    assert_eq!(body["truncated"], true);
    let (_, body) = get(&srv.base, "/api/entities").await;
    assert_eq!(body["count"], 100, "a hundred unless asked");
    assert_eq!(body["total"], 1_010, "{}", body["total"]);
    assert_eq!(body["truncated"], true);
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

/// Follow `next_cursor` from `path` until the listing ends; the pages.
async fn pages(base: &str, path: &str) -> Vec<Value> {
    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let sep = if path.contains('?') { '&' } else { '?' };
        let url = match &cursor {
            Some(cursor) => format!("{path}{sep}cursor={cursor}"),
            None => path.to_owned(),
        };
        let (status, body) = get(base, &url).await;
        assert_eq!(status, 200, "{url}: {body}");
        cursor = body["next_cursor"].as_str().map(str::to_owned);
        assert_eq!(
            body["truncated"],
            cursor.is_some(),
            "a cursor is given exactly when rows are left: {body}"
        );
        seen.push(body);
        assert!(seen.len() <= 50, "the cursor does not advance");
        if cursor.is_none() {
            return seen;
        }
    }
}

#[tokio::test]
async fn the_entity_list_says_how_many_match_and_whether_it_is_cut() {
    let srv = spawn_server().await;
    seed_entities(&srv).await;
    let (_, body) = get(&srv.base, "/api/entities").await;
    assert_eq!(body["total"], 5, "{body}");
    assert_eq!(body["truncated"], false, "{body}");
    assert_eq!(body["next_cursor"], Value::Null, "{body}");

    let (_, body) = get(&srv.base, "/api/entities?limit=2").await;
    assert_eq!(body["count"], 2, "{body}");
    assert_eq!(body["total"], 5, "{body}");
    assert_eq!(body["truncated"], true, "{body}");
    assert!(body["next_cursor"].is_string(), "{body}");

    // The total counts what the filters match, not what the page holds.
    let (_, body) = get(&srv.base, "/api/entities?type=k8s.pod&limit=1").await;
    assert_eq!(body["total"], 4, "{body}");
    let staging = add_scoped_token(&srv, "stg-viewer", "viewer", &["staging"]).await;
    let (_, body) = get_auth(&srv.base, "/api/entities?limit=1", &staging).await;
    assert_eq!(
        body["total"], 1,
        "what is out of scope is not counted: {body}"
    );
    assert_eq!(body["truncated"], false, "{body}");
}

#[tokio::test]
async fn the_entity_list_is_walked_with_the_cursor() {
    let srv = spawn_server().await;
    seed_entities(&srv).await;
    let (_, whole) = get(&srv.base, "/api/entities").await;
    let walked = pages(&srv.base, "/api/entities?limit=2").await;
    assert_eq!(walked.len(), 3);
    let keys_walked: Vec<&str> = walked.iter().flat_map(keys).collect();
    assert_eq!(keys_walked, keys(&whole));
    assert!(walked.iter().all(|page| page["total"] == 5));

    // Filters hold on every page.
    let walked = pages(&srv.base, "/api/entities?type=k8s.pod&limit=3").await;
    let keys_walked: Vec<&str> = walked.iter().flat_map(keys).collect();
    assert_eq!(keys_walked.len(), 4);
    assert!(keys_walked.iter().all(|key| key.starts_with("k8s.pod|")));

    for broken in ["zz", "5b", "5b315d", ""] {
        let (status, body) = get(&srv.base, &format!("/api/entities?cursor={broken}")).await;
        if broken.is_empty() {
            assert_eq!(status, 200, "an empty cursor is none: {body}");
            continue;
        }
        assert_eq!(status, 400, "{broken}: {body}");
        assert_eq!(body["error"], "invalid cursor");
    }
}

/// Entities last heard of in the same nanosecond are told apart by key and
/// environment: no page repeats or skips one.
#[tokio::test]
async fn the_cursor_passes_through_entities_heard_of_at_the_same_time() {
    let srv = spawn_server().await;
    let at = now_ns() - NS;
    let mut same: Vec<EntityEventRow> = (0..7)
        .map(|n| state("proc", ("process.pid", &n.to_string()), "prod", at))
        .collect();
    same.push(state("proc", ("process.pid", "3"), "staging", at));
    store(&srv, same).await;
    let walked = pages(&srv.base, "/api/entities?limit=3").await;
    let mut seen: Vec<(String, String)> = walked
        .iter()
        .flat_map(|page| page["entities"].as_array().unwrap())
        .map(|e| {
            (
                e["entity_key"].as_str().unwrap().to_owned(),
                e["environment"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(seen.len(), 8);
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 8, "{seen:?}");
}

/// A page stops short of its row limit once it carries about four
/// mebibytes of descriptions, and the cursor continues from there.
#[tokio::test]
async fn a_page_is_bounded_in_bytes_as_well_as_rows() {
    let srv = spawn_server().await;
    let at = now_ns() - NS;
    let filler = "x".repeat(60_000);
    let large: Vec<EntityEventRow> = (0..100)
        .map(|n| {
            let mut event = state("proc", ("process.pid", &n.to_string()), "prod", at);
            event.description = json!({"process.command_line": filler});
            event
        })
        .collect();
    store(&srv, large).await;

    let response = reqwest::get(format!("{}/api/entities?limit=1000", srv.base))
        .await
        .unwrap();
    let bytes = response.bytes().await.unwrap();
    assert!(bytes.len() < 5 * 1024 * 1024, "{} bytes", bytes.len());
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    let count = body["count"].as_u64().unwrap();
    assert!((1..100).contains(&count), "{count}");
    assert_eq!(body["total"], 100);
    assert_eq!(body["truncated"], true);

    let walked = pages(&srv.base, "/api/entities?limit=1000").await;
    let rows: u64 = walked.iter().map(|p| p["count"].as_u64().unwrap()).sum();
    assert_eq!(rows, 100);
}

#[tokio::test]
async fn the_history_names_its_environments_and_is_walked_with_the_cursor() {
    let srv = spawn_server().await;
    let now = now_ns();
    let mut events = Vec::new();
    for n in 0..5 {
        let mut event = state("k8s.pod", ("k8s.pod.uid", "a"), "prod", now - (50 - n) * NS);
        event.description = json!({"k8s.pod.name": format!("checkout-{n}")});
        events.push(event);
    }
    // One version elsewhere that starts when a version in prod does.
    events.push(state(
        "k8s.pod",
        ("k8s.pod.uid", "a"),
        "staging",
        now - 50 * NS,
    ));
    events.push(state("k8s.pod", ("k8s.pod.uid", "a"), "", now - 10 * NS));
    store(&srv, events).await;
    let history = format!("/api/entities/{POD_A_PATH}/history");

    let (status, body) = get(&srv.base, &history).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["count"], 7, "{body}");
    assert_eq!(body["total"], 7, "{body}");
    assert_eq!(body["truncated"], false, "{body}");
    assert_eq!(body["next_cursor"], Value::Null, "{body}");
    assert_eq!(
        body["environments"],
        json!([
            {"environment": "", "versions": 1},
            {"environment": "prod", "versions": 5},
            {"environment": "staging", "versions": 1},
        ])
    );
    let whole: Vec<(i64, String)> = starts(&body);

    let (_, body) = get(&srv.base, &format!("{history}?limit=3")).await;
    assert_eq!(body["count"], 3, "{body}");
    assert_eq!(body["total"], 7, "{body}");
    assert_eq!(body["truncated"], true, "{body}");
    let walked = pages(&srv.base, &format!("{history}?limit=3")).await;
    let walked: Vec<(i64, String)> = walked.iter().flat_map(starts).collect();
    assert_eq!(walked, whole);

    // One environment at a time.
    let (_, body) = get(&srv.base, &format!("{history}?environment=prod&limit=2")).await;
    assert_eq!(body["total"], 5, "{body}");
    assert_eq!(body["truncated"], true, "{body}");
    assert!(starts(&body).iter().all(|(_, env)| env == "prod"));
    assert_eq!(body["environments"].as_array().unwrap().len(), 3);
    let (status, body) = get(&srv.base, &format!("{history}?environment=dev")).await;
    assert_eq!(status, 200, "the entity exists, though not there: {body}");
    assert_eq!(body["total"], 0, "{body}");
    assert_eq!(body["versions"], json!([]));

    // A scoped caller is told of its own environments only.
    let staging = add_scoped_token(&srv, "stg-viewer", "viewer", &["staging"]).await;
    let (_, body) = get_auth(&srv.base, &history, &staging).await;
    assert_eq!(body["total"], 1, "{body}");
    assert_eq!(
        body["environments"],
        json!([{"environment": "staging", "versions": 1}])
    );

    let (status, body) = get_auth(&srv.base, &format!("{history}?cursor=zz"), &staging).await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"], "invalid cursor");
}

fn starts(body: &Value) -> Vec<(i64, String)> {
    body["versions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            (
                v["valid_from_ns"].as_i64().unwrap(),
                v["environment"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// A key holds whatever the sender put in the id. A client that encodes it
/// as one URL path segment reads the entity, whatever the characters.
#[tokio::test]
async fn a_key_with_any_character_is_read_once_url_encoded() {
    let srv = spawn_server().await;
    let odd = "a\"b\\c/d%e f?g#h|i=j,k'l";
    store(
        &srv,
        vec![state("host", ("host.id", odd), "prod", now_ns() - NS)],
    )
    .await;
    let (_, body) = get(&srv.base, "/api/entities?type=host").await;
    let key = body["entities"][0]["entity_key"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(key, "host|host.id=a\"b\\c%2Fd%25e f?g#h%7Ci%3Dj%2Ck'l");
    assert_eq!(body["entities"][0]["entity_id"], json!({"host.id": odd}));

    let mut url = reqwest::Url::parse(&srv.base).unwrap();
    url.path_segments_mut()
        .unwrap()
        .extend(["api", "entities", key.as_str(), "history"]);
    assert!(url.path().contains("%252F"), "{url}");
    let response = reqwest::get(url).await.unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["entity_key"], key);
    assert_eq!(body["count"], 1, "{body}");

    // Sent as it is, the key is another path, or no path at all.
    let raw = reqwest::get(format!("{}/api/entities/{key}/history", srv.base))
        .await
        .unwrap();
    assert_ne!(raw.status(), 200);
}

// ---------------------------------------------------------------------------
// DELETE /api/entities?environment=…[&type=…]: an operator removes the
// entities of one environment.

async fn purge(base: &str, query: &str, token: Option<&str>) -> (u16, Value) {
    let mut request = reqwest::Client::new().delete(format!("{base}/api/entities{query}"));
    if let Some(token) = token {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    let resp = request.send().await.unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn a_purge_removes_the_entities_of_one_environment() {
    let srv = spawn_server().await;
    seed_entities(&srv).await;
    let (status, body) = purge(&srv.base, "?environment=prod", None).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body,
        json!({"environment": "prod", "type": null, "entities": 3, "versions": 3})
    );
    let (_, body) = get(&srv.base, "/api/entities").await;
    assert_eq!(keys(&body), ["host|host.id=h-1", "k8s.pod|k8s.pod.uid=d"]);
    let (status, _) = get(&srv.base, &format!("/api/entities/{POD_A_PATH}/history")).await;
    assert_eq!(status, 404, "the history went with the entity");

    let (status, body) = purge(&srv.base, "?environment=prod", None).await;
    assert_eq!(
        status, 200,
        "nothing left to remove is not an error: {body}"
    );
    assert_eq!(body["entities"], 0);
}

#[tokio::test]
async fn a_purge_of_one_type_leaves_the_other_types() {
    let srv = spawn_server().await;
    seed_entities(&srv).await;
    let node = state("k8s.node", ("k8s.node.uid", "n"), "prod", now_ns() - NS);
    store(&srv, vec![node]).await;
    let (status, body) = purge(&srv.base, "?environment=prod&type=k8s.pod", None).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["type"], "k8s.pod");
    assert_eq!(body["entities"], 3, "{body}");
    let (_, body) = get(&srv.base, "/api/entities").await;
    assert_eq!(
        keys(&body),
        [
            "k8s.node|k8s.node.uid=n",
            "host|host.id=h-1",
            "k8s.pod|k8s.pod.uid=d"
        ]
    );
}

/// The entities that name no environment are purged with an empty
/// `environment`; leaving the parameter out is refused, so a request
/// cannot remove them by omission.
#[tokio::test]
async fn a_purge_names_its_environment() {
    let srv = spawn_server().await;
    seed_entities(&srv).await;
    for query in ["", "?type=k8s.pod"] {
        let (status, body) = purge(&srv.base, query, None).await;
        assert_eq!(status, 400, "{query}: {body}");
        assert_eq!(body["error"], "environment is required", "{query}");
    }
    let (_, body) = get(&srv.base, "/api/entities").await;
    assert_eq!(body["count"], 5, "a refused purge removes nothing");

    let (status, body) = purge(&srv.base, "?environment=", None).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["entities"], 1, "{body}");
    let (_, body) = get(&srv.base, "/api/entities").await;
    assert_eq!(body["count"], 4);
}

#[tokio::test]
async fn a_purge_needs_the_operator_role() {
    let srv = spawn_server().await;
    seed_entities(&srv).await;
    let viewer = add_scoped_token(&srv, "a-viewer", "viewer", &[]).await;
    let operator = add_scoped_token(&srv, "an-operator", "operator", &[]).await;

    let (status, body) = purge(&srv.base, "?environment=prod", None).await;
    assert_eq!(status, 401, "{body}");
    let (status, body) = purge(&srv.base, "?environment=prod", Some(&viewer)).await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(body["error"], "insufficient role");
    let (_, body) = get_auth(&srv.base, "/api/entities", &viewer).await;
    assert_eq!(body["count"], 5, "a refused purge removes nothing");

    let (status, body) = purge(&srv.base, "?environment=prod", Some(&operator)).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["entities"], 3);
}

/// A scoped operator purges its own environments only, and never the
/// entities that name no environment.
#[tokio::test]
async fn a_purge_is_confined_to_the_principals_environments() {
    let srv = spawn_server().await;
    seed_entities(&srv).await;
    let staging = add_scoped_token(&srv, "stg-operator", "operator", &["staging"]).await;
    let all = add_scoped_token(&srv, "all-viewer", "viewer", &[]).await;

    for query in ["?environment=prod", "?environment="] {
        let (status, body) = purge(&srv.base, query, Some(&staging)).await;
        assert_eq!(status, 403, "{query}: {body}");
        assert_eq!(
            body["error"], "the environment is outside the principal's scopes",
            "{query}"
        );
    }
    let (_, body) = get_auth(&srv.base, "/api/entities", &all).await;
    assert_eq!(body["count"], 5, "a refused purge removes nothing");

    let (status, body) = purge(&srv.base, "?environment=staging", Some(&staging)).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["entities"], 1);
    let (_, body) = get_auth(&srv.base, "/api/entities", &all).await;
    assert_eq!(body["count"], 4);
}
