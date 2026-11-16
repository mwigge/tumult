//! Entity storage tests (schema v21): `entity_versions` coalesces entity
//! state events into one row per distinct state, and `entities_current`
//! derives liveness from the latest row of each entity.

use serde_json::{json, Value};
use tumult_lake::{
    entity_key, EntityApplied, EntityEventKind, EntityEventRow, EntityLimits, EntityOutcome,
    LogRow, Store, WireForm, CURRENT_SCHEMA_VERSION, ENTITIES_PER_ENVIRONMENT,
    ENTITY_INTERVAL_MAX_S, ENTITY_VERSIONS_PER_ENTITY,
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
        interval_s: Some(10),
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
    apply_within(store, events, &EntityLimits::default()).outcomes
}

fn apply_within(store: &Store, events: &[EntityEventRow], limits: &EntityLimits) -> EntityApplied {
    store
        .writer()
        .unwrap()
        .apply_entity_events(events, limits)
        .unwrap()
}

/// The state of the host `id`, with `n` in its description.
fn host(id: &str, ts_ns: i64, n: i64) -> EntityEventRow {
    EntityEventRow {
        entity_id: vec![("host.id".into(), id.into())],
        ..state(ts_ns, json!({"n": n}))
    }
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
        interval_s: Some(interval_s),
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

fn intervals(store: &Store) -> Vec<i64> {
    versions(store)
        .iter()
        .map(|r| r["interval_s"].as_i64().unwrap())
        .collect()
}

/// An event that states no interval says nothing about it: the stored one
/// stays. Two reporters of one entity, of which one sends no interval,
/// would otherwise open a row with every report, and the one without
/// would make the entity never expire.
#[test]
fn a_state_without_an_interval_keeps_the_stored_one() {
    let (_d, store) = fixture();
    let desc = json!({"os.type": "linux"});
    let unstated = |ts_ns: i64, description: Value| EntityEventRow {
        interval_s: None,
        ..state(ts_ns, description)
    };
    apply(&store, &[state(PAST, desc.clone())]);
    assert_eq!(
        apply(
            &store,
            &[
                unstated(PAST + SECOND, desc.clone()),
                state(PAST + 2 * SECOND, desc.clone()),
                unstated(PAST + 3 * SECOND, desc.clone()),
            ]
        ),
        vec![EntityOutcome::Refreshed; 3]
    );
    assert_eq!(intervals(&store), vec![10]);
    // A changed state without an interval opens a row under the same terms.
    assert_eq!(
        apply(&store, &[unstated(PAST + 4 * SECOND, json!({"n": 2}))]),
        vec![EntityOutcome::Opened]
    );
    assert_eq!(intervals(&store), vec![10, 10]);
    assert_eq!(current(&store)[0]["liveness"], "expired");
}

/// The first event of an entity that states no interval opens it without
/// one: nothing says it reports periodically.
#[test]
fn an_entity_first_seen_without_an_interval_has_none() {
    let (_d, store) = fixture();
    let unstated = EntityEventRow {
        interval_s: None,
        ..state(PAST, json!({}))
    };
    assert_eq!(apply(&store, &[unstated]), vec![EntityOutcome::Opened]);
    assert_eq!(intervals(&store), vec![0]);
    assert_eq!(current(&store)[0]["liveness"], "live");
}

/// An interval of 0 that is stated is a statement: the entity sends no
/// periodic reports from here on.
#[test]
fn a_stated_interval_of_zero_still_means_no_expiry() {
    let (_d, store) = fixture();
    let zero = EntityEventRow {
        interval_s: Some(0),
        ..state(PAST + SECOND, json!({}))
    };
    apply(&store, &[state(PAST, json!({}))]);
    assert_eq!(apply(&store, &[zero]), vec![EntityOutcome::Opened]);
    assert_eq!(intervals(&store), vec![10, 0]);
    assert_eq!(current(&store)[0]["liveness"], "live");
}

/// An entity that comes back after a delete without stating an interval
/// comes back under the terms it had.
#[test]
fn an_entity_that_returns_without_an_interval_keeps_the_one_it_had() {
    let (_d, store) = fixture();
    let back = EntityEventRow {
        interval_s: None,
        ..state(PAST + 2 * SECOND, json!({}))
    };
    apply(&store, &[state(PAST, json!({})), delete(PAST + SECOND)]);
    assert_eq!(apply(&store, &[back]), vec![EntityOutcome::Opened]);
    assert_eq!(intervals(&store), vec![10, 10]);
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
            interval_s: Some(interval_s),
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

/// The observer says how the event that opened a row came in. It is a
/// record and decides nothing: the daemon has one ingest token, so it
/// cannot tell one sender from another, and a rule that compared observers
/// refused only the deletes that followed a change of the token setting.
#[test]
fn a_delete_is_accepted_whatever_observer_opened_the_row() {
    let (_d, store) = fixture();
    apply(&store, &[state(PAST, json!({}))]);
    let other = EntityEventRow {
        observer: "tok:b".into(),
        ..delete(PAST + SECOND)
    };
    assert_eq!(apply(&store, &[other]), vec![EntityOutcome::Tombstoned]);
    let rows = versions(&store);
    assert_eq!(rows[0]["deleted_at_ns"], PAST + SECOND);
    assert_eq!(rows[0]["observer"], "tok:a");
}

#[test]
fn a_row_keeps_the_observer_that_opened_it() {
    let (_d, store) = fixture();
    apply(&store, &[state(PAST, json!({}))]);
    let other = |ts_ns: i64, description: Value| EntityEventRow {
        observer: "tok:b".into(),
        ..state(ts_ns, description)
    };
    assert_eq!(
        apply(&store, &[other(PAST + SECOND, json!({}))]),
        vec![EntityOutcome::Refreshed]
    );
    assert_eq!(versions(&store)[0]["observer"], "tok:a");
    assert_eq!(
        apply(&store, &[other(PAST + 2 * SECOND, json!({"v": 2}))]),
        vec![EntityOutcome::Opened]
    );
    let rows = versions(&store);
    assert_eq!(rows[0]["observer"], "tok:a");
    assert_eq!(rows[1]["observer"], "tok:b");
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
        interval_s: Some(interval_s),
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
        interval_s: Some(interval_s),
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
        .insert_logs_and_entity_events(&[log], &[state(PAST, json!({}))], &EntityLimits::default())
        .unwrap()
        .outcomes;
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

#[test]
fn the_default_limits_are_the_documented_ones() {
    let limits = EntityLimits::default();
    assert_eq!(ENTITY_VERSIONS_PER_ENTITY, 100);
    assert_eq!(ENTITIES_PER_ENVIRONMENT, 50_000);
    assert_eq!(limits.versions_per_entity, 100);
    assert_eq!(limits.entities_per_environment, 50_000);
    assert_eq!(limits.entities_total, 200_000);
    assert_eq!(EntityLimits::per_environment(7).entities_total, 28);
    assert_eq!(
        EntityLimits::per_environment(usize::MAX).entities_total,
        usize::MAX
    );
}

fn keep(versions_per_entity: usize) -> EntityLimits {
    EntityLimits {
        versions_per_entity,
        ..EntityLimits::default()
    }
}

fn starts(store: &Store) -> Vec<i64> {
    versions(store)
        .iter()
        .map(|row| row["valid_from_ns"].as_i64().unwrap())
        .collect()
}

/// A sender that changes the description with every report gets a bounded
/// history, not a table that grows with time.
#[test]
fn an_entity_keeps_only_its_newest_versions() {
    let (_d, store) = fixture();
    let mut pruned = 0;
    for n in 0..5 {
        let applied = apply_within(&store, &[host("a", PAST + n * SECOND, n)], &keep(3));
        assert_eq!(applied.outcomes, vec![EntityOutcome::Opened]);
        pruned += applied.versions_pruned;
    }
    assert_eq!(pruned, 2);
    assert_eq!(
        starts(&store),
        vec![PAST + 2 * SECOND, PAST + 3 * SECOND, PAST + 4 * SECOND]
    );
    assert_eq!(current(&store).len(), 1);
}

#[test]
fn the_version_cap_holds_within_one_batch_and_per_environment() {
    let (_d, store) = fixture();
    let mut batch: Vec<EntityEventRow> = (0..5).map(|n| host("a", PAST + n * SECOND, n)).collect();
    // Another entity and the same entity elsewhere have histories of their
    // own.
    batch.push(host("b", PAST, 0));
    batch.push(EntityEventRow {
        environment: "dev".into(),
        ..host("a", PAST, 0)
    });
    let applied = apply_within(&store, &batch, &keep(2));
    assert_eq!(applied.versions_pruned, 3);
    let rows = versions(&store);
    let kept: Vec<(&str, &str, i64)> = rows
        .iter()
        .map(|row| {
            (
                row["environment"].as_str().unwrap(),
                row["host_id"].as_str().unwrap(),
                row["valid_from_ns"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        kept,
        vec![
            ("dev", "a", PAST),
            ("prod", "a", PAST + 3 * SECOND),
            ("prod", "a", PAST + 4 * SECOND),
            ("prod", "b", PAST),
        ]
    );
}

#[test]
fn a_version_cap_of_zero_still_keeps_the_latest_row() {
    let (_d, store) = fixture();
    for n in 0..3 {
        apply_within(&store, &[host("a", PAST + n * SECOND, n)], &keep(0));
    }
    assert_eq!(starts(&store), vec![PAST + 2 * SECOND]);
}

/// The cap counts entities, not events: what has a row is served as before,
/// and only an entity without one is refused. (What makes room at the cap
/// is in `entity_caps.rs`; here every entity is named by the batch that
/// would need its place.)
#[test]
fn a_new_entity_beyond_the_environment_cap_is_refused() {
    let (_d, store) = fixture();
    let limits = EntityLimits::per_environment(2);
    assert_eq!(
        apply_within(
            &store,
            &[host("a", PAST, 0), host("b", PAST, 0), host("c", PAST, 0)],
            &limits
        )
        .outcomes,
        vec![
            EntityOutcome::Opened,
            EntityOutcome::Opened,
            EntityOutcome::EntityLimit,
        ]
    );
    assert!(EntityOutcome::EntityLimit.is_ignored());
    let later = PAST + SECOND;
    let dev = EntityEventRow {
        environment: "dev".into(),
        ..host("c", later, 0)
    };
    let gone = EntityEventRow {
        kind: EntityEventKind::Delete,
        ..host("b", later, 0)
    };
    assert_eq!(
        apply_within(
            &store,
            &[
                host("c", later, 0),
                host("a", later, 0),
                host("a", later + 1, 1),
                dev,
                gone,
                host("b", later + SECOND, 0),
            ],
            &limits
        )
        .outcomes,
        vec![
            EntityOutcome::EntityLimit,
            EntityOutcome::Refreshed,
            EntityOutcome::Opened,
            EntityOutcome::Opened,
            EntityOutcome::Tombstoned,
            EntityOutcome::Opened,
        ],
        "refused, heartbeat, changed state, another environment, delete, back again"
    );
    let entities: Vec<(String, String)> = current(&store)
        .iter()
        .map(|row| {
            (
                row["environment"].as_str().unwrap().to_owned(),
                row["entity_key"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(entities.len(), 3, "{entities:?}");
    assert!(!entities.contains(&("prod".into(), "host|host.id=c".into())));
}

#[test]
fn an_entity_named_twice_in_one_batch_takes_one_place() {
    let (_d, store) = fixture();
    assert_eq!(
        apply_within(
            &store,
            &[
                host("a", PAST, 0),
                host("a", PAST + 1, 0),
                host("a", PAST + 2, 1),
                host("b", PAST, 0),
                host("c", PAST, 0),
            ],
            &EntityLimits::per_environment(2)
        )
        .outcomes,
        vec![
            EntityOutcome::Opened,
            EntityOutcome::Refreshed,
            EntityOutcome::Opened,
            EntityOutcome::Opened,
            EntityOutcome::EntityLimit,
        ]
    );
}

/// The environment is the sender's to name, so the environments together
/// are bounded too. The entities here are live: nothing can make room.
#[test]
fn a_new_entity_beyond_the_total_cap_is_refused_in_any_environment() {
    let (_d, store) = fixture();
    let limits = EntityLimits {
        entities_per_environment: 1,
        entities_total: 2,
        ..EntityLimits::default()
    };
    let in_env = |environment: &str| EntityEventRow {
        environment: environment.into(),
        ..host("a", FUTURE, 0)
    };
    assert_eq!(
        apply_within(&store, &[in_env("one"), in_env("two")], &limits).outcomes,
        vec![EntityOutcome::Opened, EntityOutcome::Opened]
    );
    assert_eq!(
        apply_within(&store, &[in_env("three"), in_env("one")], &limits).outcomes,
        vec![EntityOutcome::EntityLimit, EntityOutcome::Refreshed]
    );
}

fn gone_host(id: &str, ts_ns: i64) -> [EntityEventRow; 2] {
    [
        host(id, ts_ns, 0),
        EntityEventRow {
            kind: EntityEventKind::Delete,
            ..host(id, ts_ns + 1, 0)
        },
    ]
}

fn host_ids(store: &Store) -> Vec<String> {
    let mut ids: Vec<String> = versions(store)
        .iter()
        .map(|row| row["host_id"].as_str().unwrap().to_owned())
        .collect();
    ids.dedup();
    ids
}

/// Entities that are gone or expired leave, with every row, once the daemon
/// has not heard of them since the cutoff. A live entity stays whatever its
/// age.
#[test]
fn pruning_removes_entities_that_are_not_live_and_left_the_window() {
    let (_d, store) = fixture();
    let once = |id: &str, ts_ns: i64| EntityEventRow {
        interval_s: Some(0),
        ..host(id, ts_ns, 0)
    };
    let mut events = Vec::new();
    events.extend(gone_host("gone-old", PAST));
    // Two rows, the second one expired.
    events.push(host("expired-old", PAST, 0));
    events.push(host("expired-old", PAST + SECOND, 1));
    events.push(once("live-old", PAST));
    events.extend(gone_host("gone-recent", PAST + 100 * SECOND));
    apply(&store, &events);
    assert_eq!(versions(&store).len(), 5);

    let writer = store.writer().unwrap();
    assert_eq!(writer.entity_version_count().unwrap(), 5);
    assert_eq!(writer.prune_entities(PAST, 100).unwrap(), 0, "none older");
    assert_eq!(writer.prune_entities(PAST + 50 * SECOND, 100).unwrap(), 3);
    assert_eq!(writer.entity_version_count().unwrap(), 2);
    drop(writer);
    assert_eq!(host_ids(&store), vec!["gone-recent", "live-old"]);
}

#[test]
fn pruning_takes_the_longest_silent_entities_first_and_no_more_than_asked() {
    let (_d, store) = fixture();
    let mut events = Vec::new();
    for (n, id) in ["c", "a", "b"].into_iter().enumerate() {
        events.extend(gone_host(id, PAST + i64::try_from(n).unwrap() * SECOND));
    }
    apply(&store, &events);
    let writer = store.writer().unwrap();
    assert_eq!(writer.prune_entities(FUTURE, 2).unwrap(), 2);
    drop(writer);
    assert_eq!(host_ids(&store), vec!["b"]);
}

/// The table carries no index: it receives an UPDATE with every heartbeat,
/// and since schema v5 no table that is updated has one (an index that
/// WAL replay leaves out of step with its table makes every later UPDATE
/// fail). The writer therefore has to be quick without one.
#[test]
fn the_entity_table_has_no_index() {
    let (_d, store) = fixture();
    let indexes = store
        .read_only()
        .unwrap()
        .query_json_rows(
            "SELECT index_name FROM duckdb_indexes() WHERE table_name = 'entity_versions'",
        )
        .unwrap();
    assert!(indexes.is_empty(), "{indexes:?}");
}

/// The table as the first development builds of v21 created it: without
/// `last_received_ns`. `CREATE TABLE IF NOT EXISTS` leaves such a table as
/// it is, so the column is added and filled from `last_seen_ns`, the best
/// record there is of when the daemon last heard of the entity.
#[test]
fn a_lake_from_before_the_receipt_column_opens_and_is_backfilled() {
    let (_d, store) = fixture();
    let writer = store.writer().unwrap();
    for statement in [
        "DROP VIEW entities_current",
        "DROP TABLE entity_versions",
        "CREATE TABLE entity_versions (
            entity_key VARCHAR NOT NULL,
            environment VARCHAR NOT NULL DEFAULT '',
            entity_type VARCHAR NOT NULL,
            entity_id MAP (VARCHAR, VARCHAR) NOT NULL,
            state_hash VARCHAR NOT NULL,
            description JSON,
            relationships JSON,
            valid_from_ns BIGINT NOT NULL,
            last_seen_ns BIGINT NOT NULL,
            interval_s BIGINT NOT NULL DEFAULT 0,
            deleted_at_ns BIGINT,
            delete_reason VARCHAR,
            wire_form VARCHAR NOT NULL,
            observer VARCHAR NOT NULL,
            schema_url VARCHAR
        )",
        "INSERT INTO entity_versions VALUES (
            'host|host.id=old', 'prod', 'host', MAP {'host.id': 'old'}, 'h',
            '{}', '[]', 1000, 2000, 10, NULL, NULL, 'spec', 'tok:a', NULL
        )",
    ] {
        writer.execute(statement, []).unwrap();
    }
    drop(writer);
    let reopened = Store::open(store.path()).unwrap();
    let rows = versions(&reopened);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["last_received_ns"], 2000);
    assert_eq!(current(&reopened)[0]["liveness"], "expired");
    // The table takes events as one created by this build does.
    assert_eq!(
        apply(&reopened, &[host("new", FUTURE, 1), host("old", FUTURE, 1)]),
        vec![EntityOutcome::Opened; 2]
    );
    assert_eq!(versions(&reopened).len(), 3);
    // Opening again finds the column and changes nothing.
    let before = versions(&reopened);
    assert_eq!(versions(&Store::open(store.path()).unwrap()), before);
}

fn receipts(store: &Store) -> Vec<(i64, i64, i64, Option<i64>)> {
    versions(store)
        .iter()
        .map(|row| {
            (
                row["valid_from_ns"].as_i64().unwrap(),
                row["last_seen_ns"].as_i64().unwrap(),
                row["last_received_ns"].as_i64().unwrap(),
                row["deleted_at_ns"].as_i64(),
            )
        })
        .collect()
}

/// A stored row that one batch first refreshes and then supersedes is
/// written with what the refresh recorded: when the entity was last seen
/// in that state and when the daemon heard of it. Writing only the latest
/// row of each entity would lose it.
#[test]
fn a_row_refreshed_and_then_superseded_in_one_batch_keeps_the_refresh() {
    let (_d, store) = fixture();
    apply(&store, &[host("a", PAST, 0)]);
    let heartbeat = EntityEventRow {
        received_at_ns: PAST + 50 * SECOND,
        ..host("a", PAST + 5 * SECOND, 0)
    };
    let change = EntityEventRow {
        received_at_ns: PAST + 60 * SECOND,
        ..host("a", PAST + 6 * SECOND, 1)
    };
    assert_eq!(
        apply(&store, &[heartbeat, change]),
        vec![EntityOutcome::Refreshed, EntityOutcome::Opened]
    );
    assert_eq!(
        receipts(&store),
        vec![
            (PAST, PAST + 5 * SECOND, PAST + 50 * SECOND, None),
            (
                PAST + 6 * SECOND,
                PAST + 6 * SECOND,
                PAST + 60 * SECOND,
                None
            ),
        ]
    );
}

/// The same for a tombstone: a stored row that one batch deletes and then
/// supersedes (the entity came back) keeps its tombstone, its reason and
/// the time the delete was received.
#[test]
fn a_row_deleted_and_then_superseded_in_one_batch_keeps_the_tombstone() {
    let (_d, store) = fixture();
    apply(&store, &[host("a", PAST, 0)]);
    let gone = EntityEventRow {
        kind: EntityEventKind::Delete,
        received_at_ns: PAST + 50 * SECOND,
        delete_reason: Some("drained".into()),
        ..host("a", PAST + 5 * SECOND, 0)
    };
    let back = EntityEventRow {
        received_at_ns: PAST + 60 * SECOND,
        ..host("a", PAST + 6 * SECOND, 0)
    };
    assert_eq!(
        apply(&store, &[gone, back]),
        vec![EntityOutcome::Tombstoned, EntityOutcome::Opened]
    );
    assert_eq!(
        receipts(&store),
        vec![
            (PAST, PAST, PAST + 50 * SECOND, Some(PAST + 5 * SECOND)),
            (
                PAST + 6 * SECOND,
                PAST + 6 * SECOND,
                PAST + 60 * SECOND,
                None
            ),
        ]
    );
    assert_eq!(versions(&store)[0]["delete_reason"], "drained");
}
