//! Authoring endpoints (`/api/authoring*`) — the live fault catalog and
//! experiment scaffolding, thin REST wrappers over [`tumult_authoring`].
//!
//! These are the same code paths as the MCP `tumult_fault_catalog` /
//! `tumult_scaffold_experiment` tools and the CLI (`tumult new`), exposed
//! without an MCP hop so the web UI can browse the catalog and generate
//! experiment TOON in-process: the orchestration lives in
//! [`tumult_authoring::scaffold`], this module only maps arguments and
//! errors. Both endpoints are read-only w.r.t. the store: scaffolding
//! generates content but never persists — registration stays behind
//! `POST /api/runs/validate` (Operator role).

use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use tumult_authoring::scaffold::{ScaffoldError, ScaffoldInput};
use tumult_authoring::FaultCatalog;

/// Client-error body: the 400s of this module are always safe to detail
/// (they describe the request, never store internals).
fn bad_request(msg: String) -> (StatusCode, Json<Value>) {
    (StatusCode::BAD_REQUEST, Json(json!({"error": msg})))
}

/// 500 body: details are logged server-side, the client gets a fixed
/// generic message (same contract as [`crate::sql_util::internal`]). Not
/// delegated: `sql_util::internal` returns `Response`, while this
/// module's error channel is the smaller `(StatusCode, Json<Value>)`
/// tuple (`clippy::result_large_err`), and the two can't be reconciled
/// without converting one representation into the other.
fn internal(msg: String) -> (StatusCode, Json<Value>) {
    tracing::error!(error = %msg, "internal error");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "internal error"})),
    )
}

/// Load the catalog from the live plugin discovery paths (`./plugins`,
/// `~/.tumult/plugins`, `TUMULT_PLUGIN_PATH`). Discovery degrades bad
/// paths/manifests to warnings rather than failing wholesale; if it still
/// errors, serve an empty catalog — the UI's empty state explains how
/// plugin discovery is configured.
fn load_catalog() -> FaultCatalog {
    match tumult_authoring::build_catalog() {
        Ok(catalog) => catalog,
        Err(e) => {
            tracing::warn!(error = %e, "plugin discovery failed; serving empty catalog");
            FaultCatalog { domains: vec![] }
        }
    }
}

/// The catalog as JSON, mirroring the MCP tool's structured content:
/// `{action_count, domains}`.
#[must_use]
pub fn catalog_json(catalog: &FaultCatalog) -> Value {
    Value::Object(tumult_authoring::catalog_summary(catalog))
}

/// JSON body for `POST /api/authoring/scaffold`, mirroring the MCP
/// `tumult_scaffold_experiment` tool's argument schema.
#[derive(Debug, Deserialize)]
pub struct ScaffoldBody {
    /// Owning plugin (e.g. `tumult-containers`). Optional when `action` is
    /// fully qualified as `plugin::action`.
    plugin: Option<String>,
    /// Action name, or `plugin::action`.
    action: String,
    /// Argument values (numbers/booleans are stringified, then re-coerced
    /// by the builder).
    #[serde(default)]
    args: Map<String, Value>,
    /// Logical target of the fault.
    target: String,
    /// Shell command for the steady-state probe (mutually exclusive with
    /// `probe_url`; a default health check is used when both are absent).
    probe_command: Option<String>,
    /// HTTP URL for the steady-state probe.
    probe_url: Option<String>,
    /// Regex the probe output/response must match.
    probe_expect: Option<String>,
    /// Experiment title (defaults to `<action> — <target>`).
    title: Option<String>,
}

/// Scaffold an experiment from a catalog action via
/// [`tumult_authoring::scaffold_from_catalog`]: the generated TOON plus
/// whether it passes `tumult_core::engine::validate_experiment`, as
/// `{action, toon, valid, validation_error?}` — the MCP tool's structured
/// content. The action must exist in `catalog` and be a fault action
/// (probe-kind entries are rejected, same as the MCP tool).
///
/// # Errors
///
/// Returns a 400 body when the action is not in the catalog or when
/// neither `plugin` nor a `plugin::action` form is given, and a 500 when
/// TOON encoding fails.
pub fn scaffold_json(
    catalog: &FaultCatalog,
    req: &ScaffoldBody,
) -> Result<Value, (StatusCode, Json<Value>)> {
    let input = ScaffoldInput {
        plugin: req.plugin.as_deref(),
        action: &req.action,
        args: &req.args,
        target: &req.target,
        probe_command: req.probe_command.as_deref(),
        probe_url: req.probe_url.as_deref(),
        probe_expect: req.probe_expect.as_deref(),
        title: req.title.as_deref(),
    };
    tumult_authoring::scaffold_from_catalog(catalog, &input)
        .map(|outcome| Value::Object(outcome.to_json()))
        .map_err(|e| match e {
            ScaffoldError::UnqualifiedAction | ScaffoldError::UnknownAction(_) => {
                bad_request(e.to_string())
            }
            ScaffoldError::Encode(_) => internal(e.to_string()),
        })
}

/// `GET /api/authoring/catalog` — the live fault catalog (domains →
/// actions → documented args) for the UI's action picker. Discovery reads
/// plugin manifests from disk, so it runs on a blocking thread (the house
/// convention — all blocking store/disk work goes through
/// `spawn_blocking`, see `sql_util::with_reader`).
pub async fn catalog() -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    tokio::task::spawn_blocking(|| catalog_json(&load_catalog()))
        .await
        .map(Json)
        .map_err(|e| internal(format!("catalog task failed: {e}")))
}

/// `POST /api/authoring/scaffold` — generate experiment TOON from a
/// catalog action and its arguments. Pure generation: the UI registers
/// explicitly via `POST /api/runs/validate`. Runs on a blocking thread —
/// it does plugin discovery plus engine validation.
pub async fn scaffold(
    Json(req): Json<ScaffoldBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    tokio::task::spawn_blocking(move || scaffold_json(&load_catalog(), &req))
        .await
        .map_err(|e| internal(format!("scaffold task failed: {e}")))?
        .map(Json)
}
