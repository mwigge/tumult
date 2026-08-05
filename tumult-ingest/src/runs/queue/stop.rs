//! E-stop (`RunQueue::stop`).

use tumult_lake::{rollback_status, run_state};

use super::{RunQueue, StopError};
use crate::runs::{exec_write, read_run_state};

impl RunQueue {
    /// E-stop a run: cancel its token (the runner stops before the next
    /// activity and runs rollbacks) and record `stopping`. Runs still
    /// waiting are cancelled before they start. `actor` is the authenticated
    /// identity behind the stop request, recorded on the `stop_requested`
    /// audit event (`None` when unauthenticated).
    ///
    /// # Errors
    /// See [`StopError`].
    pub async fn stop(&self, run_id: &str, actor: Option<&str>) -> Result<(), StopError> {
        let token = self
            .shared
            .tokens
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(run_id)
            .cloned();
        if let Some(token) = token {
            token.cancel();
            let id = run_id.to_string();
            let actor = actor.map(str::to_string);
            exec_write(&self.shared.ingest, move |writer| {
                writer
                    .set_run_state_with(
                        &id,
                        run_state::STOPPING,
                        Some("stop_requested"),
                        None,
                        actor.as_deref(),
                    )
                    .map_err(|e| e.to_string())
            })
            .await
            .map_err(StopError::Store)?;
            return Ok(());
        }
        match read_run_state(&self.shared.db_path, run_id) {
            None => Err(StopError::NotFound),
            Some(state) if run_state::TERMINAL.contains(&state.as_str()) => {
                Err(StopError::Terminal(state))
            }
            Some(_) => {
                // Waiting (queued/validating) but no token yet: cancel before
                // start — the worker re-checks state after dequeue and skips.
                // The stop is audited exactly like the running path: a
                // `stop_requested` event naming the halting principal, then
                // the terminal `aborted` event.
                let id = run_id.to_string();
                let actor = actor.map(str::to_string);
                exec_write(&self.shared.ingest, move |writer| {
                    writer
                        .insert_run_audit(
                            &id,
                            "stop_requested",
                            Some("cancelled before start"),
                            actor.as_deref(),
                        )
                        .map_err(|e| e.to_string())?;
                    writer
                        .finish_run(
                            &id,
                            run_state::ABORTED,
                            None,
                            Some(rollback_status::NOT_NEEDED),
                            Some("cancelled before start"),
                        )
                        .map_err(|e| e.to_string())
                })
                .await
                .map_err(StopError::Store)
            }
        }
    }
}
