//! E-stop (`RunQueue::stop`).

use tumult_lake::run_state;

use super::{RunQueue, StopError};
use crate::runs::{exec_write, read_run_state};

impl RunQueue {
    /// Cancel a run and persist stop on the single writer. Waiting runs abort;
    /// started runs retain their cleanup responsibility until the runner ends.
    ///
    /// # Errors
    /// See [`StopError`].
    pub async fn stop(&self, run_id: &str, actor: Option<&str>) -> Result<(), StopError> {
        let state = read_run_state(&self.shared.db_path, run_id).ok_or(StopError::NotFound)?;
        if run_state::TERMINAL.contains(&state.as_str()) {
            return Err(StopError::Terminal(state));
        }
        let token = self
            .shared
            .tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(run_id)
            .cloned();
        if let Some(token) = token {
            token.cancel();
        }
        let id = run_id.to_string();
        let actor = actor.map(str::to_string);
        exec_write(&self.shared.ingest, move |writer| {
            writer
                .request_run_stop(&id, actor.as_deref())
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(StopError::Store)
    }
}
