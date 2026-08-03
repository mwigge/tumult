//! Authoring endpoints (`/api/authoring*`) — the live fault catalog and
//! experiment scaffolding, thin REST wrappers over [`tumult_authoring`].
//!
//! These are the same code paths as the MCP `tumult_fault_catalog` /
//! `tumult_scaffold_experiment` tools and the CLI (`tumult new`), exposed
//! without an MCP hop so the web UI can browse the catalog and generate
//! experiment TOON in-process. Both endpoints are read-only w.r.t. the
//! store: scaffolding generates content but never persists — registration
//! stays behind `POST /api/runs/validate` (Operator role).

use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use tumult_authoring::builder::{
    build_experiment_unvalidated, encode_experiment, ProbeSpec, ScaffoldRequest,
};
use tumult_authoring::{ActionKind, FaultCatalog};

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
    json!({
        "action_count": catalog.action_count(),
        "domains": catalog.domains,
    })
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

/// Scaffold an experiment from a catalog action: the generated TOON plus
/// whether it passes `tumult_core::engine::validate_experiment`, as
/// `{action, toon, valid, validation_error?}` — the MCP tool's structured
/// content. The action must exist in `catalog`.
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
    let (plugin, action) = match &req.plugin {
        Some(p) => (p.clone(), req.action.clone()),
        None => match req.action.split_once("::") {
            Some((p, a)) => (p.to_string(), a.to_string()),
            None => {
                return Err(bad_request(
                    "provide `plugin`, or a fully-qualified `action` as plugin::action".into(),
                ));
            }
        },
    };

    let qualified = format!("{plugin}::{action}");
    // Only fault actions scaffold: a catalog probe used as the method's
    // action would still validate and could register a semantically wrong
    // experiment, so probes are rejected exactly like unknown names.
    let is_action = catalog
        .find(&plugin, &action)
        .is_some_and(|a| a.kind == ActionKind::Action);
    if !is_action {
        return Err(bad_request(format!("unknown action {qualified:?}")));
    }

    let mut args = indexmap::IndexMap::new();
    for (k, v) in &req.args {
        let value = match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        args.insert(k.clone(), value);
    }

    let probe = if let Some(url) = &req.probe_url {
        ProbeSpec::Http {
            url: url.clone(),
            expect: req.probe_expect.clone().unwrap_or_default(),
        }
    } else if let Some(command) = &req.probe_command {
        ProbeSpec::Exec {
            command: command.clone(),
            expect: req.probe_expect.clone().unwrap_or_else(|| ".".into()),
        }
    } else {
        ProbeSpec::default_for(&req.target)
    };

    let title = req
        .title
        .clone()
        .unwrap_or_else(|| format!("{action} — {}", req.target));

    let request = ScaffoldRequest {
        title,
        plugin,
        action,
        args,
        target: req.target.clone(),
        probe,
    };
    let experiment = build_experiment_unvalidated(&request);
    let validity = tumult_core::engine::validate_experiment(&experiment);
    let toon = encode_experiment(&experiment).map_err(|e| internal(e.to_string()))?;

    let mut body = json!({
        "action": qualified,
        "toon": toon,
        "valid": validity.is_ok(),
    });
    if let Err(e) = &validity {
        body["validation_error"] = json!(e.to_string());
    }
    Ok(body)
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
