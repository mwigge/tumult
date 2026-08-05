//! Schedule storage tests: the v10 `run_schedules` table — CRUD roundtrip,
//! the due-schedule selection (enabled and due), and fire bookkeeping.

#![cfg(feature = "duckdb")]

use tumult_lake::{ScheduleRow, Store, CURRENT_SCHEMA_VERSION};

fn fixture() -> (tempfile::TempDir, Store) {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    (d, store)
}

fn schedule(id: &str, enabled: bool, next_run_at_ns: i64) -> ScheduleRow {
    ScheduleRow {
        id: id.into(),
        name: format!("schedule {id}"),
        registry_id: "reg-1".into(),
        interval_s: 3_600,
        vars_json: None,
        env: "dev".into(),
        target: None,
        enabled,
        next_run_at_ns,
        last_run_at_ns: None,
        last_run_id: None,
        created_by: Some("test".into()),
        created_at_ns: 1,
    }
}

#[test]
fn schema_is_v13() {
    let (_d, store) = fixture();
    assert_eq!(
        store.writer().unwrap().schema_version().unwrap(),
        CURRENT_SCHEMA_VERSION
    );
    assert_eq!(CURRENT_SCHEMA_VERSION, 13);
}

#[test]
fn schedule_crud_and_due_selection() {
    let (_d, store) = fixture();
    let writer = store.writer().unwrap();

    writer
        .create_schedule(&schedule("s-due", true, 50))
        .unwrap();
    writer
        .create_schedule(&schedule("s-future", true, 1_000))
        .unwrap();
    writer
        .create_schedule(&schedule("s-off", false, 10))
        .unwrap();

    let all = store.read_only().unwrap().list_schedules().unwrap();
    assert_eq!(all.len(), 3);
    let s = all.iter().find(|s| s.id == "s-due").unwrap();
    assert_eq!(s.name, "schedule s-due");
    assert_eq!(s.interval_s, 3_600);
    assert_eq!(s.env, "dev");
    assert!(s.enabled);
    assert!(s.last_run_at_ns.is_none());

    // Due = enabled and next_run_at_ns <= now.
    let due = store.read_only().unwrap().due_schedules(100).unwrap();
    let ids: Vec<&str> = due.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["s-due"], "future and disabled schedules are not due");

    // Fire bookkeeping: last run + advanced next run.
    writer
        .schedule_fired("s-due", Some("run-1"), 100, 3_600_100)
        .unwrap();
    let s = store
        .read_only()
        .unwrap()
        .list_schedules()
        .unwrap()
        .into_iter()
        .find(|s| s.id == "s-due")
        .unwrap();
    assert_eq!(s.last_run_at_ns, Some(100));
    assert_eq!(s.last_run_id.as_deref(), Some("run-1"));
    assert_eq!(s.next_run_at_ns, 3_600_100);

    // Disable flips the flag; delete removes the row.
    writer.set_schedule_enabled("s-due", false).unwrap();
    assert!(store
        .read_only()
        .unwrap()
        .due_schedules(200)
        .unwrap()
        .is_empty());
    writer.delete_schedule("s-due").unwrap();
    assert_eq!(
        store.read_only().unwrap().list_schedules().unwrap().len(),
        2
    );
}
