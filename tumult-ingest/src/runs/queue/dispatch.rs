//! Dispatch of approved gated runs (`RunQueue::dispatch_approved`).

use std::collections::HashMap;

use tumult_lake::{run_state, Store};

use super::{DispatchError, RunQueue, RunRequest};
use crate::runs::{exec_write, now_ns, WorkItem};

impl RunQueue {
    /// Dispatch a run whose approval cleared: flips `pending_approval` back
    /// to `queued` and hands the worker a [`WorkItem`] carrying the approved
    /// pin (re-verified before execution). All approval checks are re-read
    /// from the store here — the approve endpoint and break-glass both funnel
    /// through this one gate. Break-glass requests bypass quorum and TTL (the
    /// override's whole point) but never the pin re-verification in the
    /// worker.
    ///
    /// # Errors
    /// See [`DispatchError`].
    pub async fn dispatch_approved(&self, run_id: &str) -> Result<(), DispatchError> {
        let (request, approval_pin) = {
            let reader = Store::at(&self.shared.db_path)
                .read_only()
                .map_err(|e| DispatchError::Store(e.to_string()))?;
            let run = reader
                .run_get(run_id)
                .map_err(|e| DispatchError::Store(e.to_string()))?
                .ok_or(DispatchError::NotPending)?;
            if run["state"].as_str() != Some(run_state::PENDING_APPROVAL) {
                return Err(DispatchError::NotPending);
            }
            let approval = reader
                .approval_request(run_id)
                .map_err(|e| DispatchError::Store(e.to_string()))?
                .ok_or_else(|| DispatchError::Approval("no approval request".into()))?;
            let break_glass = approval["break_glass"].as_bool().unwrap_or(false);
            if approval["consumed_at_ns"].is_number() {
                return Err(DispatchError::Approval(
                    "approval already consumed — a second run needs a fresh approval".into(),
                ));
            }
            if !break_glass {
                let decisions = reader
                    .approval_decisions(run_id)
                    .map_err(|e| DispatchError::Store(e.to_string()))?;
                if decisions.iter().any(|d| d["decision"] == "rejected") {
                    return Err(DispatchError::Approval("request was rejected".into()));
                }
                let approved = decisions
                    .iter()
                    .filter(|d| d["decision"] == "approved")
                    .count() as i64;
                let quorum = approval["quorum_required"].as_i64().unwrap_or(1);
                if approved < quorum {
                    return Err(DispatchError::Approval(format!(
                        "quorum short: {approved}/{quorum} approvals"
                    )));
                }
                if now_ns() > approval["expires_at_ns"].as_i64().unwrap_or(0) {
                    return Err(DispatchError::Approval(
                        "approval expired before dispatch".into(),
                    ));
                }
            }
            let definition = reader
                .registry_definition(run["registry_id"].as_str().unwrap_or_default())
                .map_err(|e| DispatchError::Store(e.to_string()))?
                .ok_or_else(|| DispatchError::Store("registry row missing".into()))?;
            let vars: HashMap<String, String> = run["params_json"]
                .as_object()
                .map(|obj| {
                    obj.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default();
            let request = RunRequest {
                registry_id: definition.id,
                definition_toon: definition.definition_toon,
                vars,
                env: approval["env"].as_str().unwrap_or("dev").to_string(),
                target: approval["target"].as_str().map(str::to_string),
            };
            let pin = approval["pin_hash"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            (request, pin)
        };
        let permit = self
            .waiting
            .clone()
            .try_acquire_owned()
            .map_err(|_| DispatchError::Full)?;
        let id = run_id.to_string();
        exec_write(&self.shared.ingest, move |writer| {
            writer
                .set_run_state_with(&id, run_state::QUEUED, Some("dispatch_queued"), None, None)
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(DispatchError::Store)?;
        let item = WorkItem {
            run_id: run_id.to_string(),
            request,
            approval_pin: Some(approval_pin),
            _permit: permit,
        };
        self.tx
            .send(item)
            .await
            .map_err(|_| DispatchError::Store("run queue stopped".into()))
    }
}
