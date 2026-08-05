use std::collections::BTreeMap;

use tokio_util::sync::CancellationToken;
use tumult_core::runner::RunConfig;
use tumult_core::types::{Experiment, ExperimentStatus, Journal};
use tumult_lake::{approval_pin, rollback_status, run_state, CanonicalPin, Store};

use super::queue::{build_controls, prepare_run};
use super::{exec_write, now_ns, read_run_state, ExecutorFactory, Shared, WorkItem};
use crate::IngestWriter;

/// Map the journal's experiment status onto the run state machine.
fn terminal_state(status: &ExperimentStatus) -> &'static str {
    match status {
        ExperimentStatus::Completed => run_state::PASSED,
        ExperimentStatus::Deviated => run_state::DEVIATED,
        ExperimentStatus::Failed => run_state::FAILED,
        ExperimentStatus::Aborted | ExperimentStatus::Interrupted | ExperimentStatus::Halted => {
            run_state::ABORTED
        }
    }
}

/// Terminal-flip every gated run whose approval TTL has lapsed.
pub(super) async fn sweep_expired_approvals(shared: &Shared) {
    let ids = Store::at(&shared.db_path)
        .read_only()
        .and_then(|r| r.expired_pending_approvals(now_ns()));
    let Ok(ids) = ids else { return };
    for id in ids {
        tracing::info!(run_id = %id, "approval expired before dispatch");
        let _ = exec_write(&shared.ingest, move |writer| {
            writer
                .finish_run(
                    &id,
                    run_state::EXPIRED,
                    None,
                    Some(rollback_status::NOT_NEEDED),
                    Some("approval TTL lapsed"),
                )
                .map_err(|e| e.to_string())
        })
        .await;
    }
}

/// One worker pass over a dequeued run: validate, execute, record.
pub(super) async fn process(item: WorkItem, shared: &Shared, factory: &ExecutorFactory) {
    let WorkItem {
        run_id,
        request,
        approval_pin: expected_pin_opt,
        _permit: permit,
    } = item;
    // Dequeued: the waiting-queue slot frees now, not when the run ends.
    drop(permit);
    let ingest = &shared.ingest;

    // The run may have been cancelled while waiting.
    if read_run_state(&shared.db_path, &run_id).as_deref() != Some(run_state::QUEUED) {
        return;
    }

    // T10: a gated run reaches the worker only via dispatch_approved and
    // carries the approved canonical pin. Re-verify here — at the last
    // moment before execution — that what is about to run is bit-identical
    // to what was approved (any edit after approval breaks the pin), and
    // that the approval is unconsumed and (unless break-glass) unexpired.
    // Every failure refuses dispatch terminally.
    if let Some(expected_pin) = &expected_pin_opt {
        let params: BTreeMap<String, String> = request.vars.clone().into_iter().collect();
        let actual = approval_pin(&CanonicalPin {
            definition_toon: &request.definition_toon,
            params: &params,
            env: &request.env,
            target: request.target.as_deref(),
        });
        let mut refusal = (actual != *expected_pin).then(|| {
            format!("approval pin mismatch (approved {expected_pin}, resolves to {actual}) — definition, params, env or target edited after approval")
        });
        if refusal.is_none() {
            match Store::at(&shared.db_path)
                .read_only()
                .map_err(|e| e.to_string())
                .and_then(|r| r.approval_request(&run_id).map_err(|e| e.to_string()))
            {
                Ok(Some(req)) => {
                    let break_glass = req["break_glass"].as_bool().unwrap_or(false);
                    if req["consumed_at_ns"].is_number() {
                        refusal = Some("approval already consumed — single-use".into());
                    } else if !break_glass && now_ns() > req["expires_at_ns"].as_i64().unwrap_or(0)
                    {
                        refusal = Some("approval expired before dispatch".into());
                    }
                }
                Ok(None) => refusal = Some("approval request missing".into()),
                Err(e) => refusal = Some(format!("approval re-read failed: {e}")),
            }
        }
        if let Some(reason) = refusal {
            crate::daemon_metrics::run_failed();
            let id = run_id.clone();
            let _ = exec_write(ingest, move |writer| {
                writer
                    .insert_run_audit(&id, "dispatch_refused", Some(&reason), None)
                    .and_then(|()| {
                        writer.finish_run(&id, run_state::FAILED, None, None, Some(&reason))
                    })
                    .map_err(|e| e.to_string())
            })
            .await;
            return;
        }
        // Single-use: one dispatch consumes one approval (ADR-013).
        let id = run_id.clone();
        let pin = expected_pin.clone();
        let _ = exec_write(ingest, move |writer| {
            writer
                .consume_approval(&id, now_ns())
                .and_then(|()| writer.insert_run_audit(&id, "consumed", Some(&pin), None))
                .map_err(|e| e.to_string())
        })
        .await;
    }

    let id = run_id.clone();
    let _ = exec_write(ingest, move |writer| {
        writer
            .set_run_state(&id, run_state::VALIDATING)
            .map_err(|e| e.to_string())
    })
    .await;

    let (experiment, injected_env) = match prepare_run(&request.definition_toon, &request.vars) {
        Ok(prepared) => prepared,
        Err(e) => {
            crate::daemon_metrics::run_failed();
            let id = run_id.clone();
            let _ = exec_write(ingest, move |writer| {
                writer
                    .finish_run(&id, run_state::FAILED, None, None, Some(&e))
                    .map_err(|e| e.to_string())
            })
            .await;
            return;
        }
    };

    let id = run_id.clone();
    let _ = exec_write(ingest, move |writer| {
        writer
            .mark_run_started(&id, None)
            .map_err(|e| e.to_string())
    })
    .await;
    crate::daemon_metrics::run_started();

    let token = CancellationToken::new();
    shared
        .tokens
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(run_id.clone(), token.clone());

    let executor = factory(injected_env);
    let controls = build_controls(&experiment, &executor);
    let config = RunConfig {
        cancellation_token: Some(token),
        ..RunConfig::default()
    };
    let run_experiment = {
        let experiment = experiment.clone();
        move || tumult_core::runner::run_experiment(&experiment, &executor, &controls, &config)
    };
    let result = tokio::task::spawn_blocking(run_experiment).await;
    shared
        .tokens
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&run_id);

    let journal = match result {
        Ok(Ok(journal)) => journal,
        Ok(Err(e)) => {
            record_failure(ingest, &run_id, &format!("runner: {e}")).await;
            return;
        }
        Err(e) => {
            record_failure(ingest, &run_id, &format!("runner task: {e}")).await;
            return;
        }
    };

    record_completion(ingest, &run_id, &experiment, journal).await;
}

/// Terminal state + journal ingest for a finished run.
async fn record_completion(
    ingest: &IngestWriter,
    run_id: &str,
    experiment: &Experiment,
    journal: Journal,
) {
    crate::daemon_metrics::run_completed();
    let state = terminal_state(&journal.status).to_string();
    let rb = if journal.rollback_results.is_empty() {
        rollback_status::NOT_NEEDED
    } else if journal.rollback_failures == 0 {
        rollback_status::COMPLETED
    } else {
        rollback_status::FAILED
    }
    .to_string();
    let experiment_id = journal.experiment_id.clone();
    let id = run_id.to_string();
    let _ = exec_write(ingest, move |writer| {
        writer
            .finish_run(&id, &state, Some(&experiment_id), Some(&rb), None)
            .map_err(|e| e.to_string())
    })
    .await;

    // Journal into the analytics tables on the same single writer.
    let experiment = experiment.clone();
    let _ = exec_write(ingest, move |writer| {
        writer
            .ingest_journal(&journal, Some(&experiment))
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
    .await;
}

async fn record_failure(ingest: &IngestWriter, run_id: &str, error: &str) {
    crate::daemon_metrics::run_failed();
    let id = run_id.to_string();
    let error = error.to_string();
    let _ = exec_write(ingest, move |writer| {
        writer
            .finish_run(&id, run_state::FAILED, None, None, Some(&error))
            .map_err(|e| e.to_string())
    })
    .await;
}
