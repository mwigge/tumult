//! Daemon-run storage tests (moved out of `src/runs.rs`): registry
//! round-trips, the run state machine with its audit trail, active-run
//! reconciliation joins, and the v4 → v5 index-free migration.

#![cfg(feature = "duckdb")]

use tumult_lake::{
    rollback_status, run_state, NewRun, RegisteredDefinition, Store, CURRENT_SCHEMA_VERSION,
};

fn fixture() -> (tempfile::TempDir, Store) {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    (d, store)
}

fn def(id: &str, hash: &str) -> RegisteredDefinition {
    RegisteredDefinition {
        id: id.into(),
        name: "latency drill".into(),
        definition_toon: "title: latency drill".into(),
        content_hash: hash.into(),
        registered_at_ns: 1,
        registered_by: Some("test".into()),
    }
}

#[test]
fn registry_roundtrip_and_hash_dedup_lookup() {
    let (_d, store) = fixture();
    let writer = store.writer().unwrap();
    writer.register_definition(&def("reg-1", "hash-1")).unwrap();

    let reader = store.read_only().unwrap();
    let by_id = reader.registry_definition("reg-1").unwrap().unwrap();
    assert_eq!(by_id.definition_toon, "title: latency drill");
    let by_hash = reader.registry_by_hash("hash-1").unwrap().unwrap();
    assert_eq!(by_hash.id, "reg-1");
    assert!(reader.registry_by_hash("nope").unwrap().is_none());
}

#[test]
fn run_state_machine_and_audit_trail() {
    let (_d, store) = fixture();
    let writer = store.writer().unwrap();
    writer.register_definition(&def("reg-1", "hash-1")).unwrap();
    writer
        .insert_run(&NewRun {
            id: "run-1".into(),
            registry_id: "reg-1".into(),
            params_json: Some(r#"{"env":"staging"}"#.into()),
            queued_at_ns: 10,
            actor: Some("alice".into()),
        })
        .unwrap();
    writer
        .set_run_state("run-1", run_state::VALIDATING)
        .unwrap();
    writer.mark_run_started("run-1", None).unwrap();
    writer
        .set_run_state_with(
            "run-1",
            run_state::STOPPING,
            Some("stop_requested"),
            None,
            Some("alice"),
        )
        .unwrap();
    writer
        .finish_run(
            "run-1",
            run_state::ABORTED,
            Some("exp-1"),
            Some(rollback_status::COMPLETED),
            None,
        )
        .unwrap();

    let reader = store.read_only().unwrap();
    let run = reader.run_get("run-1").unwrap().unwrap();
    assert_eq!(run["state"], serde_json::json!("aborted"));
    assert_eq!(run["experiment_id"], serde_json::json!("exp-1"));
    assert_eq!(run["definition_name"], serde_json::json!("latency drill"));
    assert_eq!(
        run["rollback_status"],
        serde_json::json!(rollback_status::COMPLETED)
    );
    assert!(run["started_at_ns"].as_i64().unwrap() > 0);
    assert!(run["ended_at_ns"].as_i64().unwrap() > 0);

    let audit = reader.run_audit_trail("run-1").unwrap();
    let events: Vec<&str> = audit.iter().filter_map(|e| e["event"].as_str()).collect();
    assert_eq!(
        events,
        [
            "enqueued",
            "validating",
            "started",
            "stop_requested",
            "aborted"
        ]
    );
    // The user-initiated transitions carry the actor; system events don't.
    let by_event = |e: &str| audit.iter().find(|r| r["event"] == e).unwrap();
    assert_eq!(by_event("enqueued")["actor"], serde_json::json!("alice"));
    assert_eq!(
        by_event("stop_requested")["actor"],
        serde_json::json!("alice")
    );
    assert!(by_event("started")["actor"].is_null());

    // Active listing is empty for a terminal run; runs() lists it.
    assert!(reader.active_runs().unwrap().is_empty());
    assert_eq!(reader.runs(None, 10).unwrap().len(), 1);
    assert_eq!(reader.runs(Some(run_state::ABORTED), 10).unwrap().len(), 1);
    assert!(reader
        .runs(Some(run_state::RUNNING), 10)
        .unwrap()
        .is_empty());
}

#[test]
fn active_runs_joins_definition_for_reconciliation() {
    let (_d, store) = fixture();
    let writer = store.writer().unwrap();
    writer.register_definition(&def("reg-1", "hash-1")).unwrap();
    writer
        .insert_run(&NewRun {
            id: "run-9".into(),
            registry_id: "reg-1".into(),
            params_json: None,
            queued_at_ns: 5,
            actor: None,
        })
        .unwrap();
    writer.mark_run_started("run-9", Some("exp-9")).unwrap();

    let reader = store.read_only().unwrap();
    let active = reader.active_runs().unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0]["id"], serde_json::json!("run-9"));
    assert_eq!(active[0]["state"], serde_json::json!("running"));
    assert_eq!(
        active[0]["definition_toon"],
        serde_json::json!("title: latency drill")
    );
}

/// The v4 run tables (primary keys + secondary indexes) as a raw DDL
/// fixture, so the v4 → v5 rebuild can be exercised end to end.
const V4_RUN_TABLE_DDL: &str = "
CREATE TABLE schema_meta (key VARCHAR PRIMARY KEY, value BIGINT NOT NULL);
INSERT INTO schema_meta (key, value) VALUES ('version', 4);
CREATE TABLE run_registry (
    id               VARCHAR PRIMARY KEY,
    name             VARCHAR NOT NULL,
    definition_toon  VARCHAR NOT NULL,
    content_hash     VARCHAR NOT NULL,
    registered_at_ns BIGINT NOT NULL,
    registered_by    VARCHAR
);
CREATE INDEX idx_run_registry_hash ON run_registry (content_hash);
CREATE TABLE runs (
    id              VARCHAR PRIMARY KEY,
    registry_id     VARCHAR NOT NULL,
    state           VARCHAR NOT NULL,
    params_json     JSON,
    experiment_id   VARCHAR,
    rollback_status VARCHAR,
    error           VARCHAR,
    queued_at_ns    BIGINT NOT NULL,
    started_at_ns   BIGINT,
    ended_at_ns     BIGINT
);
CREATE INDEX idx_runs_state ON runs (state);
CREATE INDEX idx_runs_registry ON runs (registry_id);
CREATE TABLE run_audit (
    run_id  VARCHAR NOT NULL,
    at_ns   BIGINT NOT NULL,
    event   VARCHAR NOT NULL,
    detail  VARCHAR
);
CREATE INDEX idx_run_audit_run ON run_audit (run_id, at_ns);
";

#[test]
fn v4_store_migrates_to_index_free_run_tables() {
    let d = tempfile::TempDir::new().unwrap();
    let db = d.path().join("kronika.duckdb");
    // Build a v4-era store with a raw connection (Store::open would
    // migrate immediately).
    {
        let conn = duckdb::Connection::open(&db).unwrap();
        conn.execute_batch(V4_RUN_TABLE_DDL).unwrap();
        conn.execute_batch(
            "INSERT INTO run_registry VALUES ('reg-old','old exp','title: old','h',1,NULL);
                 INSERT INTO runs (id, registry_id, state, queued_at_ns) \
                 VALUES ('run-old', 'reg-old', 'running', 1);
                 INSERT INTO run_audit VALUES ('run-old', 1, 'enqueued', NULL);",
        )
        .unwrap();
    }

    let store = Store::open(&db).unwrap();
    let writer = store.writer().unwrap();
    assert_eq!(writer.schema_version().unwrap(), CURRENT_SCHEMA_VERSION);

    // Data survived the rebuild…
    let reader = store.read_only().unwrap();
    let run = reader.run_get("run-old").unwrap().unwrap();
    assert_eq!(run["state"], serde_json::json!("running"));
    assert_eq!(
        reader.registry_definition("reg-old").unwrap().unwrap().name,
        "old exp"
    );
    assert_eq!(reader.run_audit_trail("run-old").unwrap().len(), 1);

    // …and — the whole point — UPDATEs work without any ART index.
    writer
        .set_run_state("run-old", run_state::ORPHANED)
        .unwrap();
    let reader = store.read_only().unwrap();
    let run = reader.run_get("run-old").unwrap().unwrap();
    assert_eq!(run["state"], serde_json::json!("orphaned"));

    // No indexes remain on the run tables.
    let index_rows = reader
        .query_json_rows(
            "SELECT index_name, table_name FROM duckdb_indexes() \
             WHERE table_name IN ('runs', 'run_registry', 'run_audit')",
        )
        .unwrap();
    assert!(index_rows.is_empty(), "{index_rows:?}");

    // Re-opening is a no-op (version already current, no rebuild attempted).
    drop(store);
    let store = Store::open(&db).unwrap();
    let writer = store.writer().unwrap();
    assert_eq!(writer.schema_version().unwrap(), CURRENT_SCHEMA_VERSION);
    let reader = store.read_only().unwrap();
    assert!(reader.run_get("run-old").unwrap().is_some());
}
