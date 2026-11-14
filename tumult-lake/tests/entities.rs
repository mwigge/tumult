//! Entity storage tests (schema v21): `entity_versions` coalesces entity
//! state events into one row per distinct state, and `entities_current`
//! derives liveness from the latest row of each entity.

use serde_json::{json, Value};
use tumult_lake::{
    entity_key, EntityEventKind, EntityEventRow, EntityOutcome, LogRow, Store, WireForm,
    CURRENT_SCHEMA_VERSION, ENTITY_INTERVAL_MAX_S,
};

/// 2026-01-01T00:00:00Z: long past for the view's clock, so a row last seen
/// here with a report interval is `expired`.
const PAST: i64 = 1_767_225_600_000_000_000;
/// 2100-01-01T00:00:00Z: ahead of the view's clock, so a row last seen here
/// is `live` whatever its interval.
const FUTURE: i64 = 4_102_444_800_000_000_000;
const SECOND: i64 = 1_000_000_000;

fn fixture() -> (tempfile::TempDir, Store) {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    (d, store)
}

fn host_id() -> Vec<(String, String)> {
    vec![("host.id".into(), "h-1".into())]
}

fn state(ts_ns: i64, description: Value) -> EntityEventRow {
    EntityEventRow {
        kind: EntityEventKind::State,
        ts_ns,
        received_at_ns: ts_ns,
        entity_type: "host".into(),
        entity_id: host_id(),
        description,
        relationships: json!([]),
        interval_s: 10,
        delete_reason: None,
        wire_form: WireForm::Spec,
        observer: "tok:a".into(),
        schema_url: Some("https://opentelemetry.io/schemas/1.40.0".into()),
        environment: "prod".into(),
    }
}

fn delete(ts_ns: i64) -> EntityEventRow {
    EntityEventRow {
        kind: EntityEventKind::Delete,
        description: json!({}),
        delete_reason: Some("terminated".into()),
        ..state(ts_ns, json!({}))
    }
}

fn apply(store: &Store, events: &[EntityEventRow]) -> Vec<EntityOutcome> {
    store.writer().unwrap().apply_entity_events(events).unwrap()
}

fn versions(store: &Store) -> Vec<Value> {
    store
        .read_only()
        .unwrap()
        .query_json_rows(
            "SELECT entity_key, environment, entity_type, entity_id['host.id'] AS host_id, \
             state_hash, CAST(description AS VARCHAR) AS description, \
             CAST(relationships AS VARCHAR) AS relationships, valid_from_ns, last_seen_ns, \
             last_received_ns, interval_s, deleted_at_ns, delete_reason, wire_form, observer, schema_url \
             FROM entity_versions ORDER BY environment, entity_key, valid_from_ns",
        )
        .unwrap()
}

fn current(store: &Store) -> Vec<Value> {
    store
        .read_only()
        .unwrap()
        .query_json_rows(
            "SELECT entity_key, environment, valid_from_ns, last_seen_ns, liveness \
             FROM entities_current ORDER BY environment, entity_key",
        )
        .unwrap()
}

#[test]
fn the_key_is_the_type_and_the_sorted_id_pairs() {
    let id = vec![
        ("k8s.pod.uid".to_string(), "u-1".to_string()),
        ("k8s.cluster.uid".to_string(), "c-1".to_string()),
    ];
    assert_eq!(
        entity_key("k8s.pod", &id),
        "k8s.pod|k8s.cluster.uid=c-1,k8s.pod.uid=u-1"
    );
    assert_eq!(entity_key("host", &host_id()), "host|host.id=h-1");
}

#[test]
fn the_key_escapes_its_separators_so_two_ids_never_collide() {
    let tricky = vec![("a".to_string(), "1,b=2".to_string())];
    let plain = vec![
        ("a".to_string(), "1".to_string()),
        ("b".to_string(), "2".to_string()),
    ];
    assert_ne!(entity_key("t", &tricky), entity_key("t", &plain));
    assert_eq!(entity_key("t", &tricky), "t|a=1%2Cb%3D2");
    // No raw path separator, so a key is one URL path segment.
    let slashed = vec![("p".to_string(), "/var/x|y%".to_string())];
    assert_eq!(entity_key("a/b", &slashed), "a%2Fb|p=%2Fvar%2Fx%7Cy%25");
}

/// The lookup of the latest row passes the keys of a batch to the store as
/// one JSON parameter. A quote, a backslash or text that reads like SQL in
/// an identifying value must come back as the same key, or every heartbeat
/// of that entity would open a row.
#[test]
fn a_key_with_quotes_and_backslashes_is_found_again() {
    let (_d, store) = fixture();
    let id = r#"a"b\c'd; DROP TABLE entity_versions; --"#;
    let odd = |ts_ns: i64| EntityEventRow {
        entity_id: vec![("host.id".into(), id.into())],
        ..state(ts_ns, json!({}))
    };
    assert_eq!(apply(&store, &[odd(PAST)]), vec![EntityOutcome::Opened]);
    assert_eq!(
        apply(&store, &[odd(PAST + SECOND)]),
        vec![EntityOutcome::Refreshed]
    );
    let rows = versions(&store);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["host_id"], id);
    assert_eq!(
        rows[0]["entity_key"],
        entity_key("host", &[("host.id".into(), id.into())])
    );
    assert_eq!(
        apply(&store, &[delete(PAST + 2 * SECOND)]),
        vec![EntityOutcome::DeleteUnknown],
        "the plain host is another entity"
    );
}

#[test]
fn the_first_state_opens_a_row_with_every_column() {
    let (_d, store) = fixture();
    let event = EntityEventRow {
        relationships: json!([{"relationship.type": "runs_on", "entity.type": "k8s.node"}]),
        ..state(PAST, json!({"os.type": "linux", "host.cpu.count": 8}))
    };
    assert_eq!(apply(&store, &[event]), vec![EntityOutcome::Opened]);

    let rows = versions(&store);
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row["entity_key"], "host|host.id=h-1");
    assert_eq!(row["environment"], "prod");
    assert_eq!(row["entity_type"], "host");
    assert_eq!(row["host_id"], "h-1");
    assert_eq!(
        serde_json::from_str::<Value>(row["description"].as_str().unwrap()).unwrap(),
        json!({"host.cpu.count": 8, "os.type": "linux"})
    );
    assert_eq!(
        serde_json::from_str::<Value>(row["relationships"].as_str().unwrap()).unwrap(),
        json!([{"relationship.type": "runs_on", "entity.type": "k8s.node"}])
    );
    assert_eq!(row["valid_from_ns"], PAST);
    assert_eq!(row["last_seen_ns"], PAST);
    assert_eq!(row["interval_s"], 10);
    assert_eq!(row["deleted_at_ns"], Value::Null);
    assert_eq!(row["delete_reason"], Value::Null);
    assert_eq!(row["wire_form"], "spec");
    assert_eq!(row["observer"], "tok:a");
    assert_eq!(row["schema_url"], "https://opentelemetry.io/schemas/1.40.0");
    assert_eq!(row["state_hash"].as_str().unwrap().len(), 64);
}

#[test]
fn an_identical_heartbeat_moves_last_seen_and_adds_no_row() {
    let (_d, store) = fixture();
    let desc = json!({"os.type": "linux"});
    apply(&store, &[state(PAST, desc.clone())]);
    let outcomes = apply(
        &store,
        &[
            state(PAST + 10 * SECOND, desc.clone()),
            state(PAST + 20 * SECOND, desc.clone()),
        ],
    );
    assert_eq!(
        outcomes,
        vec![EntityOutcome::Refreshed, EntityOutcome::Refreshed]
    );
    let rows = versions(&store);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["valid_from_ns"], PAST);
    assert_eq!(rows[0]["last_seen_ns"], PAST + 20 * SECOND);

    // A late copy of an older heartbeat never moves last_seen backwards.
    assert_eq!(
        apply(&store, &[state(PAST + 5 * SECOND, desc)]),
        vec![EntityOutcome::Refreshed]
    );
    assert_eq!(versions(&store)[0]["last_seen_ns"], PAST + 20 * SECOND);
}

#[test]
fn key_order_in_the_description_does_not_change_the_state() {
    let (_d, store) = fixture();
    let first: Value = serde_json::from_str(r#"{"a": 1, "b": {"x": 1, "y": 2}}"#).unwrap();
    let second: Value = serde_json::from_str(r#"{"b": {"y": 2, "x": 1}, "a": 1}"#).unwrap();
    apply(&store, &[state(PAST, first)]);
    assert_eq!(
        apply(&store, &[state(PAST + SECOND, second)]),
        vec![EntityOutcome::Refreshed]
    );
    assert_eq!(versions(&store).len(), 1);
}

#[test]
fn the_newest_heartbeat_sets_the_wire_form() {
    let (_d, store) = fixture();
    let desc = json!({"os.type": "linux"});
    apply(&store, &[state(PAST, desc.clone())]);
    let legacy = EntityEventRow {
        wire_form: WireForm::Legacy,
        ..state(PAST + SECOND, desc)
    };
    assert_eq!(apply(&store, &[legacy]), vec![EntityOutcome::Refreshed]);
    let rows = versions(&store);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["wire_form"], "legacy");
}

/// The interval decides when an entity counts as expired, so it is part of
/// what a version is. A heartbeat that names another interval opens a row
/// and the terms that held before stay on record; it never rewrites them
/// in place.
#[test]
fn a_changed_interval_opens_a_new_row() {
    let (_d, store) = fixture();
    let desc = json!({"os.type": "linux"});
    let with_interval = |ts_ns: i64, interval_s: i64| EntityEventRow {
        interval_s,
        ..state(ts_ns, desc.clone())
    };
    apply(&store, &[with_interval(PAST, 10)]);
    assert_eq!(
        apply(
            &store,
            &[
                with_interval(PAST + SECOND, 1),
                with_interval(PAST + 2 * SECOND, 1),
                with_interval(PAST + 3 * SECOND, 0),
            ]
        ),
        vec![
            EntityOutcome::Opened,
            EntityOutcome::Refreshed,
            EntityOutcome::Opened,
        ]
    );
    let rows = versions(&store);
    let intervals: Vec<i64> = rows
        .iter()
        .map(|r| r["interval_s"].as_i64().unwrap())
        .collect();
    assert_eq!(intervals, vec![10, 1, 0]);
    assert_eq!(rows[0]["state_hash"], rows[2]["state_hash"]);
    assert_eq!(rows[0]["last_seen_ns"], PAST, "the earlier terms are kept");
}

/// Like any changed state, another interval must be newer than the row to
/// replace it. A delayed or backdated copy leaves the row as it is.
#[test]
fn a_changed_interval_that_is_not_newer_changes_nothing() {
    let (_d, store) = fixture();
    apply(&store, &[state(PAST + 10 * SECOND, json!({}))]);
    let stored = versions(&store);
    for interval_s in [0, 1, ENTITY_INTERVAL_MAX_S] {
        let backdated = EntityEventRow {
            interval_s,
            received_at_ns: FUTURE,
            ..state(PAST + 10 * SECOND, json!({}))
        };
        assert_eq!(apply(&store, &[backdated]), vec![EntityOutcome::StaleState]);
    }
    assert_eq!(versions(&store), stored);
    assert_eq!(current(&store)[0]["liveness"], "expired");
}

#[test]
fn a_changed_state_opens_a_new_row_and_keeps_the_old_one() {
    let (_d, store) = fixture();
    apply(&store, &[state(PAST, json!({"os.version": "1"}))]);
    let outcomes = apply(&store, &[state(PAST + SECOND, json!({"os.version": "2"}))]);
    assert_eq!(outcomes, vec![EntityOutcome::Opened]);

    let rows = versions(&store);
    assert_eq!(rows.len(), 2);
    assert_ne!(rows[0]["state_hash"], rows[1]["state_hash"]);
    assert_eq!(rows[0]["last_seen_ns"], PAST);
    assert_eq!(rows[0]["deleted_at_ns"], Value::Null);
    assert_eq!(rows[1]["valid_from_ns"], PAST + SECOND);

    let now = current(&store);
    assert_eq!(now.len(), 1);
    assert_eq!(now[0]["valid_from_ns"], PAST + SECOND);
}

#[test]
fn a_changed_relationship_is_a_changed_state() {
    let (_d, store) = fixture();
    let desc = json!({"os.type": "linux"});
    apply(&store, &[state(PAST, desc.clone())]);
    let related = EntityEventRow {
        relationships: json!([{"relationship.type": "part_of"}]),
        ..state(PAST + SECOND, desc)
    };
    assert_eq!(apply(&store, &[related]), vec![EntityOutcome::Opened]);
    assert_eq!(versions(&store).len(), 2);
}

/// Against a row of an earlier batch a state must be strictly newer: one
/// with the timestamp the row was last seen at may be a delayed copy.
#[test]
fn a_changed_state_not_newer_than_the_stored_row_is_ignored() {
    let (_d, store) = fixture();
    apply(&store, &[state(PAST + 10 * SECOND, json!({"v": 2}))]);
    let outcomes = apply(
        &store,
        &[
            state(PAST, json!({"v": 1})),
            state(PAST + 10 * SECOND, json!({"v": 3})),
        ],
    );
    assert_eq!(
        outcomes,
        vec![EntityOutcome::StaleState, EntityOutcome::StaleState]
    );
    let rows = versions(&store);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["last_seen_ns"], PAST + 10 * SECOND);
}

/// Events without a timestamp, or with one ahead of the daemon's clock,
/// all carry the receipt time of their export. Within one export the order
/// of arrival then decides: the later event is the newer one.
#[test]
fn a_later_event_of_one_batch_wins_a_tie_on_the_timestamp() {
    let (_d, store) = fixture();
    let outcomes = apply(
        &store,
        &[
            state(FUTURE, json!({"v": 1})),
            state(FUTURE, json!({"v": 2})),
        ],
    );
    assert_eq!(outcomes, vec![EntityOutcome::Opened, EntityOutcome::Opened]);
    let rows = versions(&store);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["description"], r#"{"v":1}"#);
    assert_eq!(rows[0]["valid_from_ns"], FUTURE);
    assert_eq!(rows[1]["description"], r#"{"v":2}"#);
    assert_eq!(
        rows[1]["valid_from_ns"],
        FUTURE + 1,
        "one nanosecond later, so the rows of an entity stay ordered"
    );
    assert_eq!(current(&store)[0]["valid_from_ns"], FUTURE + 1);
}

#[test]
fn a_state_after_a_delete_in_one_batch_brings_the_entity_back() {
    let (_d, store) = fixture();
    let outcomes = apply(
        &store,
        &[
            state(FUTURE, json!({})),
            delete(FUTURE),
            state(FUTURE, json!({})),
        ],
    );
    assert_eq!(
        outcomes,
        vec![
            EntityOutcome::Opened,
            EntityOutcome::Tombstoned,
            EntityOutcome::Opened,
        ]
    );
    let rows = versions(&store);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["deleted_at_ns"], FUTURE);
    assert_eq!(rows[1]["deleted_at_ns"], Value::Null);
    assert_eq!(current(&store)[0]["liveness"], "live");
}

/// The same tie against a row that an earlier export stored.
#[test]
fn a_delete_then_a_state_at_the_time_of_the_stored_row_leave_it_live() {
    let (_d, store) = fixture();
    apply(&store, &[state(FUTURE, json!({}))]);
    assert_eq!(
        apply(&store, &[delete(FUTURE), state(FUTURE, json!({}))]),
        vec![EntityOutcome::Tombstoned, EntityOutcome::Opened]
    );
    assert_eq!(current(&store)[0]["liveness"], "live");
    assert_eq!(versions(&store).len(), 2);
}

#[test]
fn one_batch_at_one_instant_is_applied_in_the_order_it_arrived() {
    let (_d, store) = fixture();
    let outcomes = apply(
        &store,
        &[
            state(PAST, json!({"v": 1})),
            state(PAST, json!({"v": 2})),
            state(PAST, json!({"v": 2})),
            state(PAST, json!({"v": 1})),
            delete(PAST),
        ],
    );
    assert_eq!(
        outcomes,
        vec![
            EntityOutcome::Opened,
            EntityOutcome::Opened,
            EntityOutcome::Refreshed,
            EntityOutcome::Opened,
            EntityOutcome::Tombstoned,
        ]
    );
    let rows = versions(&store);
    let starts: Vec<i64> = rows
        .iter()
        .map(|r| r["valid_from_ns"].as_i64().unwrap())
        .collect();
    assert_eq!(starts, vec![PAST, PAST + 1, PAST + 2]);
    assert_eq!(rows[2]["description"], r#"{"v":1}"#);
    assert_eq!(rows[2]["deleted_at_ns"], PAST + 2);
    assert_eq!(rows[0]["deleted_at_ns"], Value::Null);
}

/// Arrival order settles a tie only inside one batch. A batch that is sent
/// again finds its own rows and changes nothing, and a different state
/// with the same timestamp in a later batch cannot be told from a copy
/// that was delayed: it stays stale.
#[test]
fn a_tie_with_a_stored_row_is_not_settled_by_a_later_batch() {
    let (_d, store) = fixture();
    let batch = [state(PAST, json!({"v": 1})), state(PAST, json!({"v": 2}))];
    apply(&store, &batch);
    let stored = versions(&store);
    assert_eq!(stored.len(), 2);
    assert_eq!(
        apply(&store, &batch),
        vec![EntityOutcome::StaleState, EntityOutcome::Refreshed],
        "the retry of an export"
    );
    assert_eq!(versions(&store), stored);
    assert_eq!(
        apply(&store, &[state(PAST, json!({"v": 3}))]),
        vec![EntityOutcome::StaleState]
    );
    assert_eq!(versions(&store), stored);
}

/// Arrival order does not overrule a timestamp: an older event later in
/// the batch is still older.
#[test]
fn an_older_event_later_in_the_batch_stays_stale() {
    let (_d, store) = fixture();
    apply(&store, &[state(PAST + 10 * SECOND, json!({"v": 1}))]);
    let outcomes = apply(
        &store,
        &[
            state(PAST + 5 * SECOND, json!({"v": 1})),
            state(PAST + 6 * SECOND, json!({"v": 2})),
            state(PAST + 20 * SECOND, json!({"v": 3})),
            state(PAST + 15 * SECOND, json!({"v": 4})),
            delete(PAST + 15 * SECOND),
        ],
    );
    assert_eq!(
        outcomes,
        vec![
            EntityOutcome::Refreshed,
            EntityOutcome::StaleState,
            EntityOutcome::Opened,
            EntityOutcome::StaleState,
            EntityOutcome::DeleteOutOfOrder,
        ]
    );
    assert_eq!(versions(&store).len(), 2);
}

#[test]
fn a_delete_sets_the_tombstone_and_the_entity_is_gone() {
    let (_d, store) = fixture();
    apply(&store, &[state(FUTURE, json!({}))]);
    assert_eq!(current(&store)[0]["liveness"], "live");
    assert_eq!(
        apply(&store, &[delete(FUTURE + SECOND)]),
        vec![EntityOutcome::Tombstoned]
    );
    let rows = versions(&store);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["deleted_at_ns"], FUTURE + SECOND);
    assert_eq!(rows[0]["delete_reason"], "terminated");
    assert_eq!(rows[0]["last_seen_ns"], FUTURE);
    assert_eq!(current(&store)[0]["liveness"], "gone");
}

#[test]
fn a_delete_at_the_same_instant_as_the_last_state_is_accepted() {
    let (_d, store) = fixture();
    apply(&store, &[state(PAST, json!({}))]);
    assert_eq!(
        apply(&store, &[delete(PAST)]),
        vec![EntityOutcome::Tombstoned]
    );
}

#[test]
fn an_out_of_order_delete_is_ignored() {
    let (_d, store) = fixture();
    apply(&store, &[state(PAST + 10 * SECOND, json!({}))]);
    assert_eq!(
        apply(&store, &[delete(PAST)]),
        vec![EntityOutcome::DeleteOutOfOrder]
    );
    assert_eq!(versions(&store)[0]["deleted_at_ns"], Value::Null);
}

#[test]
fn a_delete_from_another_observer_is_ignored() {
    let (_d, store) = fixture();
    apply(&store, &[state(PAST, json!({}))]);
    let forged = EntityEventRow {
        observer: "tok:b".into(),
        ..delete(PAST + SECOND)
    };
    assert_eq!(
        apply(&store, &[forged]),
        vec![EntityOutcome::DeleteForeignObserver]
    );
    assert_eq!(versions(&store)[0]["deleted_at_ns"], Value::Null);
}

#[test]
fn a_heartbeat_from_another_observer_does_not_take_over_the_row() {
    let (_d, store) = fixture();
    apply(&store, &[state(PAST, json!({}))]);
    let other = EntityEventRow {
        observer: "tok:b".into(),
        ..state(PAST + SECOND, json!({}))
    };
    assert_eq!(apply(&store, &[other]), vec![EntityOutcome::Refreshed]);
    assert_eq!(versions(&store)[0]["observer"], "tok:a");
    let forged = EntityEventRow {
        observer: "tok:b".into(),
        ..delete(PAST + 2 * SECOND)
    };
    assert_eq!(
        apply(&store, &[forged]),
        vec![EntityOutcome::DeleteForeignObserver]
    );
}

#[test]
fn a_delete_of_an_unknown_or_already_gone_entity_is_ignored() {
    let (_d, store) = fixture();
    assert_eq!(
        apply(&store, &[delete(PAST)]),
        vec![EntityOutcome::DeleteUnknown]
    );
    assert!(versions(&store).is_empty());

    apply(&store, &[state(PAST, json!({})), delete(PAST + SECOND)]);
    assert_eq!(
        apply(&store, &[delete(PAST + 2 * SECOND)]),
        vec![EntityOutcome::DeleteRepeated]
    );
    assert_eq!(versions(&store)[0]["deleted_at_ns"], PAST + SECOND);
}

#[test]
fn a_state_newer_than_the_tombstone_brings_the_entity_back() {
    let (_d, store) = fixture();
    apply(&store, &[state(PAST, json!({})), delete(PAST + SECOND)]);

    // Not newer than the tombstone: a late heartbeat of the deleted entity.
    assert_eq!(
        apply(&store, &[state(PAST + SECOND, json!({}))]),
        vec![EntityOutcome::StaleState]
    );
    assert_eq!(versions(&store).len(), 1);

    assert_eq!(
        apply(&store, &[state(FUTURE, json!({}))]),
        vec![EntityOutcome::Opened]
    );
    let rows = versions(&store);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["deleted_at_ns"], PAST + SECOND);
    assert_eq!(rows[1]["deleted_at_ns"], Value::Null);
    assert_eq!(rows[1]["state_hash"], rows[0]["state_hash"]);
    let now = current(&store);
    assert_eq!(now.len(), 1);
    assert_eq!(now[0]["liveness"], "live");
}

#[test]
fn one_batch_can_open_refresh_change_and_delete_the_same_entity() {
    let (_d, store) = fixture();
    let outcomes = apply(
        &store,
        &[
            state(PAST, json!({"v": 1})),
            state(PAST + SECOND, json!({"v": 1})),
            state(PAST + 2 * SECOND, json!({"v": 2})),
            delete(PAST + 3 * SECOND),
        ],
    );
    assert_eq!(
        outcomes,
        vec![
            EntityOutcome::Opened,
            EntityOutcome::Refreshed,
            EntityOutcome::Opened,
            EntityOutcome::Tombstoned,
        ]
    );
    let rows = versions(&store);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["last_seen_ns"], PAST + SECOND);
    assert_eq!(rows[0]["deleted_at_ns"], Value::Null);
    assert_eq!(rows[1]["valid_from_ns"], PAST + 2 * SECOND);
    assert_eq!(rows[1]["deleted_at_ns"], PAST + 3 * SECOND);
}

#[test]
fn liveness_follows_three_missed_report_intervals() {
    let (_d, store) = fixture();
    let host = |name: &str, ts_ns: i64, interval_s: i64| EntityEventRow {
        entity_id: vec![("host.id".into(), name.into())],
        interval_s,
        ..state(ts_ns, json!({}))
    };
    apply(
        &store,
        &[
            host("a-silent", PAST, 10),
            host("b-reporting", FUTURE, 10),
            host("c-no-interval", PAST, 0),
        ],
    );
    let now = current(&store);
    let liveness: Vec<&str> = now
        .iter()
        .map(|r| r["liveness"].as_str().unwrap())
        .collect();
    assert_eq!(liveness, vec!["expired", "live", "live"]);
}

/// The sender stamps the event and the daemon decides on liveness, each by
/// its own clock. A Collector whose clock is behind, or one that delivers
/// from a retry queue, must not make a reporting entity look silent, so
/// silence is counted from when the daemon received the last report.
#[test]
fn liveness_counts_from_the_receipt_and_not_from_the_event_time() {
    let (_d, store) = fixture();
    let host = |name: &str, ts_ns: i64, received_at_ns: i64| EntityEventRow {
        entity_id: vec![("host.id".into(), name.into())],
        received_at_ns,
        ..state(ts_ns, json!({}))
    };
    apply(
        &store,
        &[
            host("a-clock-behind", PAST, FUTURE),
            host("b-clock-ahead", FUTURE, PAST),
        ],
    );
    let now = current(&store);
    assert_eq!(now[0]["liveness"], "live");
    assert_eq!(now[1]["liveness"], "expired");
    let rows = versions(&store);
    assert_eq!(rows[0]["last_seen_ns"], PAST);
    assert_eq!(rows[0]["last_received_ns"], FUTURE);
    assert_eq!(rows[1]["last_seen_ns"], FUTURE);
    assert_eq!(rows[1]["last_received_ns"], PAST);
}

#[test]
fn every_heartbeat_moves_the_receipt_time_forward_and_never_back() {
    let (_d, store) = fixture();
    apply(&store, &[state(PAST + 10 * SECOND, json!({}))]);
    assert_eq!(current(&store)[0]["liveness"], "expired");

    // A copy from a retry queue: older than the last report by its own
    // time, but the daemon heard from the sender just now.
    let late = EntityEventRow {
        received_at_ns: FUTURE,
        ..state(PAST + 5 * SECOND, json!({}))
    };
    assert_eq!(apply(&store, &[late]), vec![EntityOutcome::Refreshed]);
    let rows = versions(&store);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["last_seen_ns"], PAST + 10 * SECOND);
    assert_eq!(rows[0]["last_received_ns"], FUTURE);
    assert_eq!(current(&store)[0]["liveness"], "live");

    apply(&store, &[state(PAST + 20 * SECOND, json!({}))]);
    let rows = versions(&store);
    assert_eq!(rows[0]["last_seen_ns"], PAST + 20 * SECOND);
    assert_eq!(rows[0]["last_received_ns"], FUTURE, "not moved back");
}

#[test]
fn a_delete_records_when_it_was_received() {
    let (_d, store) = fixture();
    apply(&store, &[state(PAST, json!({}))]);
    let gone = EntityEventRow {
        received_at_ns: PAST + 7 * SECOND,
        ..delete(PAST + SECOND)
    };
    assert_eq!(apply(&store, &[gone]), vec![EntityOutcome::Tombstoned]);
    assert_eq!(versions(&store)[0]["last_received_ns"], PAST + 7 * SECOND);
}

#[test]
fn the_report_interval_is_bounded() {
    let (_d, store) = fixture();
    let host = |name: &str, interval_s: i64| EntityEventRow {
        entity_id: vec![("host.id".into(), name.into())],
        interval_s,
        ..state(PAST, json!({}))
    };
    apply(&store, &[host("a", i64::MAX), host("b", -5)]);
    let rows = versions(&store);
    assert_eq!(rows[0]["interval_s"], ENTITY_INTERVAL_MAX_S);
    assert_eq!(rows[1]["interval_s"], 0);
    // The view's interval arithmetic stays inside BIGINT.
    assert_eq!(current(&store).len(), 2);
}

#[test]
fn the_same_entity_in_two_environments_is_tracked_separately() {
    let (_d, store) = fixture();
    let staging = EntityEventRow {
        environment: "staging".into(),
        ..state(FUTURE, json!({}))
    };
    apply(&store, &[state(FUTURE, json!({})), staging]);
    let prod_delete = delete(FUTURE + SECOND);
    assert_eq!(
        apply(&store, &[prod_delete]),
        vec![EntityOutcome::Tombstoned]
    );
    let now = current(&store);
    assert_eq!(now.len(), 2);
    assert_eq!(now[0]["environment"], "prod");
    assert_eq!(now[0]["liveness"], "gone");
    assert_eq!(now[1]["environment"], "staging");
    assert_eq!(now[1]["liveness"], "live");
}

#[test]
fn logs_and_entity_events_of_one_export_are_stored_together() {
    let (_d, store) = fixture();
    let log = LogRow {
        ts_ns: PAST,
        body: "plain".into(),
        ..LogRow::default()
    };
    let outcomes = store
        .writer()
        .unwrap()
        .insert_logs_and_entity_events(&[log], &[state(PAST, json!({}))])
        .unwrap();
    assert_eq!(outcomes, vec![EntityOutcome::Opened]);
    assert_eq!(versions(&store).len(), 1);
    let logs = store
        .read_only()
        .unwrap()
        .query_json_rows("SELECT body FROM logs")
        .unwrap();
    assert_eq!(logs.len(), 1);
}

#[test]
fn an_empty_batch_writes_nothing() {
    let (_d, store) = fixture();
    assert!(apply(&store, &[]).is_empty());
    assert!(versions(&store).is_empty());
}

fn has_entity_objects(store: &Store) -> (bool, bool) {
    let reader = store.read_only().unwrap();
    let count = |sql: &str| reader.query_json_rows(sql).unwrap()[0]["n"] == 1;
    (
        count(
            "SELECT count(*) AS n FROM information_schema.tables \
             WHERE table_name = 'entity_versions' AND table_type = 'BASE TABLE'",
        ),
        count(
            "SELECT count(*) AS n FROM information_schema.tables \
             WHERE table_name = 'entities_current' AND table_type = 'VIEW'",
        ),
    )
}

#[test]
fn a_fresh_lake_has_the_entity_table_and_the_view() {
    let (_d, store) = fixture();
    assert_eq!(has_entity_objects(&store), (true, true));
    assert!(current(&store).is_empty());
}

#[test]
fn a_v20_lake_gains_the_entity_table_and_the_view_and_reopens_cleanly() {
    let d = tempfile::TempDir::new().unwrap();
    let path = d.path().join("kronika.duckdb");
    {
        let store = Store::open(&path).unwrap();
        let writer = store.writer().unwrap();
        for sql in [
            "DROP VIEW entities_current",
            "DROP TABLE entity_versions",
            "UPDATE schema_meta SET value = 20 WHERE key = 'version'",
        ] {
            writer.execute(sql, []).unwrap();
        }
        drop(writer);
        assert_eq!(has_entity_objects(&store), (false, false));
    }
    let store = Store::open(&path).unwrap();
    assert_eq!(
        store.writer().unwrap().schema_version().unwrap(),
        CURRENT_SCHEMA_VERSION
    );
    assert_eq!(has_entity_objects(&store), (true, true));
    apply(&store, &[state(PAST, json!({"v": 1}))]);
    drop(store);

    // A second open keeps the stored rows: the migration is idempotent.
    let again = Store::open(&path).unwrap();
    assert_eq!(
        again.writer().unwrap().schema_version().unwrap(),
        CURRENT_SCHEMA_VERSION
    );
    assert_eq!(versions(&again).len(), 1);
}
