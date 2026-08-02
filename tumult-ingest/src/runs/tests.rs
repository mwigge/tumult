use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tumult_core::runner::{ActivityExecutor, ActivityOutcome};
use tumult_core::types::Activity;
use tumult_lake::{rollback_status, run_state, NewRun, RegisteredDefinition, Store};

use super::worker::{process, sweep_expired_approvals};
use super::*;
use crate::IngestWriter;

/// Three method steps plus one rollback, native providers (the test
/// executor intercepts everything regardless of provider).
const TEST_TOON: &str = r#"
title: queue test experiment
method[3]:
  - name: action-1
    activity_type: action
    provider:
      type: native
      plugin: test
      function: noop
  - name: action-2
    activity_type: action
    provider:
      type: native
      plugin: test
      function: noop
  - name: action-3
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
"#;

/// Records executed activity names; each execution sleeps `delay`.
struct RecordingExecutor {
    executed: Arc<Mutex<Vec<String>>>,
    delay: Duration,
}
impl ActivityExecutor for RecordingExecutor {
    fn execute(&self, activity: &Activity) -> ActivityOutcome {
        self.executed.lock().unwrap().push(activity.name.clone());
        std::thread::sleep(self.delay);
        ActivityOutcome {
            success: true,
            output: Some("ok".into()),
            error: None,
            duration_ms: 0,
        }
    }
}

fn recording_factory(executed: &Arc<Mutex<Vec<String>>>, delay: Duration) -> ExecutorFactory {
    let executed = Arc::clone(executed);
    Arc::new(move |_| {
        Arc::new(RecordingExecutor {
            executed: Arc::clone(&executed),
            delay,
        })
    })
}

struct Fixture {
    _tmp: tempfile::TempDir,
    db_path: PathBuf,
    ingest: IngestWriter,
    executed: Arc<Mutex<Vec<String>>>,
}

async fn fixture() -> Fixture {
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("kronika.duckdb");
    let store = Store::open(&db_path).unwrap();
    let (ingest, _task) = IngestWriter::spawn(store.writer().unwrap(), 64);
    let executed = Arc::new(Mutex::new(Vec::new()));
    // The registry write rides the same channel as production.
    exec_write(&ingest, move |writer| {
        writer
            .register_definition(&RegisteredDefinition {
                id: "reg-1".into(),
                name: "queue test experiment".into(),
                definition_toon: TEST_TOON.into(),
                content_hash: "hash-1".into(),
                registered_at_ns: 1,
                registered_by: Some("test".into()),
            })
            .map_err(|e| e.to_string())
    })
    .await
    .unwrap();
    Fixture {
        _tmp: tmp,
        db_path,
        ingest,
        executed,
    }
}

fn request() -> RunRequest {
    RunRequest {
        registry_id: "reg-1".into(),
        definition_toon: TEST_TOON.into(),
        vars: HashMap::new(),
        env: "dev".into(),
        target: None,
    }
}

/// Record an approval decision directly on the store (the API handler's
/// write, minus the HTTP layer).
async fn approve(fx: &Fixture, run_id: &str, approver: &str) {
    let id = run_id.to_string();
    let who = approver.to_string();
    exec_write(&fx.ingest, move |writer| {
        writer
            .insert_approval_decision(&tumult_lake::ApprovalDecision {
                run_id: id,
                approver: who,
                decision: tumult_lake::approvals::decision::APPROVED.into(),
                note: None,
                decided_at_ns: now_ns(),
            })
            .map_err(|e| e.to_string())
    })
    .await
    .unwrap();
}

/// Poll the run's state until `want` or timeout (5s).
async fn await_state(fx: &Fixture, run_id: &str, want: &str) -> String {
    for _ in 0..100 {
        if let Some(state) = read_run_state(&fx.db_path, run_id) {
            if state == want {
                return state;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("run {run_id} never reached state {want}");
}

async fn await_terminal(fx: &Fixture, run_id: &str) -> String {
    for _ in 0..100 {
        if let Some(state) = read_run_state(&fx.db_path, run_id) {
            if run_state::TERMINAL.contains(&state.as_str()) {
                return state;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("run {run_id} never reached a terminal state");
}

fn run_row(fx: &Fixture, run_id: &str) -> serde_json::Value {
    Store::at(&fx.db_path)
        .read_only()
        .unwrap()
        .run_get(run_id)
        .unwrap()
        .unwrap()
}

fn audit_events(fx: &Fixture, run_id: &str) -> Vec<String> {
    Store::at(&fx.db_path)
        .read_only()
        .unwrap()
        .run_audit_trail(run_id)
        .unwrap()
        .iter()
        .filter_map(|e| e["event"].as_str().map(str::to_string))
        .collect()
}

#[tokio::test]
async fn run_executes_to_passed_and_ingests_journal() {
    let fx = fixture().await;
    let queue = RunQueue::spawn(
        fx.ingest.clone(),
        fx.db_path.clone(),
        RunQueueConfig {
            concurrency: 1,
            queue_depth: 4,
            sweep_interval: Duration::from_secs(3600),
        },
        recording_factory(&fx.executed, Duration::from_millis(5)),
    );

    let run_id = queue
        .enqueue(request(), Some("tester".to_string()))
        .await
        .unwrap();
    assert_eq!(await_terminal(&fx, &run_id).await, run_state::PASSED);

    let run = run_row(&fx, &run_id);
    assert_eq!(
        run["rollback_status"],
        serde_json::json!(rollback_status::NOT_NEEDED)
    );
    // The enqueued audit event carries the enqueueing actor; the
    // system-driven transitions (started, passed) carry none.
    let trail = Store::at(&fx.db_path)
        .read_only()
        .unwrap()
        .run_audit_trail(&run_id)
        .unwrap();
    let by_event = |e: &str| trail.iter().find(|r| r["event"] == e).unwrap();
    assert_eq!(by_event("enqueued")["actor"], serde_json::json!("tester"));
    assert!(by_event("passed")["actor"].is_null());
    let experiment_id = run["experiment_id"].as_str().unwrap();
    assert!(!experiment_id.is_empty());
    assert_eq!(
        fx.executed.lock().unwrap().as_slice(),
        ["action-1", "action-2", "action-3"]
    );
    // The journal landed in the analytics tables via the same writer.
    let rows = Store::at(&fx.db_path)
        .read_only()
        .unwrap()
        .query_json_rows("SELECT experiment_id, status FROM experiments")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["experiment_id"], serde_json::json!(experiment_id));
}

#[tokio::test]
async fn enqueue_rejects_beyond_queue_depth() {
    let fx = fixture().await;
    let queue = RunQueue::spawn(
        fx.ingest.clone(),
        fx.db_path.clone(),
        RunQueueConfig {
            concurrency: 1,
            queue_depth: 1,
            sweep_interval: Duration::from_secs(3600),
        },
        // Slow steps on purpose: r3 must be rejected while r2 still holds
        // the waiting permit, i.e. before r1 finishes — a wall-clock window
        // that tarpaulin's instrumentation can stretch past short steps.
        recording_factory(&fx.executed, Duration::from_millis(2000)),
    );

    let r1 = queue.enqueue(request(), None).await.unwrap();
    await_state(&fx, &r1, run_state::RUNNING).await;
    // r2 takes the only waiting permit; r3 must be rejected, not queued.
    let r2 = queue.enqueue(request(), None).await.unwrap();
    assert!(matches!(
        queue.enqueue(request(), None).await,
        Err(EnqueueError::Full)
    ));

    // Only the two accepted runs were persisted.
    let runs = Store::at(&fx.db_path)
        .read_only()
        .unwrap()
        .runs(None, 10)
        .unwrap();
    assert_eq!(runs.len(), 2);

    assert_eq!(await_terminal(&fx, &r1).await, run_state::PASSED);
    assert_eq!(await_terminal(&fx, &r2).await, run_state::PASSED);
}

#[tokio::test]
async fn stop_mid_method_runs_rollback_and_aborts() {
    let fx = fixture().await;
    let queue = RunQueue::spawn(
        fx.ingest.clone(),
        fx.db_path.clone(),
        RunQueueConfig {
            concurrency: 1,
            queue_depth: 4,
            sweep_interval: Duration::from_secs(3600),
        },
        // Slow steps on purpose: the e-stop must land mid-method — a
        // wall-clock window that tarpaulin's instrumentation can stretch
        // past short steps.
        recording_factory(&fx.executed, Duration::from_millis(2000)),
    );

    let run_id = queue.enqueue(request(), None).await.unwrap();
    // Wait until the first activity finished (second is sleeping).
    for _ in 0..100 {
        if !fx.executed.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    queue.stop(&run_id, Some("tester")).await.unwrap();
    assert_eq!(await_terminal(&fx, &run_id).await, run_state::ABORTED);

    let events = audit_events(&fx, &run_id);
    assert!(events.contains(&"stop_requested".to_string()), "{events:?}");
    // The stop_requested audit event carries the requesting actor.
    let trail = Store::at(&fx.db_path)
        .read_only()
        .unwrap()
        .run_audit_trail(&run_id)
        .unwrap();
    let stop_event = trail
        .iter()
        .find(|r| r["event"] == "stop_requested")
        .unwrap();
    assert_eq!(stop_event["actor"], serde_json::json!("tester"));
    // The e-stop unwound the active fault via the rollback path.
    let executed = fx.executed.lock().unwrap();
    assert!(executed.contains(&"rollback-1".to_string()), "{executed:?}");
    // …and never ran the final method step.
    assert!(!executed.contains(&"action-3".to_string()), "{executed:?}");
    let run = run_row(&fx, &run_id);
    assert_eq!(
        run["rollback_status"],
        serde_json::json!(rollback_status::COMPLETED)
    );
}

#[tokio::test]
async fn stop_queued_run_cancels_before_start() {
    let fx = fixture().await;
    let queue = RunQueue::spawn(
        fx.ingest.clone(),
        fx.db_path.clone(),
        RunQueueConfig {
            concurrency: 1,
            queue_depth: 4,
            sweep_interval: Duration::from_secs(3600),
        },
        // Slow steps on purpose: the test must observe r1 RUNNING and then
        // cancel r2 while r1 is still going. That window is wall-clock —
        // under tarpaulin's instrumentation the test's own code path can
        // take several seconds, so short steps here let r1 finish and r2
        // run to completion before the stop lands (CI flake).
        recording_factory(&fx.executed, Duration::from_millis(2000)),
    );

    let r1 = queue.enqueue(request(), None).await.unwrap();
    await_state(&fx, &r1, run_state::RUNNING).await;
    let r2 = queue.enqueue(request(), None).await.unwrap();
    queue.stop(&r2, None).await.unwrap();

    assert_eq!(await_terminal(&fx, &r2).await, run_state::ABORTED);
    let run = run_row(&fx, &r2);
    assert_eq!(run["error"], serde_json::json!("cancelled before start"));
    assert_eq!(await_terminal(&fx, &r1).await, run_state::PASSED);
    // r2 never executed anything: only r1's three method steps ran.
    assert_eq!(fx.executed.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn stop_unknown_or_terminal_run_errors() {
    let fx = fixture().await;
    let queue = RunQueue::spawn(
        fx.ingest.clone(),
        fx.db_path.clone(),
        RunQueueConfig {
            concurrency: 1,
            queue_depth: 4,
            sweep_interval: Duration::from_secs(3600),
        },
        recording_factory(&fx.executed, Duration::from_millis(5)),
    );
    assert!(matches!(
        queue.stop("nope", None).await,
        Err(StopError::NotFound)
    ));

    let run_id = queue.enqueue(request(), None).await.unwrap();
    await_terminal(&fx, &run_id).await;
    assert!(matches!(
        queue.stop(&run_id, None).await,
        Err(StopError::Terminal(_))
    ));
}

#[tokio::test]
async fn orphan_reconciliation_rolls_back_and_audits() {
    let fx = fixture().await;
    // Simulate a crash: a run left `running` by a dead process.
    exec_write(&fx.ingest, move |writer| {
        writer
            .insert_run(&NewRun {
                id: "run-orphan".into(),
                registry_id: "reg-1".into(),
                params_json: None,
                queued_at_ns: 1,
                actor: None,
            })
            .map_err(|e| e.to_string())
    })
    .await
    .unwrap();
    exec_write(&fx.ingest, move |writer| {
        writer
            .mark_run_started("run-orphan", None)
            .map_err(|e| e.to_string())
    })
    .await
    .unwrap();

    let factory = recording_factory(&fx.executed, Duration::from_millis(5));
    let count = reconcile_orphans(&fx.ingest, &fx.db_path, &factory)
        .await
        .unwrap();
    assert_eq!(count, 1);

    let run = run_row(&fx, "run-orphan");
    assert_eq!(run["state"], serde_json::json!(run_state::ABORTED));
    assert_eq!(
        run["rollback_status"],
        serde_json::json!(rollback_status::COMPLETED)
    );
    // Only the rollback executed — the orphaned method never re-ran.
    assert_eq!(fx.executed.lock().unwrap().as_slice(), ["rollback-1"]);
    let events = audit_events(&fx, "run-orphan");
    for want in ["orphan_detected", "rollback_started", "rollback_completed"] {
        assert!(events.contains(&want.to_string()), "{events:?}");
    }
}

#[tokio::test]
async fn orphan_never_started_aborts_without_rollback() {
    let fx = fixture().await;
    exec_write(&fx.ingest, move |writer| {
        writer
            .insert_run(&NewRun {
                id: "run-queued".into(),
                registry_id: "reg-1".into(),
                params_json: None,
                queued_at_ns: 1,
                actor: None,
            })
            .map_err(|e| e.to_string())
    })
    .await
    .unwrap();

    let factory = recording_factory(&fx.executed, Duration::from_millis(5));
    let count = reconcile_orphans(&fx.ingest, &fx.db_path, &factory)
        .await
        .unwrap();
    assert_eq!(count, 1);

    let run = run_row(&fx, "run-queued");
    assert_eq!(run["state"], serde_json::json!(run_state::ABORTED));
    assert_eq!(
        run["rollback_status"],
        serde_json::json!(rollback_status::NOT_NEEDED)
    );
    assert!(fx.executed.lock().unwrap().is_empty());
    let events = audit_events(&fx, "run-queued");
    assert!(events.contains(&"orphan_detected".to_string()));
    assert!(!events.contains(&"rollback_started".to_string()));
}

/// Insert a gated run directly on the store (bypassing `request_gated`'s
/// clock so TTL edge cases can be tested).
async fn insert_gated(fx: &Fixture, run_id: &str, expires_at_ns: i64) {
    let id = run_id.to_string();
    exec_write(&fx.ingest, move |writer| {
        let params = std::collections::BTreeMap::new();
        writer
            .insert_gated_run(
                &NewRun {
                    id: id.clone(),
                    registry_id: "reg-1".into(),
                    params_json: None,
                    queued_at_ns: now_ns(),
                    actor: Some("alice".into()),
                },
                &tumult_lake::ApprovalRequest {
                    run_id: id.clone(),
                    tier: "T1".into(),
                    pin_hash: tumult_lake::approval_pin(&tumult_lake::CanonicalPin {
                        definition_toon: TEST_TOON,
                        params: &params,
                        env: "dev",
                        target: None,
                    }),
                    env: "dev".into(),
                    target: None,
                    quorum_required: 1,
                    requested_by: "alice".into(),
                    requested_at_ns: now_ns(),
                    expires_at_ns,
                },
                Some("test gated run"),
            )
            .map_err(|e| e.to_string())
    })
    .await
    .unwrap();
}

fn shared(fx: &Fixture) -> Shared {
    Shared {
        db_path: fx.db_path.clone(),
        ingest: fx.ingest.clone(),
        tokens: Mutex::new(HashMap::new()),
        shutdown: CancellationToken::new(),
    }
}

#[tokio::test]
async fn gated_run_waits_for_quorum_then_executes_once() {
    let fx = fixture().await;
    let queue = RunQueue::spawn(
        fx.ingest.clone(),
        fx.db_path.clone(),
        RunQueueConfig {
            concurrency: 1,
            queue_depth: 4,
            sweep_interval: Duration::from_secs(3600),
        },
        recording_factory(&fx.executed, Duration::from_millis(5)),
    );

    let run_id = queue
        .request_gated(request(), crate::approvals::Tier::T3, Some("alice".into()))
        .await
        .unwrap();
    assert_eq!(
        read_run_state(&fx.db_path, &run_id).as_deref(),
        Some(run_state::PENDING_APPROVAL)
    );
    assert!(fx.executed.lock().unwrap().is_empty());

    // Quorum 2: one approval is not enough.
    approve(&fx, &run_id, "bob").await;
    assert!(matches!(
        queue.dispatch_approved(&run_id).await,
        Err(DispatchError::Approval(_))
    ));
    assert_eq!(
        read_run_state(&fx.db_path, &run_id).as_deref(),
        Some(run_state::PENDING_APPROVAL)
    );

    approve(&fx, &run_id, "carol").await;
    queue.dispatch_approved(&run_id).await.unwrap();
    assert_eq!(await_terminal(&fx, &run_id).await, run_state::PASSED);
    assert_eq!(
        fx.executed.lock().unwrap().as_slice(),
        ["action-1", "action-2", "action-3"]
    );

    // Single-use: the approval was consumed by the dispatch; the run is
    // no longer pending, so a second dispatch is refused.
    assert!(matches!(
        queue.dispatch_approved(&run_id).await,
        Err(DispatchError::NotPending)
    ));
    let req = Store::at(&fx.db_path)
        .read_only()
        .unwrap()
        .approval_request(&run_id)
        .unwrap()
        .unwrap();
    assert!(req["consumed_at_ns"].is_number(), "{req}");
    assert_eq!(req["quorum_required"], serde_json::json!(2));

    let events = audit_events(&fx, &run_id);
    for want in ["requested", "dispatch_queued", "consumed"] {
        assert!(events.contains(&want.to_string()), "{events:?}");
    }
    // The full trail verifies against the hash chain.
    assert!(Store::at(&fx.db_path)
        .read_only()
        .unwrap()
        .verify_run_audit_chain(&run_id)
        .unwrap());
}

#[tokio::test]
async fn dispatch_is_refused_when_content_changes_after_approval() {
    let fx = fixture().await;
    // A queued run whose approved pin does not match the content the
    // worker is handed (definition/params/env edited after approval).
    exec_write(&fx.ingest, move |writer| {
        writer
            .insert_run(&NewRun {
                id: "run-tampered".into(),
                registry_id: "reg-1".into(),
                params_json: None,
                queued_at_ns: now_ns(),
                actor: Some("alice".into()),
            })
            .map_err(|e| e.to_string())
    })
    .await
    .unwrap();
    let semaphore = Arc::new(Semaphore::new(1));
    let permit = semaphore.try_acquire_owned().unwrap();
    let item = WorkItem {
        run_id: "run-tampered".into(),
        request: request(),
        approval_pin: Some("0".repeat(64)),
        _permit: permit,
    };
    let factory = recording_factory(&fx.executed, Duration::from_millis(5));
    process(item, &shared(&fx), &factory).await;

    let run = run_row(&fx, "run-tampered");
    assert_eq!(run["state"], serde_json::json!(run_state::FAILED));
    assert!(
        run["error"].as_str().unwrap().contains("pin mismatch"),
        "{}",
        run["error"]
    );
    let events = audit_events(&fx, "run-tampered");
    assert!(
        events.contains(&"dispatch_refused".to_string()),
        "{events:?}"
    );
    assert!(fx.executed.lock().unwrap().is_empty());
}

#[tokio::test]
async fn expired_approval_refuses_dispatch_and_sweeps_terminal() {
    let fx = fixture().await;
    let queue = RunQueue::spawn(
        fx.ingest.clone(),
        fx.db_path.clone(),
        RunQueueConfig {
            concurrency: 1,
            queue_depth: 4,
            sweep_interval: Duration::from_secs(3600),
        },
        recording_factory(&fx.executed, Duration::from_millis(5)),
    );

    // TTL already lapsed at request time.
    insert_gated(&fx, "run-stale", now_ns() - 1).await;
    approve(&fx, "run-stale", "bob").await;
    let err = queue.dispatch_approved("run-stale").await.unwrap_err();
    assert!(
        matches!(&err, DispatchError::Approval(r) if r.contains("expired")),
        "{err:?}"
    );

    // The sweeper flips it terminal.
    sweep_expired_approvals(&shared(&fx)).await;
    let run = run_row(&fx, "run-stale");
    assert_eq!(run["state"], serde_json::json!(run_state::EXPIRED));
    let events = audit_events(&fx, "run-stale");
    assert!(events.contains(&"expired".to_string()), "{events:?}");
    assert!(fx.executed.lock().unwrap().is_empty());
}

#[tokio::test]
async fn break_glass_bypasses_quorum_and_ttl_but_not_the_pin() {
    let fx = fixture().await;
    let queue = RunQueue::spawn(
        fx.ingest.clone(),
        fx.db_path.clone(),
        RunQueueConfig {
            concurrency: 1,
            queue_depth: 4,
            sweep_interval: Duration::from_secs(3600),
        },
        recording_factory(&fx.executed, Duration::from_millis(5)),
    );

    // Expired, zero approvals — but overridden by break-glass.
    insert_gated(&fx, "run-override", now_ns() - 1).await;
    exec_write(&fx.ingest, move |writer| {
        writer
            .mark_break_glass("run-override", "admin", "prod down; restore now")
            .map_err(|e| e.to_string())
    })
    .await
    .unwrap();
    queue.dispatch_approved("run-override").await.unwrap();
    assert_eq!(await_terminal(&fx, "run-override").await, run_state::PASSED);
    assert_eq!(fx.executed.lock().unwrap().len(), 3);
}
