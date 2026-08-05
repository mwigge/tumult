//! Run-retention sweep tests: terminal runs (and their audit trails) older
//! than the cutoff are deleted; active runs and recent terminal runs are
//! kept.

use tumult_ingest::{Batch, IngestWriter};
use tumult_lake::{NewRun, Store};

const DAY_NS: i64 = 86_400 * 1_000_000_000;

fn now_ns() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as i64)
}

/// Seed three runs: an old terminal run (finished 120 days ago), a recent
/// terminal run, and a still-active (queued) run — each via the normal
/// writer path, with the old run's timestamps backdated directly.
async fn seed(ingest: &IngestWriter) {
    ingest
        .write(Batch::Exec(Box::new(move |writer| {
            for id in ["run-old", "run-recent", "run-active"] {
                writer
                    .insert_run(&NewRun {
                        id: id.into(),
                        registry_id: "reg-1".into(),
                        params_json: None,
                        queued_at_ns: 1,
                        actor: None,
                    })
                    .map_err(|e| e.to_string())?;
            }
            writer
                .finish_run("run-old", "passed", None, Some("not_needed"), None)
                .map_err(|e| e.to_string())?;
            writer
                .finish_run("run-recent", "failed", None, Some("not_needed"), None)
                .map_err(|e| e.to_string())?;
            // Backdate the old run past the retention cutoff (finish_run
            // stamps now, so the age is set directly).
            let old = now_ns() - 120 * DAY_NS;
            writer
                .execute(
                    &format!(
                        "UPDATE runs SET ended_at_ns = {old}, queued_at_ns = {old} \
                         WHERE id = 'run-old'"
                    ),
                    [],
                )
                .map_err(|e| e.to_string())?;
            writer
                .execute(
                    &format!("UPDATE run_audit SET at_ns = {old} WHERE run_id = 'run-old'"),
                    [],
                )
                .map_err(|e| e.to_string())?;
            Ok(())
        })))
        .await
        .unwrap();
}

#[tokio::test]
async fn sweep_deletes_old_terminal_runs_and_their_audit_trails() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("kronika.duckdb");
    let store = Store::open(&db_path).unwrap();
    let (ingest, _task) = IngestWriter::spawn(store.writer().unwrap(), 64);
    seed(&ingest).await;

    tumult_ingest::retention::sweep_expired_runs(&ingest, 90)
        .await
        .unwrap();

    let reader = store.read_only().unwrap();
    let runs = reader
        .query_json_rows("SELECT id FROM runs ORDER BY id")
        .unwrap();
    let ids: Vec<&str> = runs.iter().filter_map(|r| r["id"].as_str()).collect();
    assert_eq!(ids, ["run-active", "run-recent"], "{ids:?}");
    // The old run's audit trail went with it; the others' trails survive.
    let audit = reader
        .query_json_rows("SELECT run_id FROM run_audit WHERE run_id = 'run-old'")
        .unwrap();
    assert!(audit.is_empty(), "{audit:?}");
    let kept = reader
        .query_json_rows(
            "SELECT run_id FROM run_audit WHERE run_id IN ('run-recent', 'run-active')",
        )
        .unwrap();
    assert!(!kept.is_empty());

    // A second sweep is a no-op (nothing aged out since).
    tumult_ingest::retention::sweep_expired_runs(&ingest, 90)
        .await
        .unwrap();
    let runs = reader
        .query_json_rows("SELECT id FROM runs ORDER BY id")
        .unwrap();
    assert_eq!(runs.len(), 2);
}
