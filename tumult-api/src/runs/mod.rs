//! Run-control endpoints (`/api/runs*`) — validate, dry-run, enqueue,
//! e-stop and inspect daemon-managed experiment runs (schema v5
//! `run_registry` / `runs` / `run_audit`).
//!
//! Definitions register through `POST /api/runs/validate`: the exact
//! parse/resolve/validate pipeline the CLI's `tumult run` applies
//! ([`tumult_ingest::prepare_run`]), then a content-hash-deduped row in
//! `run_registry`. `POST /api/runs` enqueues onto the daemon's bounded
//! [`tumult_ingest::RunQueue`] (429 on overload — never silently queued);
//! `POST /api/runs/{id}/stop` cancels the run's e-stop token. All reads
//! run on a fresh read-only connection, all mutations ride the daemon's
//! single-writer channel — this module never opens a write connection.
//!
//! Split by feature area: [`registry`] (registry reads + validate),
//! [`plan`] (dry-run + scope summary), [`control`] (create/stop/stop-all),
//! [`read`] (list/detail/audit_verify).

mod control;
mod plan;
mod read;
mod registry;

use axum::response::Response;
use tumult_lake::RegisteredDefinition;

use crate::error::{bad_request, not_found};
use crate::sql_util::with_reader;
use crate::ApiState;

pub use control::{create, stop, stop_all, CreateRunRequest};
pub use plan::{dry_run, DryRunRequest};
pub use read::{audit_verify, detail, list, ListParams};
pub use registry::{registry_detail, registry_list, validate, ValidateRequest};

/// Fetch one registered definition by id, or a 404 response.
async fn registry_or_404(
    state: &ApiState,
    registry_id: &str,
) -> Result<RegisteredDefinition, Response> {
    if registry_id.chars().count() > 100 {
        return Err(bad_request("registry id too long"));
    }
    let id = registry_id.to_string();
    let def = with_reader(&state.db_path, move |reader| {
        reader.registry_definition(&id).map_err(|e| e.to_string())
    })
    .await?;
    def.ok_or_else(|| not_found(format!("unknown registry id {registry_id:?}")))
}
