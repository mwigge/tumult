//! How the entity table is kept within its bounds: the count the writer
//! holds in memory, the eviction that makes room at a bound, the operator's
//! purge, and what a batch costs in statements.

use std::collections::BTreeMap;

use serde_json::json;
use tumult_lake::{
    EntityApplied, EntityEventKind, EntityEventRow, EntityEvictions, EntityLimits, EntityOutcome,
    EntityPurge, EvictionReason, LogRow, Store, WireForm, Writer, ENTITY_UNREPORTED_AFTER_S,
};

/// 2026-01-01T00:00:00Z: long past for the view's clock.
const PAST: i64 = 1_767_225_600_000_000_000;
/// 2100-01-01T00:00:00Z: ahead of the view's clock, so an entity heard of
/// here is live whatever its interval.
const FUTURE: i64 = 4_102_444_800_000_000_000;
const SECOND: i64 = 1_000_000_000;

/// The wall clock `S` seconds on. A writer searches a bound for something
/// to evict at most once in a few seconds; a test that makes it search
/// again moves its clock on instead of waiting.
fn ahead<const S: i64>() -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    i64::try_from(now.as_nanos()).unwrap() + S * SECOND
}

fn fixture() -> (tempfile::TempDir, Store) {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    (d, store)
}

/// A host that reports every ten seconds, heard of at `ts_ns`.
fn host(id: &str, ts_ns: i64) -> EntityEventRow {
    EntityEventRow {
        kind: EntityEventKind::State,
        ts_ns,
        received_at_ns: ts_ns,
        entity_type: "host".into(),
        entity_id: vec![("host.id".into(), id.into())],
        description: json!({"n": 0}),
        relationships: json!([]),
        interval_s: Some(10),
        delete_reason: None,
        wire_form: WireForm::Spec,
        observer: "tok:a".into(),
        schema_url: None,
        environment: "prod".into(),
    }
}

/// A host that reports now and goes on reporting.
fn live(id: &str) -> EntityEventRow {
    host(id, FUTURE)
}

/// A host that reported at `ts_ns` and says it sends no periodic reports.
fn once(id: &str, ts_ns: i64) -> EntityEventRow {
    EntityEventRow {
        interval_s: Some(0),
        ..host(id, ts_ns)
    }
}

fn deleted(id: &str, ts_ns: i64) -> EntityEventRow {
    EntityEventRow {
        kind: EntityEventKind::Delete,
        ..host(id, ts_ns)
    }
}

fn within(event: EntityEventRow, environment: &str) -> EntityEventRow {
    EntityEventRow {
        environment: environment.into(),
        ..event
    }
}

/// The event `n` nanoseconds later, with another description.
fn changed(event: EntityEventRow, n: i64) -> EntityEventRow {
    EntityEventRow {
        description: json!({"n": n}),
        ts_ns: event.ts_ns + n,
        ..event
    }
}

fn apply(writer: &Writer, events: &[EntityEventRow], limits: &EntityLimits) -> EntityApplied {
    writer.apply_entity_events(events, limits).unwrap()
}

fn outcomes(
    writer: &Writer,
    events: &[EntityEventRow],
    limits: &EntityLimits,
) -> Vec<EntityOutcome> {
    apply(writer, events, limits).outcomes
}

/// The entities per environment as the table holds them.
fn stored(store: &Store) -> BTreeMap<String, usize> {
    store
        .read_only()
        .unwrap()
        .query_json_rows(
            "SELECT environment, count(DISTINCT entity_key) AS entities \
             FROM entity_versions GROUP BY environment",
        )
        .unwrap()
        .iter()
        .map(|row| {
            (
                row["environment"].as_str().unwrap().to_owned(),
                usize::try_from(row["entities"].as_u64().unwrap()).unwrap(),
            )
        })
        .collect()
}

fn host_ids(store: &Store, environment: &str) -> Vec<String> {
    store
        .read_only()
        .unwrap()
        .query_json_rows(&format!(
            "SELECT DISTINCT entity_id['host.id'] AS id FROM entity_versions \
             WHERE environment = '{environment}' ORDER BY id"
        ))
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap().to_owned())
        .collect()
}

fn count(store: &Store, table: &str) -> u64 {
    store
        .read_only()
        .unwrap()
        .query_json_rows(&format!("SELECT count(*) AS n FROM {table}"))
        .unwrap()[0]["n"]
        .as_u64()
        .unwrap()
}

fn held_is_stored(writer: &Writer, store: &Store, after: &str) -> BTreeMap<String, usize> {
    let held = writer.entities_held().unwrap();
    assert_eq!(held, stored(store), "after {after}");
    held
}

const BREAK_INSERT: &str = "ALTER TABLE entity_versions RENAME COLUMN schema_url TO schema_link";
const MEND_INSERT: &str = "ALTER TABLE entity_versions RENAME COLUMN schema_link TO schema_url";

/// One writer through everything that adds or removes an entity: what it
/// counts in memory is what the table holds, every time.
#[test]
fn the_count_the_writer_holds_equals_the_table_after_every_change() {
    let (_d, store) = fixture();
    let mut writer = store.writer().unwrap();
    let limits = EntityLimits::per_environment(3);
    let one = EntityLimits {
        versions_per_entity: 1,
        ..limits
    };
    assert!(held_is_stored(&writer, &store, "nothing").is_empty());

    apply(
        &writer,
        &[live("a"), live("b"), within(live("c"), "dev")],
        &limits,
    );
    let held = held_is_stored(&writer, &store, "three opened");
    assert_eq!(held["prod"], 2);
    assert_eq!(held["dev"], 1);

    assert_eq!(
        outcomes(&writer, &[live("d"), live("e")], &limits),
        vec![EntityOutcome::Opened, EntityOutcome::EntityLimit]
    );
    held_is_stored(&writer, &store, "a refusal");

    apply(&writer, &[deleted("b", FUTURE + 1)], &limits);
    held_is_stored(&writer, &store, "a delete");

    writer.set_clock(ahead::<10>);
    let applied = apply(&writer, &[live("f")], &limits);
    assert_eq!(applied.outcomes, vec![EntityOutcome::Opened]);
    assert_eq!(applied.evicted.total(), 1);
    assert_eq!(
        held_is_stored(&writer, &store, "an eviction")["prod"],
        3,
        "b went and f came"
    );

    for n in 1..4 {
        apply(&writer, &[changed(live("a"), n)], &one);
    }
    held_is_stored(&writer, &store, "the version cap");

    apply(&writer, &[deleted("d", FUTURE + 1)], &limits);
    assert_eq!(writer.prune_entities(i64::MAX, 10).unwrap(), 1);
    assert_eq!(held_is_stored(&writer, &store, "a prune")["prod"], 2);

    assert_eq!(
        writer.purge_entities("dev", None).unwrap(),
        EntityPurge {
            entities: 1,
            versions: 1
        }
    );
    let held = held_is_stored(&writer, &store, "a purge");
    assert!(!held.contains_key("dev"), "{held:?}");

    writer.execute(BREAK_INSERT, []).unwrap();
    assert!(writer.apply_entity_events(&[live("g")], &limits).is_err());
    writer.execute(MEND_INSERT, []).unwrap();
    assert_eq!(
        held_is_stored(&writer, &store, "a batch that failed")["prod"],
        2,
        "g was counted and never stored"
    );

    assert_eq!(
        outcomes(&writer, &[live("g"), live("h")], &limits),
        vec![EntityOutcome::Opened, EntityOutcome::EntityLimit]
    );
    held_is_stored(&writer, &store, "the batch after it");
}

/// At the bound of an environment the entities nobody reports any more go
/// first: gone or expired, the one the daemon heard of longest ago before
/// the others, then those that send no periodic reports and were silent
/// for a day. Only when none is left is a new entity refused.
#[test]
fn at_the_bound_the_silent_entities_make_room_before_anything_is_refused() {
    let (_d, store) = fixture();
    let mut writer = store.writer().unwrap();
    let limits = EntityLimits::per_environment(4);
    assert_eq!(limits.eviction_chunk(), 1);
    apply(
        &writer,
        &[
            once("unreported", PAST),
            host("expired", PAST + SECOND),
            host("gone", PAST + 2 * SECOND),
            deleted("gone", PAST + 3 * SECOND),
            live("live"),
        ],
        &limits,
    );
    let mut evictions = Vec::new();
    let later: [(&str, fn() -> i64); 3] =
        [("n1", ahead::<0>), ("n2", ahead::<10>), ("n3", ahead::<20>)];
    for (id, clock) in later {
        writer.set_clock(clock);
        let applied = apply(&writer, &[live(id)], &limits);
        assert_eq!(applied.outcomes, vec![EntityOutcome::Opened], "{id}");
        evictions.push(applied.evicted);
    }
    let of = |reason: EvictionReason| {
        let mut evicted = EntityEvictions::default();
        match reason {
            EvictionReason::Gone => evicted.gone = 1,
            EvictionReason::Expired => evicted.expired = 1,
            EvictionReason::Unreported => evicted.unreported = 1,
        }
        evicted
    };
    assert_eq!(
        evictions,
        vec![
            of(EvictionReason::Expired),
            of(EvictionReason::Gone),
            of(EvictionReason::Unreported),
        ],
        "longest silent of the gone and expired first, the unreported last"
    );
    writer.set_clock(ahead::<30>);
    let refused = apply(&writer, &[live("n4")], &limits);
    assert_eq!(refused.outcomes, vec![EntityOutcome::EntityLimit]);
    assert_eq!(refused.evicted, EntityEvictions::default());
    assert_eq!(host_ids(&store, "prod"), vec!["live", "n1", "n2", "n3"]);
}

#[test]
fn eviction_reasons_are_named_as_in_the_metric() {
    let names: Vec<&str> = EvictionReason::ALL
        .iter()
        .map(|reason| reason.as_str())
        .collect();
    assert_eq!(names, vec!["gone", "expired", "unreported"]);
    let evicted = EntityEvictions {
        gone: 1,
        expired: 2,
        unreported: 4,
    };
    assert_eq!(evicted.total(), 7);
    assert_eq!(evicted.count(EvictionReason::Expired), 2);
}

/// One eviction makes room for more than the entity that asked: looking
/// costs a pass over the table.
#[test]
fn one_eviction_removes_a_hundredth_of_the_bound() {
    let (_d, store) = fixture();
    let writer = store.writer().unwrap();
    let limits = EntityLimits::per_environment(300);
    assert_eq!(limits.eviction_chunk(), 3);
    assert_eq!(EntityLimits::per_environment(0).eviction_chunk(), 1);
    assert_eq!(EntityLimits::default().eviction_chunk(), 500);
    assert_eq!(
        EntityLimits::per_environment(usize::MAX).eviction_chunk(),
        5_000
    );
    let old: Vec<EntityEventRow> = (0..300)
        .map(|n| host(&format!("old-{n:03}"), PAST + n * SECOND))
        .collect();
    apply(&writer, &old, &limits);
    let applied = apply(&writer, &[live("new")], &limits);
    assert_eq!(applied.evicted.expired, 3);
    let ids = host_ids(&store, "prod");
    assert_eq!(ids.len(), 298);
    assert_eq!(ids[0], "new");
    assert_eq!(ids[1], "old-003", "the three longest silent went");
}

/// An entity that sends no periodic reports never expires. It is live for
/// the API however old it is; at a bound it makes room once it was silent
/// for the time the limits name, and not before.
#[test]
fn an_entity_without_reports_makes_room_only_after_the_documented_silence() {
    assert_eq!(ENTITY_UNREPORTED_AFTER_S, 86_400);
    assert_eq!(EntityLimits::default().unreported_after_s, 86_400);
    let (_d, store) = fixture();
    let writer = store.writer().unwrap();
    let limits = EntityLimits::per_environment(1);
    apply(&writer, &[once("recent", FUTURE)], &limits);
    assert_eq!(
        outcomes(&writer, &[live("b")], &limits),
        vec![EntityOutcome::EntityLimit],
        "heard of within the day"
    );
    apply(&writer, &[within(once("old", PAST), "dev")], &limits);
    let patient = EntityLimits {
        unreported_after_s: 200 * 365 * 86_400,
        ..limits
    };
    assert_eq!(
        outcomes(&writer, &[within(live("b"), "dev")], &patient),
        vec![EntityOutcome::EntityLimit],
        "the time the limits name has not passed"
    );
}

#[test]
fn an_old_entity_without_reports_is_evicted_under_the_default_silence() {
    let (_d, store) = fixture();
    let writer = store.writer().unwrap();
    let limits = EntityLimits::per_environment(1);
    apply(&writer, &[once("old", PAST)], &limits);
    let applied = apply(&writer, &[live("b")], &limits);
    assert_eq!(applied.outcomes, vec![EntityOutcome::Opened]);
    assert_eq!(applied.evicted.unreported, 1);
    assert_eq!(host_ids(&store, "prod"), vec!["b"]);
}

/// What the batch itself names is never evicted for it: the batch may be
/// about to report it.
#[test]
fn an_entity_the_batch_names_is_not_evicted_for_that_batch() {
    let (_d, store) = fixture();
    let writer = store.writer().unwrap();
    let limits = EntityLimits::per_environment(1);
    apply(&writer, &[host("a", PAST)], &limits);
    assert_eq!(
        outcomes(&writer, &[live("b"), live("a")], &limits),
        vec![EntityOutcome::EntityLimit, EntityOutcome::Refreshed]
    );
    assert_eq!(host_ids(&store, "prod"), vec!["a"]);
}

/// At the bound of the lake, when the environment of the new entity holds
/// nothing to evict, room is made in whatever environment has a silent
/// entity.
#[test]
fn at_the_bound_of_the_lake_room_is_made_in_any_environment_when_its_own_has_none() {
    let (_d, store) = fixture();
    let writer = store.writer().unwrap();
    let limits = EntityLimits {
        entities_per_environment: 2,
        entities_total: 2,
        ..EntityLimits::default()
    };
    apply(
        &writer,
        &[within(host("a", PAST), "one"), within(live("a"), "two")],
        &limits,
    );
    let applied = apply(&writer, &[within(live("a"), "three")], &limits);
    assert_eq!(applied.outcomes, vec![EntityOutcome::Opened]);
    assert_eq!(applied.evicted.expired, 1);
    assert_eq!(
        outcomes(&writer, &[within(live("a"), "four")], &limits),
        vec![EntityOutcome::EntityLimit]
    );
    let held = held_is_stored(&writer, &store, "an eviction for the lake");
    assert_eq!(held.keys().collect::<Vec<_>>(), vec!["three", "two"]);
}

/// At the bound of the lake a new entity makes room in its own environment
/// when that holds a silent entity, also when another environment holds
/// one that was silent for longer: what a sender adds in one environment
/// does not push out the history of another.
#[test]
fn at_the_bound_of_the_lake_room_is_made_in_the_environment_of_the_new_entity_first() {
    let (_d, store) = fixture();
    let writer = store.writer().unwrap();
    let limits = EntityLimits {
        entities_per_environment: 10,
        entities_total: 4,
        ..EntityLimits::default()
    };
    apply(
        &writer,
        &[
            within(host("silent", PAST), "b"),
            within(live("live"), "b"),
            within(host("silent", PAST + SECOND), "a"),
            within(live("live"), "a"),
        ],
        &limits,
    );
    let applied = apply(&writer, &[within(live("new"), "a")], &limits);
    assert_eq!(applied.outcomes, vec![EntityOutcome::Opened]);
    assert_eq!(applied.evicted.expired, 1);
    assert_eq!(host_ids(&store, "a"), vec!["live", "new"]);
    assert_eq!(host_ids(&store, "b"), vec!["live", "silent"]);
    held_is_stored(&writer, &store, "an eviction in the environment itself");
}

/// The writer remembers for a while that a bound held nothing to evict,
/// so a sender that keeps naming new entities does not make every export
/// search the table. A delete ends that: there is something now, and the
/// next search of the bound, a few seconds after the last, finds it.
#[test]
fn a_delete_makes_room_for_the_next_search() {
    let (_d, store) = fixture();
    let mut writer = store.writer().unwrap();
    let limits = EntityLimits::per_environment(2);
    apply(&writer, &[live("a"), live("b")], &limits);

    let before = writer.entity_statements();
    assert_eq!(
        outcomes(&writer, &[live("c")], &limits),
        vec![EntityOutcome::EntityLimit]
    );
    let searched = writer.entity_statements() - before;
    let before = writer.entity_statements();
    assert_eq!(
        outcomes(&writer, &[live("d")], &limits),
        vec![EntityOutcome::EntityLimit]
    );
    let remembered = writer.entity_statements() - before;
    assert!(
        remembered < searched,
        "the second refusal ran {remembered} statements, the first {searched}"
    );

    apply(&writer, &[deleted("a", FUTURE + 1)], &limits);
    assert_eq!(
        outcomes(&writer, &[live("c")], &limits),
        vec![EntityOutcome::EntityLimit],
        "the bound was searched a moment ago"
    );
    writer.set_clock(ahead::<10>);
    let applied = apply(&writer, &[live("c")], &limits);
    assert_eq!(applied.outcomes, vec![EntityOutcome::Opened]);
    assert_eq!(applied.evicted.gone, 1);
}

/// The operator's purge removes an environment, or one type in it, with
/// every row, and nothing else.
#[test]
fn a_purge_removes_one_environment_or_one_type_in_it() {
    let (_d, store) = fixture();
    let writer = store.writer().unwrap();
    let limits = EntityLimits::per_environment(4);
    let service = EntityEventRow {
        entity_type: "service".into(),
        entity_id: vec![("service.name".into(), "api".into())],
        ..live("unused")
    };
    apply(
        &writer,
        &[
            live("a"),
            changed(live("a"), 1),
            live("b"),
            service,
            within(live("a"), "dev"),
        ],
        &limits,
    );
    assert_eq!(
        writer.purge_entities("staging", None).unwrap(),
        EntityPurge::default()
    );
    assert_eq!(
        writer.purge_entities("prod", Some("k8s.pod")).unwrap(),
        EntityPurge::default()
    );
    assert_eq!(
        writer.purge_entities("prod", Some("host")).unwrap(),
        EntityPurge {
            entities: 2,
            versions: 3
        }
    );
    let held = held_is_stored(&writer, &store, "the hosts of prod");
    assert_eq!(held["prod"], 1);
    assert_eq!(held["dev"], 1);
    assert_eq!(
        writer.purge_entities("prod", None).unwrap(),
        EntityPurge {
            entities: 1,
            versions: 1
        }
    );
    assert_eq!(count(&store, "entity_versions"), 1);
    assert_eq!(host_ids(&store, "dev"), vec!["a"]);
}

/// A purge is how an operator ends a lock-out: the environment is full of
/// live entities, nothing can be evicted, and after the purge there is
/// room at once.
#[test]
fn a_purge_makes_room_at_once() {
    let (_d, store) = fixture();
    let writer = store.writer().unwrap();
    let limits = EntityLimits::per_environment(2);
    apply(&writer, &[live("a"), live("b")], &limits);
    assert_eq!(
        outcomes(&writer, &[live("c")], &limits),
        vec![EntityOutcome::EntityLimit]
    );
    writer.purge_entities("prod", None).unwrap();
    assert_eq!(
        outcomes(&writer, &[live("c"), live("d"), live("e")], &limits),
        vec![
            EntityOutcome::Opened,
            EntityOutcome::Opened,
            EntityOutcome::EntityLimit
        ]
    );
}

/// A lake with three hosts and a service in prod and a host in dev, one of
/// the hosts of prod gone, and a writer that has counted nothing yet: the
/// daemon after a start, after it rebuilt its connection, or after a
/// transaction that failed.
fn a_writer_that_has_not_counted() -> (tempfile::TempDir, Store, Writer) {
    let (d, store) = fixture();
    let service = EntityEventRow {
        entity_type: "service".into(),
        entity_id: vec![("service.name".into(), "api".into())],
        ..live("unused")
    };
    let first = store.writer().unwrap();
    apply(
        &first,
        &[
            live("a"),
            live("b"),
            host("gone", PAST),
            deleted("gone", PAST + SECOND),
            service,
            within(live("a"), "dev"),
        ],
        &EntityLimits::default(),
    );
    drop(first);
    let writer = store.writer().unwrap();
    (d, store, writer)
}

fn total(held: &BTreeMap<String, usize>) -> usize {
    held.values().sum()
}

/// What a removal takes is taken out of the count once, also when the
/// writer counts the table for the first time because of that removal.
#[test]
fn a_purge_of_an_environment_by_a_writer_that_has_not_counted_leaves_the_count_true() {
    let (_d, store, writer) = a_writer_that_has_not_counted();
    assert_eq!(writer.purge_entities("prod", None).unwrap().entities, 4);
    let held = held_is_stored(&writer, &store, "a purge of prod");
    assert_eq!(held, BTreeMap::from([("dev".to_owned(), 1)]));
    assert_eq!(total(&held), 1);
    // The count of the lake is true as well: one more entity fills it.
    let two = EntityLimits {
        entities_total: 2,
        ..EntityLimits::default()
    };
    assert_eq!(
        outcomes(&writer, &[live("c"), within(live("c"), "test")], &two),
        vec![EntityOutcome::Opened, EntityOutcome::EntityLimit]
    );
}

#[test]
fn a_purge_of_one_type_by_a_writer_that_has_not_counted_leaves_the_count_true() {
    let (_d, store, writer) = a_writer_that_has_not_counted();
    assert_eq!(
        writer
            .purge_entities("prod", Some("host"))
            .unwrap()
            .entities,
        3
    );
    let held = held_is_stored(&writer, &store, "a purge of the hosts of prod");
    assert_eq!(held["prod"], 1, "the service stays");
    assert_eq!(held["dev"], 1);
    assert_eq!(total(&held), 2);
}

#[test]
fn a_prune_by_a_writer_that_has_not_counted_leaves_the_count_true() {
    let (_d, store, writer) = a_writer_that_has_not_counted();
    assert_eq!(writer.prune_entities(i64::MAX, 10).unwrap(), 1);
    let held = held_is_stored(&writer, &store, "a prune");
    assert_eq!(held["prod"], 3, "the gone host went");
    assert_eq!(held["dev"], 1);
    assert_eq!(total(&held), 4);
}

/// The single writer serves all telemetry, so what a batch costs must not
/// grow with its events: 5 000 of them are a handful of statements,
/// whatever they do to the rows (open them, refresh them, supersede them
/// or delete them). A statement per event would be thousands.
#[test]
fn a_batch_of_five_thousand_events_is_a_handful_of_statements() {
    const STATEMENTS: u64 = 40;
    let (_d, store) = fixture();
    let writer = store.writer().unwrap();
    let limits = EntityLimits::default();
    let batch = |ts_ns: i64, n: i64| -> Vec<EntityEventRow> {
        (0..5_000)
            .map(|i| changed(host(&format!("h-{i}"), ts_ns - n), n))
            .collect()
    };
    let deletes: Vec<EntityEventRow> = batch(PAST + 3 * SECOND, 1)
        .into_iter()
        .map(|event| EntityEventRow {
            kind: EntityEventKind::Delete,
            ..event
        })
        .collect();
    let steps = [
        ("opened", batch(PAST, 0), EntityOutcome::Opened),
        (
            "refreshed",
            batch(PAST + SECOND, 0),
            EntityOutcome::Refreshed,
        ),
        (
            "superseded",
            batch(PAST + 2 * SECOND, 1),
            EntityOutcome::Opened,
        ),
        ("deleted", deletes, EntityOutcome::Tombstoned),
    ];
    for (what, events, expected) in steps {
        let before = writer.entity_statements();
        let applied = apply(&writer, &events, &limits);
        let ran = writer.entity_statements() - before;
        assert!(
            applied.outcomes.iter().all(|outcome| *outcome == expected),
            "{what}"
        );
        assert!(ran > 0, "{what}: the statements are counted");
        assert!(
            ran <= STATEMENTS,
            "{what}: 5000 events ran {ran} statements"
        );
    }
    assert_eq!(writer.entity_version_count().unwrap(), 10_000);
    let gone = store
        .read_only()
        .unwrap()
        .query_json_rows(
            "SELECT count(*) AS n, min(last_seen_ns) AS seen FROM entities_current \
             WHERE liveness = 'gone' AND delete_reason IS NULL",
        )
        .unwrap();
    assert_eq!(gone[0]["n"], 5_000);
    assert_eq!(gone[0]["seen"], PAST + 2 * SECOND);
}

/// An export that names one unknown entity costs the same few statements
/// whether the table holds two entities or thousands: the entities are
/// not counted again.
#[test]
fn an_unknown_entity_costs_the_same_statements_whatever_the_table_holds() {
    let (_d, store) = fixture();
    let writer = store.writer().unwrap();
    let limits = EntityLimits::default();
    let cost = |id: &str| {
        let before = writer.entity_statements();
        apply(&writer, &[live(id)], &limits);
        writer.entity_statements() - before
    };
    let first = cost("first");
    let second = cost("second");
    assert!(second < first, "the first export counted the table");
    let many: Vec<EntityEventRow> = (0..3_000).map(|n| live(&format!("h-{n}"))).collect();
    apply(&writer, &many, &limits);
    assert_eq!(cost("third"), second);
}

/// The logs of an export and its entity events are stored together or not
/// at all, and a batch that failed leaves the connection fit for the next
/// one: the staging tables are temporary and were written in the
/// transaction that was rolled back.
#[test]
fn a_failed_fold_leaves_no_log_rows_and_the_next_batch_is_stored() {
    let (_d, store) = fixture();
    let writer = store.writer().unwrap();
    let limits = EntityLimits::default();
    let log = LogRow {
        ts_ns: PAST,
        body: "plain".into(),
        ..LogRow::default()
    };
    let events = [live("a"), changed(live("a"), 1), live("b")];
    apply(&writer, &[live("a")], &limits);

    writer.execute(BREAK_INSERT, []).unwrap();
    let failed = writer.insert_logs_and_entity_events(std::slice::from_ref(&log), &events, &limits);
    assert!(failed.is_err());
    writer.execute(MEND_INSERT, []).unwrap();
    assert_eq!(count(&store, "logs"), 0, "the logs went with the fold");
    assert_eq!(count(&store, "entity_versions"), 1);

    let applied = writer
        .insert_logs_and_entity_events(&[log], &events, &limits)
        .unwrap();
    assert_eq!(
        applied.outcomes,
        vec![
            EntityOutcome::Refreshed,
            EntityOutcome::Opened,
            EntityOutcome::Opened
        ]
    );
    assert_eq!(count(&store, "logs"), 1);
    assert_eq!(count(&store, "entity_versions"), 3);
}
