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
//! Split by feature area: `registry` (registry reads + validate),
//! `plan` (dry-run + scope summary), `control` (create/stop/stop-all),
//! `read` (list/detail/audit_verify).

mod control;
mod plan;
mod read;
mod registry;

use crate::error::ApiError;
use tumult_lake::RegisteredDefinition;

use crate::error::{bad_request, not_found};
use crate::sql_util::with_reader;
use crate::ApiState;

pub use control::{create, stop, stop_all, CreateRunRequest};
pub use plan::{dry_run, DryRunRequest};
pub use read::{audit_verify, detail, list, ListParams};
pub use registry::{registry_detail, registry_list, validate, ValidateRequest};

/// Fetch one registered definition by id, or a 404 response.
pub(crate) async fn registry_or_404(
    state: &ApiState,
    registry_id: &str,
) -> Result<RegisteredDefinition, ApiError> {
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

/// Preparation failures may contain a resolved credential in a parser diagnostic.
/// Keep those diagnostics out of HTTP responses and logs.
pub(crate) fn prepare_for_api(
    toon: &str,
    vars: &std::collections::HashMap<String, String>,
) -> Result<
    (
        tumult_core::types::Experiment,
        std::collections::HashMap<String, String>,
    ),
    String,
> {
    tumult_ingest::prepare_run(toon, vars).map_err(|diagnostic| {
        let stage = ["parse", "config", "secrets", "template", "validate"]
            .into_iter()
            .find(|stage| diagnostic.starts_with(&format!("{stage}:")))
            .unwrap_or("prepare");
        format!("{stage}: definition could not be resolved; check its syntax, parameters and configured sources")
    })
}

/// Resolve a display artifact with placeholders instead of reading credential
/// sources. This avoids value-based redaction gaps (numeric secrets, colliding
/// injection keys, and secrets embedded in other strings).
pub(crate) fn preview_experiment(
    toon: &str,
    vars: &std::collections::HashMap<String, String>,
) -> Result<tumult_core::types::Experiment, String> {
    use tumult_core::{
        engine::{apply_template_vars, parse_experiment},
        types::ConfigValue,
    };
    let error = |_| "definition cannot be previewed safely".to_string();
    let raw = parse_experiment(toon).map_err(error)?;
    let config = raw
        .configuration
        .iter()
        .map(|(key, value)| {
            let value = match value {
                ConfigValue::Inline { value } => value.clone(),
                ConfigValue::Env { .. } => "[REDACTED]".into(),
            };
            (key.clone(), value)
        })
        .collect::<std::collections::HashMap<_, _>>();
    let secrets = raw
        .secrets
        .iter()
        .flat_map(|(group, entries)| {
            entries
                .keys()
                .map(move |key| (format!("{group}.{key}"), "[REDACTED]".to_string()))
        })
        .collect::<std::collections::HashMap<_, _>>();
    apply_template_vars(&raw, vars, &config, &secrets).map_err(error)
}
