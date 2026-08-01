use std::collections::HashMap;
use std::path::Path;

use tumult_lake::{rollback_status, run_state, Store};

use super::queue::build_controls;
use super::{exec_write, ExecutorFactory};
use crate::IngestWriter;

/// Rollback outcome of one orphaned run.
enum OrphanOutcome {
    /// Never started executing — nothing to unwind.
    NothingExecuted,
    RolledBack,
    RollbackFailed(String),
}

/// Reconcile runs left active by a previous process lifetime. Called once at
/// daemon startup, before the servers accept traffic. Returns the number of
/// orphaned runs processed.
///
/// Fault cleanup is the primary duty: rollbacks are attempted even when the
/// state/audit writes themselves fail (a store error never skips a rollback
/// — the fault may still be live; write failures are logged and the run row
/// keeps its active state so the next start retries).
///
/// # Errors
/// Returns an error if the store cannot be read.
pub async fn reconcile_orphans(
    ingest: &IngestWriter,
    db_path: &Path,
    factory: &ExecutorFactory,
) -> Result<usize, String> {
    let orphans = {
        let reader = Store::at(db_path)
            .read_only()
            .map_err(|e| format!("open store read-only: {e}"))?;
        reader.active_runs().map_err(|e| e.to_string())?
    };
    if orphans.is_empty() {
        return Ok(0);
    }
    let count = orphans.len();
    for orphan in orphans {
        let run_id = orphan["id"].as_str().unwrap_or_default().to_string();
        let prior = orphan["state"].as_str().unwrap_or_default().to_string();
        let toon = orphan["definition_toon"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        tracing::warn!(run_id = %run_id, prior_state = %prior, "orphaned run from previous process lifetime");

        let id = run_id.clone();
        // Best-effort: a broken store must never skip the rollback — the
        // fault may still be live in the target system. The run row keeps
        // its active state, so the next daemon start retries.
        if let Err(e) = exec_write(ingest, move |writer| {
            writer
                .set_run_state_with(
                    &id,
                    run_state::ORPHANED,
                    Some("orphan_detected"),
                    Some("daemon restarted; run was owned by a previous process"),
                    None,
                )
                .map_err(|e| e.to_string())
        })
        .await
        {
            tracing::error!(run_id = %run_id, error = %e, "orphan state write failed; attempting rollback anyway");
        }

        let outcome = if prior == run_state::RUNNING || prior == run_state::STOPPING {
            attempt_orphan_rollback(ingest, &run_id, &toon, factory).await
        } else {
            OrphanOutcome::NothingExecuted
        };

        let id = run_id.clone();
        if let Err(e) = exec_write(ingest, move |writer| {
            let result = match &outcome {
                OrphanOutcome::NothingExecuted => writer.finish_run(
                    &id,
                    run_state::ABORTED,
                    None,
                    Some(rollback_status::NOT_NEEDED),
                    Some("orphaned before execution"),
                ),
                OrphanOutcome::RolledBack => writer.finish_run(
                    &id,
                    run_state::ABORTED,
                    None,
                    Some(rollback_status::COMPLETED),
                    Some("orphaned; rollback completed after restart"),
                ),
                OrphanOutcome::RollbackFailed(e) => writer.finish_run(
                    &id,
                    run_state::ROLLBACK_PENDING,
                    None,
                    Some(rollback_status::FAILED),
                    Some(e),
                ),
            };
            result.map_err(|e| e.to_string())
        })
        .await
        {
            tracing::error!(run_id = %run_id, error = %e, "orphan terminal state write failed");
        }
    }
    Ok(count)
}

/// Run the rollback phase of an orphaned run's definition; audit the attempt.
async fn attempt_orphan_rollback(
    ingest: &IngestWriter,
    run_id: &str,
    toon: &str,
    factory: &ExecutorFactory,
) -> OrphanOutcome {
    let id = run_id.to_string();
    let _ = exec_write(ingest, move |writer| {
        writer
            .insert_run_audit(&id, "rollback_started", Some("orphan recovery"), None)
            .map_err(|e| e.to_string())
    })
    .await;

    let experiment = match tumult_core::engine::parse_experiment(toon) {
        Ok(exp) => exp,
        Err(e) => return OrphanOutcome::RollbackFailed(format!("definition unparseable: {e}")),
    };
    // Orphan recovery cannot re-resolve secrets from the crashed run's
    // context; rollbacks run with the daemon's current environment.
    let executor = factory(HashMap::new());
    let controls = build_controls(&experiment, &executor);
    let results = tokio::task::spawn_blocking(move || {
        tumult_core::runner::run_orphan_rollback(&experiment, &executor, &controls)
    })
    .await;

    let (event, outcome) = match results {
        Ok(results) => {
            let failures: Vec<String> = results
                .iter()
                .filter(|r| r.status != tumult_core::types::ActivityStatus::Succeeded)
                .map(|r| r.name.clone())
                .collect();
            if failures.is_empty() {
                ("rollback_completed", OrphanOutcome::RolledBack)
            } else {
                (
                    "rollback_failed",
                    OrphanOutcome::RollbackFailed(format!(
                        "rollback activities failed: {}",
                        failures.join(", ")
                    )),
                )
            }
        }
        Err(e) => (
            "rollback_failed",
            OrphanOutcome::RollbackFailed(format!("rollback task: {e}")),
        ),
    };
    let id = run_id.to_string();
    let _ = exec_write(ingest, move |writer| {
        writer
            .insert_run_audit(&id, event, None, None)
            .map_err(|e| e.to_string())
    })
    .await;
    outcome
}
