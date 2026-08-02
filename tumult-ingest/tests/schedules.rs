//! Schedule scheduler tests: a due schedule fires a run through the normal
//! run path — tier classification and approval gating preserved — and a
//! disabled schedule never fires.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tumult_core::runner::{ActivityExecutor, ActivityOutcome};
use tumult_core::types::Activity;
use tumult_ingest::{IngestWriter, RunQueue, RunQueueConfig};
use tumult_lake::{RegisteredDefinition, ScheduleRow, Store};

/// Fault definition (actions + rollback): classifies T1 on the default
/// "dev" env, so a scheduled run must park in `pending_approval`.
const FAULT_TOON: &str = "
title: scheduled fault experiment
method[1]:
  - name: action-1
    activity_type: action
    provider:
      type: native
      plugin: test
      function: noop
rollbacks[1]:
  - name: rollback-1
    activity_type: action
    provider:
      type: native
      plugin: test
      function: noop
";

/// Probe-only definition: classifies T0, so a scheduled run enqueues and
/// executes directly.
const PROBE_TOON: &str = "
title: scheduled probe experiment
method[1]:
  - name: check-1
    activity_type: probe
    provider:
      type: native
      plugin: test
      function: noop
";

struct NoopExecutor;
impl ActivityExecutor for NoopExecutor {
    fn execute(&self, _activity: &Activity) -> ActivityOutcome {
        ActivityOutcome {
            success: true,
            output: Some("ok".into()),
            error: None,
            duration_ms: 0,
        }
    }
}

struct Fixture {
    _tmp: tempfile::TempDir,
    db_path: PathBuf,
    ingest: IngestWriter,
    runs: RunQueue,
}

async fn fixture() -> Fixture {
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("kronika.duckdb");
    let store = Store::open(&db_path).unwrap();
    let (ingest, _task) = IngestWriter::spawn(store.writer().unwrap(), 64);
    let runs = RunQueue::spawn(
        ingest.clone(),
        db_path.clone(),
        RunQueueConfig::default(),
        Arc::new(|_| Arc::new(NoopExecutor) as Arc<dyn ActivityExecutor>),
    );
    for (id, name, toon) in [
        ("reg-fault", "scheduled fault experiment", FAULT_TOON),
        ("reg-probe", "scheduled probe experiment", PROBE_TOON),
    ] {
        let def = RegisteredDefinition {
            id: id.into(),
            name: name.into(),
            definition_toon: toon.into(),
            content_hash: format!("hash-{id}"),
            registered_at_ns: 1,
            registered_by: Some("test".into()),
        };
        let ingest = ingest.clone();
        ingest
            .write(tumult_ingest::Batch::Exec(Box::new(move |writer| {
                writer.register_definition(&def).map_err(|e| e.to_string())
            })))
            .await
            .unwrap();
    }
    Fixture {
        _tmp: tmp,
        db_path,
        ingest,
        runs,
    }
}

fn schedule(id: &str, registry_id: &str, enabled: bool, next_run_at_ns: i64) -> ScheduleRow {
    ScheduleRow {
        id: id.into(),
        name: id.into(),
        registry_id: registry_id.into(),
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

async fn create_schedule(fx: &Fixture, row: ScheduleRow) {
    fx.ingest
        .write(tumult_ingest::Batch::Exec(Box::new(move |writer| {
            writer.create_schedule(&row).map_err(|e| e.to_string())
        })))
        .await
        .unwrap();
}

#[tokio::test]
async fn due_schedule_fires_through_the_normal_run_path() {
    let fx = fixture().await;
    create_schedule(&fx, schedule("s-fault", "reg-fault", true, 1)).await;
    create_schedule(&fx, schedule("s-probe", "reg-probe", true, 1)).await;
    create_schedule(&fx, schedule("s-off", "reg-probe", false, 1)).await;

    let fired = tumult_ingest::schedules::fire_due_schedules(&fx.db_path, &fx.ingest, &fx.runs)
        .await
        .unwrap();
    assert_eq!(fired, 2, "the disabled schedule is skipped");

    let reader = Store::at(&fx.db_path).read_only().unwrap();
    let runs = reader.runs(None, 10).unwrap();
    assert_eq!(runs.len(), 2, "{runs:?}");

    // The fault run is gated exactly like a manual POST /api/runs: parked in
    // pending_approval, requested by the schedule.
    let fault = runs
        .iter()
        .find(|r| r["registry_id"] == "reg-fault")
        .unwrap();
    assert_eq!(fault["state"], "pending_approval", "{runs:?}");
    let audit = reader
        .run_audit_trail(fault["id"].as_str().unwrap())
        .unwrap();
    let requested = audit
        .iter()
        .find(|e| e["event"] == "requested")
        .expect("gated run records a requested event");
    assert_eq!(requested["actor"], "schedule:s-fault");

    // The probe run classifies T0 and executes to passed.
    let probe = runs
        .iter()
        .find(|r| r["registry_id"] == "reg-probe")
        .unwrap();
    assert_ne!(probe["state"], "pending_approval", "{runs:?}");
    let probe_id = probe["id"].as_str().unwrap().to_string();
    let mut terminal = String::new();
    for _ in 0..100 {
        let r = Store::at(&fx.db_path).read_only().unwrap();
        let state = r
            .run_get(&probe_id)
            .unwrap()
            .map(|run| run["state"].as_str().unwrap_or_default().to_string())
            .unwrap_or_default();
        if ["passed", "failed", "deviated", "aborted"].contains(&state.as_str()) {
            terminal = state;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(terminal, "passed");

    // Fire bookkeeping: both fired schedules advanced; the disabled one did not.
    let schedules = reader.list_schedules().unwrap();
    let fault_s = schedules.iter().find(|s| s.id == "s-fault").unwrap();
    assert_eq!(fault_s.last_run_id.as_deref(), fault["id"].as_str());
    assert!(fault_s.next_run_at_ns > 1);
    let off = schedules.iter().find(|s| s.id == "s-off").unwrap();
    assert!(off.last_run_id.is_none());
    assert_eq!(off.next_run_at_ns, 1);
}
