//! Recurring runs: the schedule scheduler (schema v10 `run_schedules`).
//!
//! A tokio interval task mirrors the daemon's report/lake schedulers: every
//! tick fires each due, enabled schedule through the **normal run path** —
//! the same parse/resolve/validate pipeline, tier classification and
//! approval gating as `POST /api/runs` — so a scheduled production run still
//! parks for approval. Fired runs are recorded with actor
//! `schedule:<name>`; fire bookkeeping (last run, next fire) rides the
//! daemon's single-writer channel.
//!
//! Missed-fire policy: each fire advances `next_run_at_ns` to
//! `now + interval`, so a daemon that was down fires a schedule exactly once
//! on its first tick back — missed fires collapse, they never pile up. A
//! full run queue skips the fire (retried next tick); a broken definition
//! advances the schedule anyway (no error loop every tick).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use tumult_lake::{ScheduleRow, Store};

use crate::approvals::{classify, introspect, Tier, TierInput};
use crate::runs::{prepare_run, RunQueue, RunRequest};
use crate::{Batch, IngestWriter};

fn now_ns() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as i64)
}

/// The scheduler's tick interval from `TUMULTD_SCHEDULE_TICK_S` (default
/// 30s, minimum 1s); invalid values fall back to the default.
#[must_use]
pub fn tick_from_env() -> Duration {
    std::env::var("TUMULTD_SCHEDULE_TICK_S")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&s| s > 0)
        .map_or_else(|| Duration::from_secs(30), Duration::from_secs)
}

/// Spawn the schedule scheduler: fires due schedules every `tick` until
/// `shutdown` is cancelled. The task holds an `IngestWriter` clone, so the
/// daemon must cancel the token and await the returned handle before
/// draining the writer channel (same contract as the lake scheduler).
pub fn spawn_schedule_scheduler(
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
                    if let Err(e) = fire_due_schedules(&db_path, &ingest, &runs).await {
                        tracing::warn!(error = %e, "schedule scheduler tick failed");
                    }
                }
                () = shutdown.cancelled() => {
                    tracing::info!("schedule scheduler exiting (shutdown)");
                    break;
                }
            }
        }
    })
}

/// One scheduler tick: fire every due enabled schedule. Returns the number
/// of schedules fired. A failure to *read* the store aborts the tick;
/// per-schedule failures are logged and isolated to that schedule.
pub async fn fire_due_schedules(
    db_path: &Path,
    ingest: &IngestWriter,
    runs: &RunQueue,
) -> Result<usize, String> {
    let now = now_ns();
    let due = Store::at(db_path)
        .read_only()
        .map_err(|e| e.to_string())?
        .due_schedules(now)
        .map_err(|e| e.to_string())?;
    let mut fired = 0usize;
    for schedule in due {
        let fired_run = fire_one(db_path, runs, &schedule).await;
        if fired_run.is_none() && !schedule_broken(db_path, &schedule) {
            // Queue full (or a transient store error): leave next_run_at_ns
            // alone so the fire is retried next tick.
            continue;
        }
        // Fired, or a broken definition (advanced anyway, so a bad schedule
        // does not error every tick — fire attempts resume once the operator
        // fixes the definition). A crash between fire and advance refires on
        // the next tick: at-least-once, like the run queue itself.
        fired += usize::from(fired_run.is_some());
        let id = schedule.id.clone();
        let next = now + schedule.interval_s.max(1) * 1_000_000_000;
        ingest
            .write(Batch::Exec(Box::new(move |writer| {
                writer
                    .schedule_fired(&id, fired_run.as_deref(), now, next)
                    .map_err(|e| e.to_string())
            })))
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(fired)
}

/// Whether the schedule's definition fails to resolve — checked cheaply
/// after a failed fire to distinguish "broken" from "queue full".
fn schedule_broken(db_path: &Path, schedule: &ScheduleRow) -> bool {
    let Ok(reader) = Store::at(db_path).read_only() else {
        return false;
    };
    let Ok(Some(def)) = reader.registry_definition(&schedule.registry_id) else {
        return true;
    };
    let vars = parse_vars(schedule);
    prepare_run(&def.definition_toon, &vars).is_err()
}

fn parse_vars(schedule: &ScheduleRow) -> HashMap<String, String> {
    schedule
        .vars_json
        .as_deref()
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or_default()
}

/// Fire one schedule: classify its definition into a risk tier exactly like
/// `POST /api/runs` and enqueue (T0) or park for approval (T1–T3). Returns
/// the new run id, or `None` when nothing was enqueued (broken definition
/// or full queue).
async fn fire_one(db_path: &Path, runs: &RunQueue, schedule: &ScheduleRow) -> Option<String> {
    let definition = match Store::at(db_path).read_only() {
        Ok(reader) => match reader.registry_definition(&schedule.registry_id) {
            Ok(Some(def)) => def,
            Ok(None) => {
                tracing::warn!(schedule = %schedule.id, registry_id = %schedule.registry_id, "scheduled definition no longer registered");
                return None;
            }
            Err(e) => {
                tracing::warn!(schedule = %schedule.id, error = %e, "schedule registry read failed");
                return None;
            }
        },
        Err(e) => {
            tracing::warn!(schedule = %schedule.id, error = %e, "schedule store open failed");
            return None;
        }
    };
    let actor = format!("schedule:{}", schedule.name);
    let request = RunRequest {
        registry_id: definition.id,
        definition_toon: definition.definition_toon,
        vars: parse_vars(schedule),
        env: schedule.env.clone(),
        target: schedule.target.clone(),
    };
    let experiment = match prepare_run(&request.definition_toon, &request.vars) {
        Ok((experiment, _env)) => experiment,
        Err(e) => {
            tracing::warn!(schedule = %schedule.id, error = %e, "scheduled definition failed to resolve");
            return None;
        }
    };
    let tier = classify(&TierInput {
        env: schedule.env.clone(),
        catalog_matched: false,
        introspection: introspect(&experiment),
    });
    let outcome = if tier == Tier::T0 {
        runs.enqueue(request, Some(actor)).await
    } else {
        runs.request_gated(request, tier, Some(actor)).await
    };
    match outcome {
        Ok(run_id) => Some(run_id),
        Err(e) => {
            let reason = match e {
                crate::runs::EnqueueError::Full => "run queue full; retrying next tick".to_string(),
                crate::runs::EnqueueError::Store(e) => e,
            };
            tracing::warn!(schedule = %schedule.id, error = %reason, "scheduled fire rejected");
            None
        }
    }
}
