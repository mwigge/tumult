//! `GameDay` supervisor tests: a campaign advances its experiments as
//! sequential child runs — each through the normal tier classification
//! (a fault step parks for approval) — and the parent run takes the
//! campaign outcome when the last child is terminal.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tumult_core::runner::{ActivityExecutor, ActivityOutcome};
use tumult_core::types::Activity;
use tumult_ingest::{IngestWriter, RunQueue, RunQueueConfig};
use tumult_lake::{ApprovalDecision, NewRun, RegisteredDefinition, Store};

const PROBE_TOON: &str = "
title: campaign probe
method[1]:
  - name: check-1
    activity_type: probe
    provider:
      type: native
      plugin: test
      function: noop
";

const FAULT_TOON: &str = "
title: campaign fault
method[1]:
  - name: inject-1
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

/// Like `NoopExecutor` but holds each activity for 500ms, so a run is still
/// executing when the test e-stops it.
struct SlowExecutor;
impl ActivityExecutor for SlowExecutor {
    fn execute(&self, _activity: &Activity) -> ActivityOutcome {
        std::thread::sleep(Duration::from_millis(500));
        ActivityOutcome {
            success: true,
            output: Some("ok".into()),
            error: None,
            duration_ms: 500,
        }
    }
}

struct Fixture {
    _tmp: tempfile::TempDir,
    db_path: std::path::PathBuf,
    ingest: IngestWriter,
    runs: RunQueue,
}

#[allow(clippy::unused_async)]
async fn fixture() -> Fixture {
    fixture_with(Arc::new(|_| {
        Arc::new(NoopExecutor) as Arc<dyn ActivityExecutor>
    }))
}

fn fixture_with(factory: tumult_ingest::runs::ExecutorFactory) -> Fixture {
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("kronika.duckdb");
    let store = Store::open(&db_path).unwrap();
    let (ingest, _task) = IngestWriter::spawn(store.writer().unwrap(), 64);
    let runs = RunQueue::spawn(
        ingest.clone(),
        db_path.clone(),
        RunQueueConfig::default(),
        factory,
    );
    Fixture {
        _tmp: tmp,
        db_path,
        ingest,
        runs,
    }
}

async fn write(
    fx: &Fixture,
    f: impl FnOnce(&tumult_lake::Writer) -> Result<(), String> + Send + 'static,
) {
    fx.ingest
        .write(tumult_ingest::Batch::Exec(Box::new(f)))
        .await
        .unwrap();
}

/// Register the two experiment definitions plus a gameday envelope
/// referencing them in order, and start the parent campaign run.
async fn seed_campaign(fx: &Fixture, steps: &[(&str, &str)]) {
    let defs: Vec<RegisteredDefinition> = steps
        .iter()
        .enumerate()
        .map(|(i, (name, toon))| RegisteredDefinition {
            id: format!("reg-step-{i}"),
            name: (*name).into(),
            definition_toon: (*toon).into(),
            content_hash: format!("hash-{i}"),
            registered_at_ns: 1,
            registered_by: Some("test".into()),
        })
        .collect();
    let step_json: Vec<Value> = defs
        .iter()
        .enumerate()
        .map(|(i, d)| json!({"path": format!("step-{i}.toon"), "registry_id": d.id}))
        .collect();
    write(fx, move |writer| {
        for def in &defs {
            writer.register_definition(def).map_err(|e| e.to_string())?;
        }
        writer
            .register_gameday_definition(&RegisteredDefinition {
                id: "gd-1".into(),
                name: "test campaign".into(),
                definition_toon: json!({
                    "toon": "title: test campaign",
                    "experiments": step_json,
                    "scoring": {"pass_threshold": 0.75},
                })
                .to_string(),
                content_hash: "hash-gd".into(),
                registered_at_ns: 1,
                registered_by: Some("test".into()),
            })
            .map_err(|e| e.to_string())?;
        writer
            .insert_run(&NewRun {
                id: "parent-1".into(),
                registry_id: "gd-1".into(),
                params_json: Some(json!({"env": "dev"}).to_string()),
                queued_at_ns: 1,
                actor: Some("tester".into()),
            })
            .map_err(|e| e.to_string())
    })
    .await;
}

/// Poll a run until it reaches a terminal state; returns its state.
async fn await_terminal(fx: &Fixture, run_id: &str) -> String {
    for _ in 0..200 {
        let reader = Store::at(&fx.db_path).read_only().unwrap();
        if let Some(run) = reader.run_get(run_id).unwrap() {
            let state = run["state"].as_str().unwrap_or_default().to_string();
            if [
                "passed", "deviated", "failed", "aborted", "orphaned", "rejected", "expired",
            ]
            .contains(&state.as_str())
            {
                return state;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("run {run_id} never reached a terminal state");
}

fn children_of(fx: &Fixture, parent: &str) -> Vec<Value> {
    let reader = Store::at(&fx.db_path).read_only().unwrap();
    reader
        .query_json_rows(&format!(
            "SELECT * FROM runs WHERE gameday_id = '{parent}' ORDER BY queued_at_ns"
        ))
        .unwrap()
}

#[tokio::test]
async fn campaign_advances_sequentially_and_parks_at_approvals() {
    let fx = fixture().await;
    seed_campaign(
        &fx,
        &[
            ("probe one", PROBE_TOON),
            ("fault two", FAULT_TOON),
            ("probe three", PROBE_TOON),
        ],
    )
    .await;

    // First advance: step 1 enqueues as a child and the parent goes running.
    let enqueued = tumult_ingest::gamedays::advance_campaigns(&fx.db_path, &fx.ingest, &fx.runs)
        .await
        .unwrap();
    assert_eq!(enqueued, 1);
    let kids = children_of(&fx, "parent-1");
    assert_eq!(kids.len(), 1);
    assert_eq!(kids[0]["registry_id"], "reg-step-0");
    assert_eq!(kids[0]["gameday_id"], "parent-1");
    let parent = Store::at(&fx.db_path)
        .read_only()
        .unwrap()
        .run_get("parent-1")
        .unwrap()
        .unwrap();
    assert_eq!(parent["state"], "running");

    // Step 1 passes; step 2 (fault) parks in pending_approval — the
    // campaign waits; a second advance adds nothing.
    let child1 = kids[0]["id"].as_str().unwrap().to_string();
    assert_eq!(await_terminal(&fx, &child1).await, "passed");
    let enqueued = tumult_ingest::gamedays::advance_campaigns(&fx.db_path, &fx.ingest, &fx.runs)
        .await
        .unwrap();
    assert_eq!(enqueued, 1);
    let kids = children_of(&fx, "parent-1");
    assert_eq!(kids.len(), 2);
    let child2 = kids[1]["id"].as_str().unwrap().to_string();
    assert_eq!(kids[1]["state"], "pending_approval");
    let enqueued = tumult_ingest::gamedays::advance_campaigns(&fx.db_path, &fx.ingest, &fx.runs)
        .await
        .unwrap();
    assert_eq!(enqueued, 0, "parked at the approval gate");
    assert_eq!(children_of(&fx, "parent-1").len(), 2);

    // Approve step 2; it runs to passed; step 3 follows; the parent takes
    // the campaign outcome once every child is terminal.
    let child2_for_approval = child2.clone();
    write(&fx, move |writer| {
        writer
            .insert_approval_decision(&ApprovalDecision {
                run_id: child2_for_approval,
                approver: "boss".into(),
                decision: "approved".into(),
                note: None,
                decided_at_ns: 1,
            })
            .map_err(|e| e.to_string())
    })
    .await;
    fx.runs.dispatch_approved(&child2).await.unwrap();
    assert_eq!(await_terminal(&fx, &child2).await, "passed");

    let enqueued = tumult_ingest::gamedays::advance_campaigns(&fx.db_path, &fx.ingest, &fx.runs)
        .await
        .unwrap();
    assert_eq!(enqueued, 1);
    let kids = children_of(&fx, "parent-1");
    assert_eq!(kids.len(), 3);
    let child3 = kids[2]["id"].as_str().unwrap().to_string();
    assert_eq!(await_terminal(&fx, &child3).await, "passed");

    let enqueued = tumult_ingest::gamedays::advance_campaigns(&fx.db_path, &fx.ingest, &fx.runs)
        .await
        .unwrap();
    assert_eq!(enqueued, 0);
    let parent = Store::at(&fx.db_path)
        .read_only()
        .unwrap()
        .run_get("parent-1")
        .unwrap()
        .unwrap();
    assert_eq!(parent["state"], "passed", "all steps passed ≥ threshold");
}

#[tokio::test]
async fn campaign_parent_deviates_below_the_pass_threshold() {
    // Two probe steps; the second is e-stopped (aborted). Pass rate 1/2 is
    // below the 0.75 threshold, so the parent ends deviated. The slow
    // executor keeps the child running long enough for the stop to land.
    let fx = fixture_with(Arc::new(|_| {
        Arc::new(SlowExecutor) as Arc<dyn ActivityExecutor>
    }));
    seed_campaign(&fx, &[("probe one", PROBE_TOON), ("probe two", PROBE_TOON)]).await;

    tumult_ingest::gamedays::advance_campaigns(&fx.db_path, &fx.ingest, &fx.runs)
        .await
        .unwrap();
    let kids = children_of(&fx, "parent-1");
    let child1 = kids[0]["id"].as_str().unwrap().to_string();
    assert_eq!(await_terminal(&fx, &child1).await, "passed");

    tumult_ingest::gamedays::advance_campaigns(&fx.db_path, &fx.ingest, &fx.runs)
        .await
        .unwrap();
    let kids = children_of(&fx, "parent-1");
    let child2 = kids[1]["id"].as_str().unwrap().to_string();
    fx.runs.stop(&child2, Some("tester")).await.unwrap();
    assert_eq!(await_terminal(&fx, &child2).await, "aborted");

    tumult_ingest::gamedays::advance_campaigns(&fx.db_path, &fx.ingest, &fx.runs)
        .await
        .unwrap();
    let parent = Store::at(&fx.db_path)
        .read_only()
        .unwrap()
        .run_get("parent-1")
        .unwrap()
        .unwrap();
    assert_eq!(parent["state"], "deviated", "1/2 passed < 0.75 threshold");
}

/// A daemon "restart" mid-campaign: the startup orphan sweep must leave the
/// gameday parent alone (its registry row is `kind = 'gameday'`, excluded
/// from `active_runs`), and the supervisor's next tick resumes the campaign
/// from store state instead of rolling it back or stalling it.
#[tokio::test]
async fn campaign_parent_survives_a_daemon_restart() {
    let fx = fixture().await;
    seed_campaign(&fx, &[("probe one", PROBE_TOON), ("probe two", PROBE_TOON)]).await;
    let parent = || {
        Store::at(&fx.db_path)
            .read_only()
            .unwrap()
            .run_get("parent-1")
            .unwrap()
            .unwrap()
    };

    // Mid-campaign state: step 1 has passed, the parent is running.
    tumult_ingest::gamedays::advance_campaigns(&fx.db_path, &fx.ingest, &fx.runs)
        .await
        .unwrap();
    let child1 = children_of(&fx, "parent-1")[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(await_terminal(&fx, &child1).await, "passed");
    assert_eq!(parent()["state"], "running");

    // Simulated restart: re-run the startup orphan reconciliation over the
    // live store. The running parent is a campaign, not a stray experiment
    // run, so nothing is reconciled, orphaned, or rolled back.
    let factory: tumult_ingest::runs::ExecutorFactory =
        Arc::new(|_| Arc::new(NoopExecutor) as Arc<dyn ActivityExecutor>);
    let reconciled = tumult_ingest::runs::reconcile_orphans(&fx.ingest, &fx.db_path, &factory)
        .await
        .unwrap();
    assert_eq!(
        reconciled, 0,
        "the campaign parent must be excluded from the orphan sweep"
    );
    assert_eq!(parent()["state"], "running");
    let audit = Store::at(&fx.db_path)
        .read_only()
        .unwrap()
        .query_json_rows("SELECT event FROM run_audit WHERE run_id = 'parent-1'")
        .unwrap();
    assert!(
        !audit.iter().any(|r| matches!(
            r["event"].as_str(),
            Some("orphan_detected" | "rollback_started")
        )),
        "no phantom orphan/rollback audit entries: {audit:?}"
    );

    // The supervisor resumes from store state: step 2 enqueues, passes, and
    // the campaign completes.
    let enqueued = tumult_ingest::gamedays::advance_campaigns(&fx.db_path, &fx.ingest, &fx.runs)
        .await
        .unwrap();
    assert_eq!(enqueued, 1, "the campaign resumes with the next step");
    let kids = children_of(&fx, "parent-1");
    assert_eq!(kids.len(), 2);
    let child2 = kids[1]["id"].as_str().unwrap().to_string();
    assert_eq!(await_terminal(&fx, &child2).await, "passed");

    let enqueued = tumult_ingest::gamedays::advance_campaigns(&fx.db_path, &fx.ingest, &fx.runs)
        .await
        .unwrap();
    assert_eq!(enqueued, 0);
    assert_eq!(
        parent()["state"],
        "passed",
        "campaign completes after restart"
    );
}
