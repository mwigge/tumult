//! Dry-run plan preview (`POST /api/runs/dry-run`) and the blast-radius
//! scope summary it carries.

use std::collections::HashMap;

use axum::extract::State;
use axum::response::Response;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::ApiState;

/// JSON body: which registered definition, plus optional template variables.
#[derive(Debug, Deserialize)]
pub struct DryRunRequest {
    registry_id: String,
    #[serde(default)]
    vars: HashMap<String, String>,
}

/// `POST /api/runs/dry-run` — the resolved execution plan for a registered
/// definition (title, estimate, baseline, hypothesis probes, method steps in
/// order, guards, rollbacks) with nothing executed — the JSON counterpart of
/// the CLI's `--dry-run` output. The additive `scope` block summarizes the
/// blast radius for the launch preview: declared note, targeted fault
/// actions, guards and the concurrent-fault cap.
pub async fn dry_run(
    State(state): State<ApiState>,
    Json(req): Json<DryRunRequest>,
) -> Result<Json<Value>, Response> {
    let def = super::registry_or_404(&state, &req.registry_id).await?;
    match tumult_ingest::prepare_run(&def.definition_toon, &req.vars) {
        Err(e) => Ok(Json(json!({"valid": false, "error": e}))),
        Ok((experiment, _env)) => Ok(Json(json!({
            "valid": true,
            "registry_id": def.id,
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
        }))),
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
