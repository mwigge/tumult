//! Dry-run plan preview (`POST /api/runs/dry-run`) and the blast-radius
//! scope summary it carries.

use crate::error::ApiError;
use std::collections::HashMap;

use crate::auth::Principal;
use axum::extract::State;
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::ApiState;

/// JSON body: which registered definition, plus optional template variables.
#[derive(Debug, Deserialize)]
pub struct DryRunRequest {
    registry_id: String,
    #[serde(default)]
    vars: HashMap<String, String>,
    env: Option<String>,
    target: Option<String>,
}

/// `POST /api/runs/dry-run` — the resolved execution plan for a registered
/// definition (title, estimate, baseline, hypothesis probes, method steps in
/// order, guards, rollbacks) with nothing executed — the JSON counterpart of
/// the CLI's `--dry-run` output, with server-derived values masked. The
/// additive `scope` block summarizes the
/// blast radius for the launch preview: declared note, targeted fault
/// actions, guards and the concurrent-fault cap.
pub async fn dry_run(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Json(req): Json<DryRunRequest>,
) -> Result<Json<Value>, ApiError> {
    if req
        .env
        .as_deref()
        .is_some_and(|env| !principal.env_allowed(env))
    {
        return Err(crate::error::forbidden(
            "environment is outside the principal's scopes",
        ));
    }
    let def = super::registry_or_404(&state, &req.registry_id).await?;
    match crate::runs::prepare_for_api(&def.definition_toon, &req.vars) {
        Err(e) => Ok(Json(json!({"valid": false, "error": e}))),
        Ok((resolved, injected)) => {
            let execution_hash =
                tumult_ingest::execution_policy::execution_hash(&resolved, &injected)
                    .map_err(crate::error::bad_request)?;
            let context = req.env.as_deref().map(|env| {
                match tumult_ingest::execution_policy::classify_execution(&state.db_path, &resolved, &injected, env, req.target.as_deref(), !principal.env_scopes.is_empty()) {
                    Ok(tier) => json!({"env": env, "target": req.target, "tier": tier.as_str(), "error": null}),
                    Err(error) => json!({"env": env, "target": req.target, "error": error}),
                }
            });
            let experiment = super::preview_experiment(&def.definition_toon, &req.vars)
                .map_err(crate::error::bad_request)?;
            let mut plan = json!({
                "valid": true,
                "registry_id": def.id,
                "execution_hash": tumult_ingest::execution_policy::preview_token(&execution_hash),
                "execution_context": context,
                "plan": {
                    "title": experiment.title,
                    "description": experiment.description,
                    "tags": experiment.tags,
                    "estimate": experiment.estimate,
                    "baseline": experiment.baseline,
                    "hypothesis": experiment.steady_state_hypothesis,
                    "guards": experiment.guards,
                    "method": experiment.method,
                    "rollbacks": experiment.rollbacks,
                    "controls": experiment.controls,
                    "regulatory": experiment.regulatory,
                    "blast_radius": experiment.blast_radius,
                    "scope": scope_of(&experiment),
                },
            });
            if principal.role == tumult_auth::Role::Admin && principal.env_scopes.is_empty() {
                plan["binding_hash"] = json!(execution_hash);
            }
            Ok(Json(plan))
        }
    }
}

/// Provider argument keys that identify what a fault aims at; everything
/// else (durations, rates, flags) stays out of the scope summary.
const TARGET_ARG_KEYS: &[&str] = &[
    "container",
    "host",
    "selector",
    "process",
    "interface",
    "pod",
    "namespace",
];

/// The blast-radius summary of a resolved experiment: the declared note, the
/// fault-injecting method steps with their identifying arguments, the guards
/// and the concurrent-fault cap. Nulls and empty lists stand for "nothing
/// declared" — the block is always present.
fn scope_of(experiment: &tumult_core::types::Experiment) -> Value {
    let actions: Vec<Value> = experiment
        .method
        .iter()
        .filter(|a| a.activity_type == tumult_core::types::ActivityType::Action)
        .map(|a| {
            let (provider, action, targets) = provider_summary(&a.provider);
            json!({
                "step": a.name,
                "provider": provider,
                "action": action,
                "targets": targets,
            })
        })
        .collect();
    let guards: Vec<Value> = experiment
        .guards
        .iter()
        .map(|g| {
            json!({
                "name": g.name,
                "probe": g.probe.name,
                "min_breaches": g.min_breaches,
            })
        })
        .collect();
    json!({
        "blast_radius": experiment.blast_radius,
        "actions": actions,
        "guards": guards,
        "max_concurrent_faults": experiment.max_concurrent_faults,
    })
}

/// One-line provider identity plus the arguments that name its target.
fn provider_summary(provider: &tumult_core::types::Provider) -> (String, String, Value) {
    use tumult_core::types::Provider;
    match provider {
        Provider::Native {
            plugin,
            function,
            arguments,
        }
        | Provider::Script {
            plugin,
            function,
            arguments,
            ..
        } => {
            let targets: serde_json::Map<String, Value> = arguments
                .iter()
                .filter(|(k, _)| TARGET_ARG_KEYS.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            (plugin.clone(), function.clone(), Value::Object(targets))
        }
        Provider::Process { path, .. } => (String::from("process"), path.clone(), json!({})),
    }
}
