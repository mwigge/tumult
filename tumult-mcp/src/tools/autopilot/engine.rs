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
use tumult_graph::lineage::{compute_lineage, ControlServiceStatus, LineageCell};

use crate::error::ToolError;

const NANOS_PER_DAY: i64 = 86_400_000_000_000;

fn elapsed_days(now_ns: i64, then_ns: i64) -> u32 {
    let days = now_ns.saturating_sub(then_ns).div_euclid(NANOS_PER_DAY);
    u32::try_from(days).unwrap_or(u32::MAX)
}

fn elapsed_hours(now_ns: i64, then_ns: i64) -> f64 {
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
fn confidence_for(score: f64, broken: bool) -> ConfidenceTier {
    if broken || score >= 1.0 {
        ConfidenceTier::High
    } else {
        ConfidenceTier::Directional
    }
}

/// Inspect a playbook experiment file for the validator's structural facts.
fn inspect_experiment(path: &str) -> (bool, bool, bool, usize) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return (false, false, false, 0);
    };
    let Ok(exp) = tumult_core::engine::parse_experiment(&content) else {
        return (false, false, false, 0);
    };
    let has_steady = exp
        .steady_state_hypothesis
        .as_ref()
        .is_some_and(|h| !h.probes.is_empty());
    let has_rollback = !exp.rollbacks.is_empty();
    let has_guard = !exp.guards.is_empty();
    let faults = exp
        .method
        .iter()
        .filter(|a| a.activity_type == tumult_core::types::ActivityType::Action)
        .count();
    (has_steady, has_rollback && has_guard, has_guard, faults)
}

/// Dynamic guard-telemetry pre-flight: run the playbook's guard probe ONCE
/// and evaluate its tolerance — "can I actually see the blast I'm about to
/// cause?" A guard whose probe fails or errors here would be blind during
/// the run, which is the #1 reported failure mode of autonomous chaos
/// (stop conditions bound to dead telemetry). Returns:
/// - `Some(true)`  — probe executed and met its tolerance
/// - `Some(false)` — probe failed, errored, or tolerance unmet
/// - `None`        — no guard/probe to verify (gate downgrades on None)
fn preflight_guard_telemetry(playbook_path: &str) -> Option<bool> {
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
fn latest_evidence_ns(inputs: &TopologyInputs, cell: &LineageCell) -> Option<i64> {
    inputs
        .edges
        .iter()
        .filter(|e| {
            e.rel == "evidences" && e.dst == cell.article_id && cell.experiments.contains(&e.src)
        })
        .map(|e| e.ts)
        .max()
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

    let class_history = store
        .autopilot_class_history()
        .map_err(|e| ToolError::Store(e.to_string()))?;
    let runs_today = store
        .autopilot_decisions_since(now_ns - 24 * 3_600_000_000_000)
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
        let (has_steady, has_rollback, has_guard, fault_count) = playbook
            .as_deref()
            .map_or((false, false, false, 0), inspect_experiment);

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
            experiment_has_guard: has_guard,
            experiment_has_rollback: has_rollback,
            experiment_has_steady_state: has_steady,
            experiment_fault_count: fault_count,
            trigger,
        };

        // Ambient: the target has an "open deviation" when its most recent
        // relevant lineage state is Broken and nothing clean ran since.
        let hours_since_last = store
            .autopilot_last_enacted_on(&candidate.service_id)
            .map_err(|e| ToolError::Store(e.to_string()))?
            .map(|ts| elapsed_hours(now_ns, ts));
        let open_deviation = lineage.iter().any(|c| {
            c.service_id == candidate.service_id
                && c.status == ControlServiceStatus::Broken
                && !broken // revalidating the broken control itself is the point
        });
        let ambient = AmbientContext {
            open_deviation_for_target: open_deviation,
            runs_today,
            hours_since_last_run_on_service: hours_since_last,
            within_business_hours,
            concurrent_experiments,
            // Dynamic pre-flight: actually run the guard probe once. Only
            // attempted when a guard exists — probing hopeless candidates
            // would waste probe timeouts.
            guard_telemetry_ok: if has_guard {
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
    for event in store
        .change_events_since(week_ago)
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
            let Some(article_id) = playbook_article(&pb.experiment) else {
                continue;
            };
            let (has_steady, has_rollback, has_guard, fault_count) =
                inspect_experiment(&pb.experiment);
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
                experiment_has_guard: has_guard,
                experiment_has_rollback: has_rollback,
                experiment_has_steady_state: has_steady,
                experiment_fault_count: fault_count,
                trigger: Trigger::ChangeEvent {
                    source: event.source.clone(),
                    detail: event.detail.clone(),
                },
            };
            let hours_since_last = store
                .autopilot_last_enacted_on(&candidate.service_id)
                .map_err(|e| ToolError::Store(e.to_string()))?
                .map(|ts| elapsed_hours(now_ns, ts));
            let open_deviation = lineage.iter().any(|c| {
                c.service_id == candidate.service_id && c.status == ControlServiceStatus::Broken
            });
            let ambient = AmbientContext {
                open_deviation_for_target: open_deviation,
                runs_today,
                hours_since_last_run_on_service: hours_since_last,
                within_business_hours,
                concurrent_experiments,
                guard_telemetry_ok: if has_guard {
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
/// exactly as in [`assemble_candidates`].
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
    let (has_steady, has_rollback, has_guard, fault_count) = record
        .playbook
        .as_deref()
        .map_or((false, false, false, 0), inspect_experiment);
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
        experiment_has_guard: has_guard,
        experiment_has_rollback: has_rollback,
        experiment_has_steady_state: has_steady,
        experiment_fault_count: fault_count,
        trigger: Trigger::Manual,
    };

    let hours_since_last = store
        .autopilot_last_enacted_on(&candidate.service_id)
        .map_err(|e| ToolError::Store(e.to_string()))?
        .map(|ts| elapsed_hours(now_ns, ts));
    let open_deviation = lineage
        .iter()
        .any(|c| c.service_id == candidate.service_id && c.status == ControlServiceStatus::Broken);
    let runs_today = store
        .autopilot_decisions_since(now_ns - 24 * 3_600_000_000_000)
        .map_err(|e| ToolError::Store(e.to_string()))?;
    let ambient = AmbientContext {
        open_deviation_for_target: open_deviation,
        runs_today,
        hours_since_last_run_on_service: hours_since_last,
        within_business_hours,
        concurrent_experiments,
        guard_telemetry_ok: if has_guard {
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
    let autonomy = store
        .autopilot_class_history()
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
/// re-proves.
fn playbook_article(experiment_path: &str) -> Option<String> {
    let content = std::fs::read_to_string(experiment_path).ok()?;
    let exp = tumult_core::engine::parse_experiment(&content).ok()?;
    let regulatory = exp.regulatory.as_ref()?;
    for framework in &regulatory.frameworks {
        for req in &regulatory.requirements {
            if let Some(citation) = tumult_graph::resolve_citation(framework, &req.id) {
                return Some(tumult_graph::compliance_article_id(
                    citation.framework,
                    citation.control_id,
                ));
            }
        }
    }
    None
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
        trigger: match &c.trigger {
            Trigger::Staleness { .. } => "staleness".into(),
            Trigger::BrokenControl { .. } => "broken_control".into(),
            Trigger::Manual => "manual".into(),
            Trigger::ChangeEvent { .. } => "change_event".into(),
        },
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
