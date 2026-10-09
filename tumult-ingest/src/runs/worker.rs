use std::collections::BTreeMap;

use tokio_util::sync::CancellationToken;
use tumult_core::runner::RunConfig;
use tumult_core::types::{Experiment, ExperimentStatus, Journal};
use tumult_lake::{approval_pin, rollback_status, run_state, CanonicalPin, Store};

use super::queue::build_controls;
use super::recovery_plan::RecoveryPlan;
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
    let ids = match ids {
        Ok(ids) => ids,
        Err(error) => {
            tracing::error!(%error, "approval expiry read failed");
            return;
        }
    };
    for id in ids {
        tracing::info!(run_id = %id, "approval expired before dispatch");
        if let Err(error) = exec_write(&shared.ingest, move |writer| {
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
        .await
        {
            tracing::error!(%error, "approval expiry persistence failed");
        }
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

    // Register before any externally visible worker transition. A concurrent
    // stop either cancels this token or wins the queued-state compare-and-set.
    let token = CancellationToken::new();
    {
        let mut tokens = shared
            .tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let std::collections::hash_map::Entry::Vacant(slot) = tokens.entry(run_id.clone()) else {
            tracing::warn!(%run_id, "duplicate work item refused; active cancellation token preserved");
            return;
        };
        slot.insert(token.clone());
    }
    let _registration = TokenRegistration {
        shared,
        run_id: &run_id,
    };

    // The run may have been cancelled while waiting.
    if read_run_state(&shared.db_path, &run_id).as_deref() != Some(run_state::QUEUED) {
        return;
    }

    if let Err(reason) = verify_approval(shared, &run_id, &request, expected_pin_opt.as_deref()) {
        crate::daemon_metrics::run_failed();
        let id = run_id.clone();
        if let Err(error) = exec_write(ingest, move |writer| {
            writer
                .insert_run_audit(&id, "dispatch_refused", Some(&reason), None)
                .and_then(|()| writer.fail_run(&id, &reason))
                .map_err(|e| e.to_string())
        })
        .await
        {
            tracing::error!(run_id = %run_id, %error, "dispatch refusal persistence failed");
        }
        return;
    }

    let id = run_id.clone();
    if let Err(error) = exec_write(ingest, move |writer| {
        writer.claim_run(&id).map_err(|e| e.to_string())
    })
    .await
    {
        tracing::warn!(run_id = %run_id, %error, "run claim refused; no provider dispatched");
        return;
    }

    let plan = match RecoveryPlan::capture(&request.definition_toon, &request.vars) {
        Ok(plan) => plan,
        Err(error) => {
            record_failure(ingest, &run_id, &error).await;
            return;
        }
    };
    let (experiment, injected_env) = match plan.prepare() {
        Ok(prepared) => prepared,
        Err(error) => {
            record_failure(ingest, &run_id, &error).await;
            return;
        }
    };

    if let Err(error) =
        validate_dispatch_context(shared, &run_id, &request, &experiment, &injected_env)
    {
        record_failure(ingest, &run_id, &error).await;
        return;
    }

    let plan_json = match serde_json::to_string(&plan) {
        Ok(json) => json,
        Err(error) => {
            record_failure(ingest, &run_id, &format!("recovery plan: {error}")).await;
            return;
        }
    };
    if token.is_cancelled() {
        return;
    }
    let id = run_id.clone();
    let pin = expected_pin_opt.clone();
    if let Err(error) = exec_write(ingest, move |writer| {
        writer
            .begin_run_execution(&id, &plan_json, pin.as_deref())
            .map_err(|e| e.to_string())
    })
    .await
    {
        tracing::error!(run_id = %run_id, %error, "durable start refused; no provider dispatched");
        record_failure(ingest, &run_id, &error).await;
        return;
    }
    crate::daemon_metrics::run_started();

    let executor = factory(injected_env);
    let controls = build_controls(&experiment, &executor);
    let config = RunConfig {
        cancellation_token: Some(token.clone()),
        load_executor: Some(std::sync::Arc::new(tumult_core::runner::k6::K6LoadExecutor)),
        ..RunConfig::default()
    };
    let run_experiment = {
        let experiment = experiment.clone();
        let executor = executor.clone();
        let controls = controls.clone();
        move || tumult_core::runner::run_experiment(&experiment, &executor, &controls, &config)
    };
    let result = tokio::task::spawn_blocking(run_experiment).await;
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

    complete_or_cleanup(
        ingest,
        &run_id,
        &experiment,
        executor,
        controls,
        &token,
        journal,
    )
    .await;
}

/// Validate the approved immutable request before claiming its execution slot.
fn verify_approval(
    shared: &Shared,
    run_id: &str,
    request: &super::RunRequest,
    expected_pin: Option<&str>,
) -> Result<(), String> {
    let Some(expected_pin) = expected_pin else {
        return Ok(());
    };
    let params: BTreeMap<String, String> = request.vars.clone().into_iter().collect();
    let actual = approval_pin(&CanonicalPin {
        definition_toon: &request.definition_toon,
        params: &params,
        env: &request.env,
        target: request.target.as_deref(),
    });
    if actual != expected_pin {
        return Err(format!("approval pin mismatch (approved {expected_pin}, resolves to {actual}) — definition, params, env or target edited after approval"));
    }
    let req = Store::at(&shared.db_path)
        .read_only()
        .map_err(|e| e.to_string())?
        .approval_request(run_id)
        .map_err(|e| format!("approval re-read failed: {e}"))?
        .ok_or_else(|| "approval request missing".to_string())?;
    if req["consumed_at_ns"].is_number() {
        return Err("approval already consumed — single-use".into());
    }
    if !req["break_glass"].as_bool().unwrap_or(false)
        && now_ns() > req["expires_at_ns"].as_i64().unwrap_or(0)
    {
        return Err("approval expired before dispatch".into());
    }
    Ok(())
}

async fn complete_or_cleanup(
    ingest: &IngestWriter,
    run_id: &str,
    experiment: &Experiment,
    executor: std::sync::Arc<dyn tumult_core::runner::ActivityExecutor>,
    controls: std::sync::Arc<tumult_core::controls::ControlRegistry>,
    token: &CancellationToken,
    mut journal: Journal,
) {
    let completion = record_completion(ingest, run_id, experiment, &journal, token).await;
    if completion.is_ok() {
        return;
    }
    // A stop can arrive after the runner returns but before its terminal write.
    // The writer rejects that completion; perform cleanup, then retry once.
    if token.is_cancelled() && journal.status != ExperimentStatus::Interrupted {
        journal.status = ExperimentStatus::Interrupted;
        journal.analysis = None;
        if journal.rollback_results.is_empty() {
            let cleanup_experiment = experiment.clone();
            match tokio::task::spawn_blocking(move || {
                tumult_core::runner::run_orphan_rollback(&cleanup_experiment, &executor, &controls)
            })
            .await
            {
                Ok(results) => {
                    journal.rollback_failures = u32::try_from(
                        results
                            .iter()
                            .filter(|r| r.status != tumult_core::types::ActivityStatus::Succeeded)
                            .count(),
                    )
                    .unwrap_or(u32::MAX);
                    journal.rollback_results = results;
                }
                Err(error) => {
                    record_failure(ingest, run_id, &format!("late-stop cleanup: {error}")).await;
                    return;
                }
            }
        }
        journal.ended_at_ns = now_ns();
        journal.duration_ms =
            u64::try_from(journal.ended_at_ns.saturating_sub(journal.started_at_ns) / 1_000_000)
                .unwrap_or(0);
        if let Err(error) = record_completion(ingest, run_id, experiment, &journal, token).await {
            tracing::error!(%run_id, %error, "stopped-run completion failed; leaving run recoverable");
        }
        return;
    }
    if let Err(error) = completion {
        tracing::error!(%run_id, %error, "completion persistence failed; leaving run recoverable");
    }
}

/// Terminal state + journal ingest for a finished run.
async fn record_completion(
    ingest: &IngestWriter,
    run_id: &str,
    experiment: &Experiment,
    journal: &Journal,
    token: &CancellationToken,
) -> Result<(), String> {
    let state = if journal.rollback_failures > 0 {
        run_state::ROLLBACK_PENDING
    } else {
        terminal_state(&journal.status)
    }
    .to_string();
    let rb = if journal.rollback_results.is_empty() {
        rollback_status::NOT_NEEDED
    } else if journal.rollback_failures == 0 {
        rollback_status::COMPLETED
    } else {
        rollback_status::FAILED
    }
    .to_string();
    let experiment_id = journal.experiment_id.clone();
    let experiment = experiment.clone();
    let id = run_id.to_string();
    let journal = journal.clone();
    let token = token.clone();
    let result = exec_write(ingest, move |writer| {
        if token.is_cancelled() && journal.status != ExperimentStatus::Interrupted {
            return Err("stop accepted before completion; cleanup required".into());
        }
        // Evidence must commit before a terminal state excludes this run from
        // recovery. If the second write fails the conservative retry is safe.
        writer
            .ingest_journal(&journal, Some(&experiment))
            .map_err(|e| e.to_string())?;
        writer
            .finish_run(&id, &state, Some(&experiment_id), Some(&rb), None)
            .map_err(|e| e.to_string())
    })
    .await;
    if result.is_ok() {
        crate::daemon_metrics::run_completed();
    }
    result
}

async fn record_failure(ingest: &IngestWriter, run_id: &str, error: &str) {
    crate::daemon_metrics::run_failed();
    let id = run_id.to_string();
    let error = error.to_string();
    if let Err(persistence_error) = exec_write(ingest, move |writer| {
        writer.fail_run(&id, &error).map_err(|e| e.to_string())
    })
    .await
    {
        tracing::error!(%run_id, %persistence_error, "failure persistence failed; run remains recoverable");
    }
}

struct TokenRegistration<'a> {
    shared: &'a Shared,
    run_id: &'a str,
}

impl Drop for TokenRegistration<'_> {
    fn drop(&mut self) {
        self.shared
            .tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(self.run_id);
    }
}

/// Re-evaluate authenticated permissions and the operator-owned environment
/// binding at dispatch, including changes since approval or scheduling.
fn validate_dispatch_context(
    shared: &Shared,
    run_id: &str,
    request: &super::RunRequest,
    experiment: &Experiment,
    injected: &std::collections::HashMap<String, String>,
) -> Result<(), String> {
    let reader = Store::at(&shared.db_path)
        .read_only()
        .map_err(|e| e.to_string())?;
    let audit = reader.run_audit_trail(run_id).map_err(|e| e.to_string())?;
    let actor = audit.iter().find_map(|row| row["actor"].as_str());
    let requires_binding =
        crate::execution_policy::actor_requires_binding(&shared.db_path, actor, &request.env)?;
    let approval = reader.approval_request(run_id).map_err(|e| e.to_string())?;
    let recorded_hash = reader
        .run_execution_hash(run_id)
        .map_err(|error| error.to_string())?;
    if let Some(recorded_hash) = recorded_hash {
        let current_hash = crate::execution_policy::execution_hash(experiment, injected)?;
        if current_hash != recorded_hash {
            return Err("resolved configuration or credentials changed after submission; submit for approval again".into());
        }
    } else if actor.is_some() || approval.is_some() {
        return Err("run has no pinned execution fingerprint; submit it again".into());
    }
    if actor.is_none() && approval.is_none() {
        return Ok(());
    } // trusted in-process enqueue
    let tier = crate::execution_policy::classify_execution(
        &shared.db_path,
        experiment,
        injected,
        &request.env,
        request.target.as_deref(),
        requires_binding,
    )?;
    let approved_tier = approval
        .as_ref()
        .and_then(|row| row["tier"].as_str())
        .unwrap_or("T0");
    if tier.as_str() > approved_tier {
        return Err(
            "resolved execution requires a higher approval tier; submit for approval again".into(),
        );
    }
    Ok(())
}
