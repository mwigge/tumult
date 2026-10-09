//! Run-retention sweep tests: terminal runs (and their audit trails) older
//! than the cutoff are deleted; active runs and recent terminal runs are
//! kept.

use tumult_ingest::{Batch, IngestWriter};
use tumult_lake::{NewRun, Store};

const DAY_NS: i64 = 86_400 * 1_000_000_000;

fn now_ns() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(0))
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
async fn retention_refuses_to_destroy_report_and_audit_history() {
    let tmp = tempfile::TempDir::new().unwrap();
    let store = Store::open(&tmp.path().join("db")).unwrap();
    let (ingest, _task) = IngestWriter::spawn(store.writer().unwrap(), 64);
    seed(&ingest).await;
    let error = tumult_ingest::retention::sweep_expired_runs(&ingest, 90)
        .await
        .unwrap_err();
    assert!(error.contains("historical"), "{error}");
    tumult_ingest::retention::sweep_expired_runs(&ingest, 0)
        .await
        .unwrap();
    let reader = store.read_only().unwrap();
    assert_eq!(
        reader
            .query_json_rows("SELECT count(*) AS n FROM runs")
            .unwrap()[0]["n"],
        3
    );
    assert!(!reader
        .query_json_rows("SELECT * FROM run_audit WHERE run_id = 'run-old'")
        .unwrap()
        .is_empty());
}
