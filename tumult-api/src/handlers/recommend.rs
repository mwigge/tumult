//! `POST /api/risks/{id}/recommend` — risk → experiment → autopilot
//! candidate. A passive finding names what is missing; this turns it into
//! a registered experiment definition (scaffolded from the live fault
//! catalog, or the existing experiment the finding is about) and an
//! autopilot [`Candidate`] with a [`Trigger::Risk`] that the policy gate
//! evaluates and records when a policy is loaded. Operators only.

use std::collections::HashMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::Serialize;
use serde_json::{json, Map, Value};
use tumult_autopilot::{AmbientContext, Candidate, ConfidenceTier, Trigger};
use tumult_risk::{Risk, Severity};

use crate::auth::Principal;
use crate::error::{not_found, unavailable, ApiError};
use crate::sql_util::{internal, now_ns, with_reader};
use crate::ApiState;

/// What the finding asks for: a new scaffolded fault, or a rerun of the
/// experiment the finding is about.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum Recommendation {
    Scaffold {
        plugin: &'static str,
        action: &'static str,
        target: String,
        kind: String,
        rationale: String,
    },
    Rerun {
        experiment: String,
        rationale: String,
    },
}

/// The fault that covers a discipline nobody has run against the target.
fn fault_for_kind(kind: &str) -> Option<(&'static str, &'static str)> {
    match kind {
        "chaos" => Some(("tumult-process", "kill-process")),
        "dr" => Some(("tumult-containers", "stop-container")),
        "load" | "robustness" => Some(("tumult-stress", "cpu-stress")),
        "performance" | "nfr" => Some(("tumult-network", "add-latency")),
        _ => None,
    }
}

/// Map one finding to the experiment that would retire it. `None` means no
/// automated experiment fits (security findings, for one).
pub(crate) fn recommendation_for(risk: &Risk) -> Option<Recommendation> {
    let target = risk.target.clone().unwrap_or_else(|| risk.subject.clone());
    let kind = risk
        .recommended_kind
        .clone()
        .unwrap_or_else(|| "chaos".into());
    let scaffold = |plugin, action, rationale: String| {
        Some(Recommendation::Scaffold {
            plugin,
            action,
            target: target.clone(),
            kind: kind.clone(),
            rationale,
        })
    };
    let rerun = |rationale: String| {
        let experiment = risk.subject.split('/').next().unwrap_or("").to_string();
        if experiment.is_empty() {
            return None;
        }
        Some(Recommendation::Rerun {
            experiment,
            rationale,
        })
    };
    match risk.rule.as_str() {
        "discipline_gap" => {
            let (plugin, action) = fault_for_kind(&kind)?;
            scaffold(
                plugin,
                action,
                format!("no {kind} evidence for {target}; start with one bounded fault"),
            )
        }
        "stale_evidence" => {
            rerun("evidence is stale; rerun the latest experiment".into()).or_else(|| {
                let (plugin, action) = fault_for_kind(&kind)?;
                scaffold(
                    plugin,
                    action,
                    format!("{kind} evidence for {target} is stale"),
                )
            })
        }
        "latest_run_failed" | "objective_breached" | "recovery_target_missed" => {
            rerun("the latest run deviated; rerun after the fix to close the finding".into())
        }
        "never_run_definition" => rerun("a registered definition that never ran".into()),
        "untested_dependency" => scaffold(
            "tumult-network",
            "partition-host",
            format!("{target} is called but nothing has ever partitioned it"),
        ),
        "single_replica" | "no_pod_disruption_budget" => scaffold(
            "tumult-containers",
            "stop-container",
            format!("{target} cannot lose an instance; prove what happens when it does"),
        ),
        "no_resource_limits" => scaffold(
            "tumult-stress",
            "memory-stress",
            format!("{target} has no limits; find out what saturation does"),
        ),
        "no_readiness_probe" => scaffold(
            "tumult-process",
            "suspend-process",
            format!("{target} has no readiness probe; a frozen process keeps receiving traffic"),
        ),
        _ => None,
    }
}

fn confidence(risk: &Risk) -> ConfidenceTier {
    if risk.severity >= Severity::High {
        ConfidenceTier::High
    } else {
        ConfidenceTier::Directional
    }
}

/// Append the discipline and the target block the scaffolder does not
/// know about, so the registered definition scores and gates by target.
fn with_kind_and_target(toon: &str, kind: &str, target: &str, env: Option<&str>) -> String {
    let mut out = toon.trim_end().to_string();
    out.push_str(&format!("\nkind: {kind}\ntarget:\n  system: {target}\n"));
    if let Some(env) = env {
        out.push_str(&format!("  environment: {env}\n"));
    }
    out
}

struct Registered {
    id: String,
    name: String,
    registered: bool,
    toon: Option<String>,
    introspection: tumult_ingest::approvals::Introspection,
}

async fn register_scaffold(
    state: &ApiState,
    principal: &Principal,
    risk: &Risk,
    plugin: &str,
    action: &str,
    target: &str,
    kind: &str,
) -> Result<Registered, ApiError> {
    let body: crate::authoring::ScaffoldBody = serde_json::from_value(json!({
        "plugin": plugin,
        "action": action,
        "args": Map::<String, Value>::new(),
        "target": target,
        "title": format!("{} — {}", risk.title, target),
    }))
    .map_err(|e| internal(e.to_string()))?;
    let scaffolded = tokio::task::spawn_blocking(move || {
        crate::authoring::scaffold_json(&crate::authoring::load_catalog(), &body)
    })
    .await
    .map_err(|e| internal(format!("scaffold task failed: {e}")))?
    .map_err(|e| ApiError::from(axum::response::IntoResponse::into_response(e)))?;
    if scaffolded["valid"] != true {
        return Err(internal(format!(
            "scaffold did not validate: {}",
            scaffolded["validation_error"]
        )));
    }
    let toon = with_kind_and_target(
        scaffolded["toon"].as_str().unwrap_or_default(),
        kind,
        target,
        risk.environment.as_deref(),
    );
    let vars = HashMap::new();
    let experiment = crate::runs::preview_experiment(&toon, &vars).map_err(internal)?;
    let introspection = tumult_ingest::approvals::introspect(&experiment);
    let registration = crate::registry::register_definition(
        state,
        &toon,
        &experiment.title,
        principal.actor().or_else(|| Some("api".into())),
        false,
        "run registration is not wired (no ingest handle)",
    )
    .await?;
    Ok(Registered {
        id: registration.id,
        name: registration.name,
        registered: registration.registered,
        toon: Some(toon),
        introspection,
    })
}

async fn find_registered(state: &ApiState, name: &str) -> Result<Registered, ApiError> {
    let needle = name.to_string();
    let row = with_reader(&state.db_path, move |reader| {
        let sql = format!(
            "SELECT id, name, definition_toon FROM run_registry WHERE name = {} \
             ORDER BY registered_at_ns DESC LIMIT 1",
            crate::sql_util::sql_string(&needle)
        );
        reader
            .query_json_rows(&sql)
            .map(|rows| rows.into_iter().next())
            .map_err(|e| e.to_string())
    })
    .await?;
    let Some(row) = row else {
        return Err(not_found(format!(
            "no registered definition named {name}; register it first"
        )));
    };
    let toon = row["definition_toon"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let experiment = crate::runs::preview_experiment(&toon, &HashMap::new()).map_err(internal)?;
    Ok(Registered {
        id: row["id"].as_str().unwrap_or_default().to_string(),
        name: row["name"].as_str().unwrap_or_default().to_string(),
        registered: false,
        toon: None,
        introspection: tumult_ingest::approvals::introspect(&experiment),
    })
}

fn candidate_for(risk: &Risk, rec: &Recommendation, reg: &Registered) -> Candidate {
    let (plugin, action) = match rec {
        Recommendation::Scaffold { plugin, action, .. } => {
            ((*plugin).to_string(), (*action).to_string())
        }
        Recommendation::Rerun { .. } => reg
            .introspection
            .first_fault
            .clone()
            .unwrap_or_else(|| ("run".into(), "run".into())),
    };
    let rationale = match rec {
        Recommendation::Scaffold { rationale, .. } | Recommendation::Rerun { rationale, .. } => {
            rationale.clone()
        }
    };
    Candidate {
        id: uuid::Uuid::new_v4().to_string(),
        service_id: risk.target.clone().unwrap_or_else(|| risk.subject.clone()),
        tier: risk.environment.clone(),
        plugin,
        action,
        article_id: risk.id.clone(),
        score: f64::from(risk.rank.min(100)) / 100.0,
        reasons: vec![risk.title.clone(), rationale],
        confidence: confidence(risk),
        playbook_experiment: Some(reg.name.clone()),
        experiment_has_guard: reg.introspection.has_guard,
        experiment_has_rollback: reg.introspection.has_rollback,
        experiment_has_steady_state: reg.introspection.has_steady_state,
        experiment_fault_count: reg.introspection.fault_count,
        trigger: Trigger::Risk {
            risk_id: risk.id.clone(),
            rule: risk.rule.clone(),
        },
    }
}

/// Run the validator and, when a policy is loaded, the gate; the decision
/// is recorded on the single writer so the audit trail holds it.
async fn decide(
    state: &ApiState,
    risk: &Risk,
    candidate: &Candidate,
    open_deviation: bool,
) -> Result<(Value, Option<Value>), ApiError> {
    let validator = tumult_autopilot::validate(candidate);
    let validator_json = json!({
        "hollow": validator.hollow,
        "blockers": validator.blockers,
        "enactable": validator.enactable,
    });
    let Some(policy) = state.autopilot_policy() else {
        return Ok((validator_json, None));
    };
    let ambient = AmbientContext {
        open_deviation_for_target: open_deviation,
        runs_today: 0,
        hours_since_last_run_on_service: None,
        within_business_hours: true,
        concurrent_experiments: 0,
        // Unknown at pre-flight: the run-time approval gate re-evaluates
        // with live telemetry before anything is enacted.
        guard_telemetry_ok: None,
    };
    let decision =
        tumult_autopilot::evaluate(&policy.policy, candidate, &ambient, None, &validator);
    let verdict = match &decision.verdict {
        tumult_autopilot::Verdict::Enact => "enact",
        tumult_autopilot::Verdict::Downgrade { .. } => "downgrade",
        tumult_autopilot::Verdict::Propose { .. } => "propose",
        tumult_autopilot::Verdict::Veto { .. } => "veto",
    }
    .to_string();
    let decision_json = json!({
        "verdict": verdict,
        "detail": serde_json::to_value(&decision.verdict).map_err(|e| internal(e.to_string()))?,
        "rules_evaluated": decision.rules_evaluated,
        "policy_hash": policy.policy_hash(),
    });
    let Some(ingest) = state.ingest_handle() else {
        return Ok((validator_json, Some(decision_json)));
    };
    let record = tumult_lake::duckdb_store::autopilot::DecisionRecord {
        id: candidate.id.clone(),
        decided_at_ns: now_ns(),
        trigger: "risk".into(),
        service_id: candidate.service_id.clone(),
        tier: candidate.tier.clone(),
        plugin: candidate.plugin.clone(),
        action: candidate.action.clone(),
        article_id: risk.id.clone(),
        score: candidate.score,
        reasons: json!(candidate.reasons),
        confidence: serde_json::to_value(candidate.confidence)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default(),
        playbook: candidate.playbook_experiment.clone(),
        validator: validator_json.clone(),
        verdict,
        gate_rules: json!(decision.rules_evaluated),
        gate_detail: decision_json["detail"].clone(),
        policy_hash: policy.policy_hash().to_string(),
        autonomy_score: None,
    };
    ingest
        .write(tumult_ingest::Batch::Exec(Box::new(move |writer| {
            writer
                .insert_autopilot_decision(&record)
                .map_err(|e| e.to_string())
        })))
        .await
        .map_err(|e| internal(e.to_string()))?;
    Ok((validator_json, Some(decision_json)))
}

pub(crate) async fn recommend(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let (_, report) = super::risks::scan_for(&state, &principal, None).await?;
    let Some(risk) = report
        .risks
        .iter()
        .chain(report.suppressed.iter())
        .find(|r| r.id == id)
        .cloned()
    else {
        return Err(not_found(format!("no current risk with id {id}")));
    };
    let Some(rec) = recommendation_for(&risk) else {
        return Err(ApiError::from(axum::response::IntoResponse::into_response(
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({
                    "error": format!("no automated experiment fits rule {} ({})", risk.rule, risk.title)
                })),
            ),
        )));
    };
    if state.ingest_handle().is_none() {
        return Err(unavailable(
            "recommendations are not wired (no ingest handle)",
        ));
    }
    let registered = match &rec {
        Recommendation::Scaffold {
            plugin,
            action,
            target,
            kind,
            ..
        } => register_scaffold(&state, &principal, &risk, plugin, action, target, kind).await?,
        Recommendation::Rerun { experiment, .. } => find_registered(&state, experiment).await?,
    };
    let candidate = candidate_for(&risk, &rec, &registered);
    let open_deviation = report.risks.iter().any(|r| {
        r.rule == "latest_run_failed"
            && r.target == risk.target
            && r.environment == risk.environment
    });
    let (validator, decision) = decide(&state, &risk, &candidate, open_deviation).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "risk_id": risk.id,
            "rule": risk.rule,
            "recommendation": rec,
            "experiment": {
                "registry_id": registered.id,
                "name": registered.name,
                "registered": registered.registered,
                "toon": registered.toon,
            },
            "candidate": candidate,
            "validator": validator,
            "decision": decision,
        })),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn risk(rule: &str, kind: Option<&str>, subject: &str) -> Risk {
        Risk {
            id: "r_1".into(),
            rule: rule.into(),
            severity: Severity::High,
            rank: 80,
            title: "t".into(),
            detail: "d".into(),
            target: Some("database".into()),
            environment: Some("demo".into()),
            subject: subject.into(),
            recommended_kind: kind.map(str::to_owned),
            evidence: vec![],
            suppression: None,
        }
    }

    #[test]
    fn discipline_gaps_scaffold_a_fault_for_the_kind() {
        let rec = recommendation_for(&risk("discipline_gap", Some("dr"), "dr")).unwrap();
        match rec {
            Recommendation::Scaffold {
                plugin,
                action,
                target,
                kind,
                ..
            } => {
                assert_eq!((plugin, action), ("tumult-containers", "stop-container"));
                assert_eq!((target.as_str(), kind.as_str()), ("database", "dr"));
            }
            Recommendation::Rerun { .. } => panic!("expected a scaffold"),
        }
        assert!(
            recommendation_for(&risk("discipline_gap", Some("security"), "security")).is_none()
        );
    }

    #[test]
    fn evidence_findings_rerun_the_experiment_they_are_about() {
        let rec = recommendation_for(&risk("objective_breached", None, "pg-failover/p99")).unwrap();
        assert_eq!(
            rec,
            match rec {
                Recommendation::Rerun { ref experiment, .. } => {
                    assert_eq!(experiment, "pg-failover");
                    rec.clone()
                }
                Recommendation::Scaffold { .. } => panic!("expected a rerun"),
            }
        );
        assert!(matches!(
            recommendation_for(&risk("never_run_definition", None, "reg-1")).unwrap(),
            Recommendation::Rerun { ref experiment, .. } if experiment == "reg-1"
        ));
    }

    #[test]
    fn manifest_and_dependency_findings_pick_the_matching_fault() {
        let dep = risk("untested_dependency", Some("chaos"), "payments");
        let mut dep = dep;
        dep.target = None;
        assert!(matches!(
            recommendation_for(&dep).unwrap(),
            Recommendation::Scaffold { action: "partition-host", ref target, .. } if target == "payments"
        ));
        assert!(matches!(
            recommendation_for(&risk("no_resource_limits", None, "demo/api")).unwrap(),
            Recommendation::Scaffold {
                action: "memory-stress",
                ..
            }
        ));
        assert!(recommendation_for(&risk("unknown_rule", None, "x")).is_none());
    }

    #[test]
    fn kind_and_target_are_appended_to_the_definition() {
        let toon = with_kind_and_target("title: t\nmethod[0]:\n", "dr", "db", Some("demo"));
        assert!(toon.ends_with("kind: dr\ntarget:\n  system: db\n  environment: demo\n"));
        let bare = with_kind_and_target("title: t", "chaos", "db", None);
        assert!(bare.ends_with("kind: chaos\ntarget:\n  system: db\n"));
    }

    #[test]
    fn confidence_follows_severity() {
        let mut r = risk("discipline_gap", Some("chaos"), "chaos");
        assert_eq!(confidence(&r), ConfidenceTier::High);
        r.severity = Severity::Medium;
        assert_eq!(confidence(&r), ConfidenceTier::Directional);
    }
}
