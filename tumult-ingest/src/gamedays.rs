//! GameDay campaigns: the supervisor that advances each active campaign
//! through its experiments as sequential child runs (schema v12).
//!
//! A campaign is a parent run whose registry row has `kind = 'gameday'`
//! (`runs.gameday_id IS NULL`); its children are ordinary runs linked via
//! `runs.gameday_id`. Every tick, each active campaign either enqueues its
//! next experiment — through the normal run path, so a gated experiment
//! parks the campaign at an approval — or, when every child is terminal,
//! takes the campaign outcome: `failed` when any child failed or orphaned,
//! else `passed` when the fraction of passed children meets the
//! campaign's `scoring.pass_threshold` (default 0.75), else `deviated`.
//!
//! Shared k6 load and the aggregate `GameDayJournal` remain deferred —
//! campaigns here are the sequential child-run model only.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tumult_lake::{rollback_status, run_state, Store};

use crate::approvals::{classify, introspect, Tier, TierInput};
use crate::runs::{prepare_run, EnqueueError, RunQueue, RunRequest};
use crate::IngestWriter;

/// The supervisor's tick interval from `TUMULTD_GAMEDAY_TICK_S` (default
/// 15s, minimum 1s); invalid values fall back to the default.
#[must_use]
pub fn tick_from_env() -> Duration {
    std::env::var("TUMULTD_GAMEDAY_TICK_S")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&s| s > 0)
        .map_or_else(|| Duration::from_secs(15), Duration::from_secs)
}

/// Spawn the campaign supervisor (same shutdown contract as the other
/// daemon background tasks).
pub fn spawn_gameday_supervisor(
    db_path: PathBuf,
    ingest: IngestWriter,
    runs: RunQueue,
    tick: Duration,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tick);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(e) = advance_campaigns(&db_path, &ingest, &runs).await {
                        tracing::warn!(error = %e, "gameday supervisor tick failed");
                    }
                }
                () = shutdown.cancelled() => {
                    tracing::info!("gameday supervisor exiting (shutdown)");
                    break;
                }
            }
        }
    })
}

/// One supervisor tick: advance every active campaign. Returns the number
/// of child runs enqueued.
pub async fn advance_campaigns(
    db_path: &Path,
    ingest: &IngestWriter,
    runs: &RunQueue,
) -> Result<usize, String> {
    let parents = Store::at(db_path)
        .read_only()
        .map_err(|e| e.to_string())?
        .query_json_rows(
            "SELECT r.*, g.definition_toon AS gameday_definition FROM runs r \
             JOIN run_registry g ON g.id = r.registry_id \
             WHERE g.kind = 'gameday' AND r.gameday_id IS NULL \
               AND r.state IN ('queued', 'running')",
        )
        .map_err(|e| e.to_string())?;
    let mut enqueued = 0usize;
    for parent in &parents {
        match advance_one(db_path, ingest, runs, parent).await {
            Ok(n) => enqueued += n,
            Err(e) => tracing::warn!(run = %parent["id"], error = %e, "campaign advance failed"),
        }
    }
    Ok(enqueued)
}

/// Advance one campaign: finish it when every step is terminal, wait while
/// the current child is active, otherwise enqueue the next step.
async fn advance_one(
    db_path: &Path,
    ingest: &IngestWriter,
    runs: &RunQueue,
    parent: &Value,
) -> Result<usize, String> {
    let parent_id = parent["id"].as_str().unwrap_or_default().to_string();
    let envelope: Value =
        serde_json::from_str(parent["gameday_definition"].as_str().unwrap_or("{}"))
            .map_err(|e| e.to_string())?;
    let steps = envelope["experiments"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let pass_threshold = envelope["scoring"]["pass_threshold"]
        .as_f64()
        .unwrap_or(0.75);
    let reader = Store::at(db_path).read_only().map_err(|e| e.to_string())?;
    let children = reader
        .query_json_rows(&format!(
            "SELECT * FROM runs WHERE gameday_id = '{}' ORDER BY queued_at_ns",
            parent_id.replace('\'', "''")
        ))
        .map_err(|e| e.to_string())?;

    if children.len() >= steps.len() {
        if children.iter().all(is_terminal) {
            let outcome = campaign_outcome(&children, pass_threshold);
            finish_parent(ingest, &parent_id, outcome).await?;
        }
        return Ok(0);
    }
    if children.last().is_some_and(|c| !is_terminal(c)) {
        return Ok(0); // current step still executing (or parked for approval)
    }

    // Enqueue the next step through the normal run path.
    let registry_id = steps[children.len()]["registry_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let def = reader
        .registry_definition(&registry_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("campaign step registry row {registry_id} missing"))?;
    let env = parent["params_json"]["env"]
        .as_str()
        .unwrap_or("dev")
        .to_string();
    let (experiment, _env) = prepare_run(&def.definition_toon, &HashMap::new())?;
    let tier = classify(&TierInput {
        env: env.clone(),
        catalog_matched: false,
        introspection: introspect(&experiment),
    });
    let request = RunRequest {
        registry_id: def.id,
        definition_toon: def.definition_toon,
        vars: HashMap::new(),
        env,
        target: None,
    };
    let actor = format!("gameday:{parent_id}");
    let child_id = match if tier == Tier::T0 {
        runs.enqueue(request, Some(actor)).await
    } else {
        runs.request_gated(request, tier, Some(actor)).await
    } {
        Ok(id) => id,
        Err(EnqueueError::Full) => {
            tracing::warn!(parent = %parent_id, "run queue full; campaign step retries next tick");
            return Ok(0);
        }
        Err(EnqueueError::Store(e)) => return Err(e),
    };

    // Link the child and mark the parent running.
    let parent_state = parent["state"].as_str().unwrap_or_default().to_string();
    let parent_for_write = parent_id.clone();
    let child_for_write = child_id.clone();
    ingest
        .write(crate::Batch::Exec(Box::new(move |writer| {
            writer
                .set_run_gameday(&child_for_write, &parent_for_write)
                .map_err(|e| e.to_string())?;
            if parent_state == "queued" {
                writer
                    .set_run_state_with(
                        &parent_for_write,
                        "running",
                        Some("campaign_started"),
                        None,
                        None,
                    )
                    .map_err(|e| e.to_string())?;
            }
            Ok(())
        })))
        .await
        .map_err(|e| e.to_string())?;
    Ok(1)
}

/// Whether a child run is done (any terminal state counts — the campaign
/// moves on from aborted/rejected/expired steps too).
fn is_terminal(run: &Value) -> bool {
    run["state"]
        .as_str()
        .is_some_and(|s| run_state::TERMINAL.contains(&s))
}

/// The campaign outcome: any failed/orphaned child fails the campaign;
/// otherwise the fraction of passed children must meet the threshold.
fn campaign_outcome(children: &[Value], pass_threshold: f64) -> &'static str {
    let failed = children
        .iter()
        .any(|c| matches!(c["state"].as_str(), Some("failed" | "orphaned")));
    if failed {
        return run_state::FAILED;
    }
    let passed = children
        .iter()
        .filter(|c| c["state"].as_str() == Some(run_state::PASSED))
        .count();
    #[allow(clippy::cast_precision_loss)]
    let fraction = passed as f64 / children.len().max(1) as f64;
    if fraction >= pass_threshold {
        run_state::PASSED
    } else {
        run_state::DEVIATED
    }
}

/// Finish the parent run with the campaign outcome.
async fn finish_parent(
    ingest: &IngestWriter,
    parent_id: &str,
    outcome: &str,
) -> Result<(), String> {
    let id = parent_id.to_string();
    let outcome = outcome.to_string();
    ingest
        .write(crate::Batch::Exec(Box::new(move |writer| {
            writer
                .finish_run(&id, &outcome, None, Some(rollback_status::NOT_NEEDED), None)
                .map_err(|e| e.to_string())
        })))
        .await
        .map_err(|e| e.to_string())
}
