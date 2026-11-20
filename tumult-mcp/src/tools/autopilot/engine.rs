//! The autopilot engine: assemble candidates from the lineage, gather
//! ambient context, gate, persist (audit-before-act), and — only for an
//! `enact` verdict with `execute` — run the playbook experiment.
//!
//! Reproducibility: everything the gate saw (candidate, ambient, autonomy,
//! policy hash) is persisted with the verdict, so any decision can be
//! replayed bit-for-bit against the corresponding policy text.

use tumult_autopilot::{
    class_key, evaluate, validate, AmbientContext, AutonomyRecord, Candidate, ConfidenceTier,
    GateDecision, LoadedPolicy, Trigger, Verdict,
};
use tumult_core::types::{Experiment, StructuralFacts};
use tumult_graph::lineage::{compute_lineage, ControlServiceStatus, LineageCell};

use crate::error::ToolError;

const NANOS_PER_DAY: i64 = 86_400_000_000_000;

pub(super) fn elapsed_days(now_ns: i64, then_ns: i64) -> u32 {
    let days = now_ns.saturating_sub(then_ns).div_euclid(NANOS_PER_DAY);
    u32::try_from(days).unwrap_or(u32::MAX)
}

pub(super) fn elapsed_hours(now_ns: i64, then_ns: i64) -> f64 {
    let nanos = u64::try_from(now_ns.saturating_sub(then_ns)).unwrap_or(0);
    std::time::Duration::from_nanos(nanos).as_secs_f64() / 3_600.0
}

use crate::tools::topology::inputs::{gather_inputs, recommendations_for, TopologyInputs};

/// A fully assembled decision, ready to persist and (maybe) enact.
pub(super) struct Assembled {
    pub candidate: Candidate,
    pub decision: GateDecision,
    pub autonomy_score: Option<f64>,
}

/// Confidence is derived from the deterministic score: a broken control or
/// a score at/above 1.0 is High; anything weaker is Directional.
pub(super) fn confidence_for(score: f64, broken: bool) -> ConfidenceTier {
    if broken || score >= 1.0 {
        ConfidenceTier::High
    } else {
        ConfidenceTier::Directional
    }
}

/// Read and parse a playbook experiment file; the error says why not.
fn load_playbook(path: &str) -> Result<Experiment, String> {
    let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    tumult_core::engine::parse_experiment(&content).map_err(|e| e.to_string())
}

/// Inspect a playbook experiment file for the validator's structural facts.
pub(super) fn inspect_experiment(path: &str) -> Result<StructuralFacts, String> {
    load_playbook(path).map(|exp| exp.structural_facts())
}

/// The facts a candidate carries for its playbook: those of the file, or
/// none and the reason the file cannot be read, with the path as the policy
/// names it. No playbook, no facts and no error.
fn playbook_facts(path: Option<&str>) -> (StructuralFacts, Option<String>) {
    let Some(path) = path else {
        return (StructuralFacts::default(), None);
    };
    match inspect_experiment(path) {
        Ok(facts) => (facts, None),
        Err(error) => (StructuralFacts::default(), Some(format!("{path}: {error}"))),
    }
}

/// Dynamic guard-telemetry pre-flight: run the playbook's guard probe ONCE
/// and evaluate its tolerance — "can I actually see the blast I'm about to
/// cause?" A guard whose probe fails or errors here would be blind during
/// the run, which is the #1 reported failure mode of autonomous chaos
/// (stop conditions bound to dead telemetry). Returns:
/// - `Some(true)`  — probe executed and met its tolerance
/// - `Some(false)` — probe failed, errored, or tolerance unmet
/// - `None`        — no guard/probe to verify (gate downgrades on None)
pub(super) fn preflight_guard_telemetry(playbook_path: &str) -> Option<bool> {
    use tumult_core::engine::evaluate_tolerance;
    use tumult_core::runner::ActivityExecutor;

    let content = std::fs::read_to_string(playbook_path).ok()?;
    let experiment = tumult_core::engine::parse_experiment(&content).ok()?;
    let guard = experiment.guards.first()?;

    let outcome = crate::handler::ProcessExecutor::new().execute(&guard.probe);
    if !outcome.success {
        return Some(false);
    }
    let Some(tolerance) = guard.probe.tolerance.as_ref() else {
        // Validated experiments always carry guard tolerances; a missing one
        // means we cannot judge the signal — treat as unverified.
        return Some(false);
    };
    let actual = serde_json::Value::String(outcome.output.unwrap_or_default());
    Some(evaluate_tolerance(&actual, tolerance))
}

/// Latest `evidences` timestamp (ns) for any of a cell's experiments toward
/// its article — the freshness the staleness trigger measures.
pub(super) fn latest_evidence_ns(inputs: &TopologyInputs, cell: &LineageCell) -> Option<i64> {
    inputs
        .edges
        .iter()
        .filter(|e| {
            e.rel == "evidences" && e.dst == cell.article_id && cell.experiments.contains(&e.src)
        })
        .map(|e| e.ts)
        .max()
}

/// Whether an open deviation blocks a candidate: a Broken lineage cell on
/// its service means the system is already degraded, so nothing new is
/// injected there. A broken-control candidate is exempt, because
/// revalidating the break is the point of it.
pub(super) fn open_deviation_blocks(
    lineage: &[LineageCell],
    service_id: &str,
    trigger: &Trigger,
) -> bool {
    if matches!(trigger, Trigger::BrokenControl { .. }) {
        return false;
    }
    lineage
        .iter()
        .any(|c| c.service_id == service_id && c.status == ControlServiceStatus::Broken)
}

/// Assemble gate-ready candidates from the store. Pure with respect to the
/// clock: `now_ns` and ambient facts are passed in. `concurrent_experiments`
/// is the server-wide enactment-ledger reading — while another enactment is
/// in flight it is 1, so the `ambient.no_concurrent_experiment` veto fires.
#[allow(clippy::too_many_lines)]
pub(super) fn assemble_candidates(
    store: &tumult_lake::AnalyticsStore,
    policy: &LoadedPolicy,
    now_ns: i64,
    within_business_hours: bool,
    limit: usize,
    concurrent_experiments: u32,
) -> Result<Vec<Assembled>, ToolError> {
    let inputs = gather_inputs(store)?;
    let lineage = compute_lineage(&inputs.lineage_input(), None, None);
    let recommendations = recommendations_for(store, &inputs, &lineage, limit)?;

    let class_history = tumult_query::autopilot_class_history(store)
        .map_err(|e| ToolError::Store(e.to_string()))?;
    let runs_today =
        tumult_query::autopilot_decisions_since(store, now_ns - 24 * 3_600_000_000_000)
            .map_err(|e| ToolError::Store(e.to_string()))?;

    let tier_of = |service_id: &str| -> Option<String> {
        inputs
            .services_with_attrs
            .iter()
            .find(|(n, _)| n.id == service_id)
            .and_then(|(_, attrs)| attrs.get("tier").and_then(|t| t.as_str()).map(String::from))
    };

    let mut out = Vec::new();
    for rec in recommendations {
        let cell = lineage
            .iter()
            .find(|c| c.article_id == rec.article_id && c.service_id == rec.service_id);
        let broken = cell.is_some_and(|c| c.status == ControlServiceStatus::Broken);
        let trigger = if broken {
            Trigger::BrokenControl {
                article_id: rec.article_id.clone(),
            }
        } else {
            // No evidence at all reads as "stale since forever" — u32::MAX
            // is the documented sentinel for never-evidenced.
            let age_days = cell
                .and_then(|c| latest_evidence_ns(&inputs, c))
                .map_or(u32::MAX, |ts| elapsed_days(now_ns, ts));
            Trigger::Staleness {
                article_id: rec.article_id.clone(),
                age_days,
            }
        };

        let service_name = rec
            .service_id
            .strip_prefix("svc:")
            .unwrap_or(&rec.service_id);
        let playbook = policy
            .policy
            .playbook_for(&rec.plugin, &rec.action, Some(service_name))
            .map(|p| p.experiment.clone());
        let (facts, playbook_error) = playbook_facts(playbook.as_deref());

        let tier = tier_of(&rec.service_id);
        let candidate = Candidate {
            id: uuid::Uuid::new_v4().to_string(),
            service_id: rec.service_id.clone(),
            tier: tier.clone(),
            plugin: rec.plugin.clone(),
            action: rec.action.clone(),
            article_id: rec.article_id.clone(),
            score: rec.score,
            reasons: rec.reasons.clone(),
            confidence: confidence_for(rec.score, broken),
            playbook_experiment: playbook,
            experiment_has_guard: facts.has_guard,
            experiment_has_rollback: facts.has_rollback,
            experiment_has_steady_state: facts.has_steady_state,
            experiment_fault_count: facts.fault_count,
            playbook_error,
            trigger,
        };

        let hours_since_last =
            tumult_query::autopilot_last_enacted_on(store, &candidate.service_id)
                .map_err(|e| ToolError::Store(e.to_string()))?
                .map(|ts| elapsed_hours(now_ns, ts));
        let open_deviation =
            open_deviation_blocks(&lineage, &candidate.service_id, &candidate.trigger);
        let ambient = AmbientContext {
            open_deviation_for_target: open_deviation,
            runs_today,
            hours_since_last_run_on_service: hours_since_last,
            within_business_hours,
            concurrent_experiments,
            // Dynamic pre-flight: actually run the guard probe once. Only
            // attempted when a guard exists — probing hopeless candidates
            // would waste probe timeouts.
            guard_telemetry_ok: if facts.has_guard {
                candidate
                    .playbook_experiment
                    .as_deref()
                    .and_then(preflight_guard_telemetry)
            } else {
                None
            },
        };

        let key = class_key(&candidate.plugin, &candidate.action, tier.as_deref());
        let record = class_history
            .iter()
            .find(|h| h.class_key == key)
            .map(|h| AutonomyRecord {
                enacted_total: h.enacted_total,
                enacted_clean: h.enacted_clean,
            });
        let autonomy_score = record.as_ref().and_then(|r| {
            (r.enacted_total > 0).then(|| f64::from(r.enacted_clean) / f64::from(r.enacted_total))
        });

        let validator = validate(&candidate);
        let decision = evaluate(
            &policy.policy,
            &candidate,
            &ambient,
            record.as_ref(),
            &validator,
        );
        out.push(Assembled {
            candidate,
            decision,
            autonomy_score,
        });
    }

    // ── Change-event candidates ─────────────────────────────────────────
    // A recorded deploy/config change invalidates a service's evidence even
    // when the lineage still looks fresh (Azure mission-critical guidance:
    // change-triggered invalidation, not just time-triggered). For each
    // service with a change event in the last 7 days and a matching
    // playbook, propose revalidating the article the playbook evidences —
    // unless a recommendation-based candidate for that service already ran
    // this pass.
    let seen_services: std::collections::HashSet<String> =
        out.iter().map(|a| a.candidate.service_id.clone()).collect();
    let week_ago = now_ns - 7 * 24 * 3_600_000_000_000;
    let mut seen_change: std::collections::HashSet<String> = std::collections::HashSet::new();
    for event in tumult_query::change_events_since(store, week_ago)
        .map_err(|e| ToolError::Store(e.to_string()))?
    {
        let service_id = if event.service_id.starts_with("svc:") {
            event.service_id.clone()
        } else {
            format!("svc:{}", event.service_id)
        };
        if seen_services.contains(&service_id) || !seen_change.insert(service_id.clone()) {
            continue;
        }
        let service_name = service_id.strip_prefix("svc:").unwrap_or(&service_id);
        for pb in &policy.policy.playbook {
            if pb.service.as_deref() != Some(service_name) {
                continue;
            }
            let article_id = match playbook_article(&pb.experiment) {
                Ok(Some(article_id)) => article_id,
                Ok(None) => continue,
                Err(error) => {
                    tracing::warn!(
                        name: "mcp.autopilot.playbook.skipped",
                        { file.path = %pb.experiment, error.message = %error },
                        "change-event revalidation skipped: the playbook cannot be read"
                    );
                    continue;
                }
            };
            let (facts, playbook_error) = playbook_facts(Some(&pb.experiment));
            let candidate = Candidate {
                id: uuid::Uuid::new_v4().to_string(),
                service_id: service_id.clone(),
                tier: tier_of(&service_id),
                plugin: pb.plugin.clone(),
                action: pb.action.clone(),
                article_id,
                score: 1.0,
                reasons: vec![format!(
                    "change event from '{}' invalidates evidence for {service_id}",
                    event.source
                )],
                confidence: ConfidenceTier::High,
                playbook_experiment: Some(pb.experiment.clone()),
                experiment_has_guard: facts.has_guard,
                experiment_has_rollback: facts.has_rollback,
                experiment_has_steady_state: facts.has_steady_state,
                experiment_fault_count: facts.fault_count,
                playbook_error,
                trigger: Trigger::ChangeEvent {
                    source: event.source.clone(),
                    detail: event.detail.clone(),
                },
            };
            let hours_since_last =
                tumult_query::autopilot_last_enacted_on(store, &candidate.service_id)
                    .map_err(|e| ToolError::Store(e.to_string()))?
                    .map(|ts| elapsed_hours(now_ns, ts));
            let open_deviation =
                open_deviation_blocks(&lineage, &candidate.service_id, &candidate.trigger);
            let ambient = AmbientContext {
                open_deviation_for_target: open_deviation,
                runs_today,
                hours_since_last_run_on_service: hours_since_last,
                within_business_hours,
                concurrent_experiments,
                guard_telemetry_ok: if facts.has_guard {
                    preflight_guard_telemetry(&pb.experiment)
                } else {
                    None
                },
            };
            let key = class_key(
                &candidate.plugin,
                &candidate.action,
                candidate.tier.as_deref(),
            );
            let record =
                class_history
                    .iter()
                    .find(|h| h.class_key == key)
                    .map(|h| AutonomyRecord {
                        enacted_total: h.enacted_total,
                        enacted_clean: h.enacted_clean,
                    });
            let autonomy_score = record.as_ref().and_then(|r| {
                (r.enacted_total > 0)
                    .then(|| f64::from(r.enacted_clean) / f64::from(r.enacted_total))
            });
            let validator = validate(&candidate);
            let decision = evaluate(
                &policy.policy,
                &candidate,
                &ambient,
                record.as_ref(),
                &validator,
            );
            out.push(Assembled {
                candidate,
                decision,
                autonomy_score,
            });
            break; // one candidate per changed service
        }
    }
    Ok(out)
}

/// Re-run the full gate for a persisted decision against CURRENT state.
///
/// This is the approval re-gate: an `approve` recorded minutes or days ago
/// must never execute on stale facts. Everything the original pass computed
/// is recomputed here — the experiment file is re-inspected (its content may
/// have changed), the ambient snapshot is re-read from the store, the guard
/// telemetry pre-flight probe runs once more, and the autonomy record is
/// re-aggregated. `concurrent_experiments` is the enactment-ledger reading,
/// exactly as in [`assemble_candidates`]. The trigger is read back from the
/// record, so a broken-control decision stays exempt from the open
/// deviation it was recorded to revalidate.
///
/// The policy is supplied by the caller (the respond tool requires it on
/// approve); the caller also verifies its hash against the decision record.
pub(super) fn regate_decision(
    store: &tumult_lake::AnalyticsStore,
    policy: &LoadedPolicy,
    record: &tumult_lake::DecisionRecord,
    now_ns: i64,
    within_business_hours: bool,
    concurrent_experiments: u32,
) -> Result<GateDecision, ToolError> {
    let inputs = gather_inputs(store)?;
    let lineage = compute_lineage(&inputs.lineage_input(), None, None);

    // Re-inspect the playbook file as it exists NOW — a playbook edited
    // since the decision is gated on its current content, not the original.
    let (facts, playbook_error) = playbook_facts(record.playbook.as_deref());
    let candidate = Candidate {
        id: record.id.clone(),
        service_id: record.service_id.clone(),
        tier: record.tier.clone(),
        plugin: record.plugin.clone(),
        action: record.action.clone(),
        article_id: record.article_id.clone(),
        score: record.score,
        reasons: Vec::new(),
        confidence: if record.confidence == "high" {
            ConfidenceTier::High
        } else {
            ConfidenceTier::Directional
        },
        playbook_experiment: record.playbook.clone(),
        experiment_has_guard: facts.has_guard,
        experiment_has_rollback: facts.has_rollback,
        experiment_has_steady_state: facts.has_steady_state,
        experiment_fault_count: facts.fault_count,
        playbook_error,
        trigger: trigger_of_record(&record.trigger, &record.article_id),
    };

    let hours_since_last = tumult_query::autopilot_last_enacted_on(store, &candidate.service_id)
        .map_err(|e| ToolError::Store(e.to_string()))?
        .map(|ts| elapsed_hours(now_ns, ts));
    let open_deviation = open_deviation_blocks(&lineage, &candidate.service_id, &candidate.trigger);
    let runs_today =
        tumult_query::autopilot_decisions_since(store, now_ns - 24 * 3_600_000_000_000)
            .map_err(|e| ToolError::Store(e.to_string()))?;
    let ambient = AmbientContext {
        open_deviation_for_target: open_deviation,
        runs_today,
        hours_since_last_run_on_service: hours_since_last,
        within_business_hours,
        concurrent_experiments,
        guard_telemetry_ok: if facts.has_guard {
            candidate
                .playbook_experiment
                .as_deref()
                .and_then(preflight_guard_telemetry)
        } else {
            None
        },
    };

    let key = class_key(
        &candidate.plugin,
        &candidate.action,
        candidate.tier.as_deref(),
    );
    let autonomy = tumult_query::autopilot_class_history(store)
        .map_err(|e| ToolError::Store(e.to_string()))?
        .into_iter()
        .find(|h| h.class_key == key)
        .map(|h| AutonomyRecord {
            enacted_total: h.enacted_total,
            enacted_clean: h.enacted_clean,
        });

    let validator = validate(&candidate);
    Ok(evaluate(
        &policy.policy,
        &candidate,
        &ambient,
        autonomy.as_ref(),
        &validator,
    ))
}

/// The compliance article a playbook experiment evidences (its first
/// resolvable regulatory requirement) — what a change-event revalidation
/// re-proves. The error says why the file cannot be read.
pub(super) fn playbook_article(experiment_path: &str) -> Result<Option<String>, String> {
    let exp = load_playbook(experiment_path)?;
    let Some(regulatory) = exp.regulatory.as_ref() else {
        return Ok(None);
    };
    for framework in &regulatory.frameworks {
        for req in &regulatory.requirements {
            if let Some(citation) = tumult_graph::resolve_citation(framework, &req.id) {
                return Ok(Some(tumult_graph::compliance_article_id(
                    citation.framework,
                    citation.control_id,
                )));
            }
        }
    }
    Ok(None)
}

/// The trigger kind as a decision record stores it. [`trigger_of_record`]
/// reads it back.
pub(super) fn trigger_token(trigger: &Trigger) -> &'static str {
    match trigger {
        Trigger::Staleness { .. } => "staleness",
        Trigger::BrokenControl { .. } => "broken_control",
        Trigger::Manual => "manual",
        Trigger::ChangeEvent { .. } => "change_event",
        Trigger::Risk { .. } => "risk",
    }
}

/// The trigger of a decision record, as far as the record holds it. The
/// record keeps the kind and the article, and only a broken control needs
/// more than that at the re-gate: the article whose break it revalidates.
/// Every other kind, and a kind this build does not know, reads back as
/// [`Trigger::Manual`]: the run an operator approved, with no exemption.
pub(super) fn trigger_of_record(token: &str, article_id: &str) -> Trigger {
    match token {
        "broken_control" => Trigger::BrokenControl {
            article_id: article_id.to_string(),
        },
        _ => Trigger::Manual,
    }
}

/// Persist one decision (audit-before-act) and mirror it into the graph.
pub(super) fn persist_decision(
    store: &tumult_lake::AnalyticsStore,
    policy: &LoadedPolicy,
    assembled: &Assembled,
    now_ns: i64,
) -> Result<(), ToolError> {
    let c = &assembled.candidate;
    let verdict_token = match &assembled.decision.verdict {
        Verdict::Enact => "enact",
        Verdict::Downgrade { .. } => "downgrade",
        Verdict::Propose { .. } => "propose",
        Verdict::Veto { .. } => "veto",
    };
    let gate_detail = match &assembled.decision.verdict {
        Verdict::Enact => serde_json::json!({}),
        Verdict::Downgrade { reasons } | Verdict::Propose { reasons } => {
            serde_json::json!({ "reasons": reasons })
        }
        Verdict::Veto { rule } => serde_json::json!({ "rule": rule }),
    };
    let validator = validate(c);

    let record = tumult_lake::DecisionRecord {
        id: c.id.clone(),
        decided_at_ns: now_ns,
        trigger: trigger_token(&c.trigger).into(),
        service_id: c.service_id.clone(),
        tier: c.tier.clone(),
        plugin: c.plugin.clone(),
        action: c.action.clone(),
        article_id: c.article_id.clone(),
        score: c.score,
        reasons: serde_json::json!(c.reasons),
        confidence: format!("{:?}", c.confidence).to_lowercase(),
        playbook: c.playbook_experiment.clone(),
        validator: serde_json::json!({
            "hollow": validator.hollow,
            "blockers": validator.blockers,
            "enactable": validator.enactable,
        }),
        verdict: verdict_token.into(),
        gate_rules: serde_json::json!(assembled.decision.rules_evaluated),
        gate_detail,
        policy_hash: policy.policy_hash().to_string(),
        autonomy_score: assembled.autonomy_score,
    };
    store
        .insert_autopilot_decision(&record)
        .map_err(|e| ToolError::Store(e.to_string()))?;
    store
        .record_recommendation_node(
            &c.id,
            verdict_token,
            &serde_json::json!({
                "verdict": verdict_token,
                "service": c.service_id,
                "article": c.article_id,
                "plugin": c.plugin,
                "action": c.action,
                "score": c.score,
                "policy_hash": policy.policy_hash(),
            }),
        )
        .map_err(|e| ToolError::Store(e.to_string()))?;
    Ok(())
}
