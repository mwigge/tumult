//! Causal attribution for deviations: which injected fault broke the run?
//!
//! The deviation node used to carry only a terminal status; auditors and the
//! lineage layer need more — which guard tripped, what was observed, and
//! (when it can be said *without guessing*) which fault caused it. The rules
//! here are deliberately conservative, mirroring `resolve_citation`: an
//! ambiguous situation produces no `caused_by` edge rather than a wrong one.

use tumult_core::types::{ActivityStatus, ActivityType, Experiment, Journal, Provider};

/// Enrichment for a deviation node, derived purely from the journal (and the
/// experiment definition when available).
#[derive(Debug, Clone)]
pub struct DeviationDetail {
    /// Node attrs: `status`, `failing_actions`, and a flattened `halt`
    /// record when the run was halted by a guard.
    pub attrs: serde_json::Value,
    /// Fault node ids attributed as causes — empty when attribution would
    /// be a guess.
    pub caused_by_fault_ids: Vec<String>,
}

/// Derive deviation detail from a journal.
///
/// Attribution rules, in order:
/// 1. Every failed/timed-out action maps to its fault id (unambiguous: the
///    activity itself broke).
/// 2. A guard-halted run with no failed action but exactly one injected
///    fault attributes to that fault (nothing else was running).
/// 3. Anything else: no attribution.
#[must_use]
pub fn deviation_detail(journal: &Journal, experiment: Option<&Experiment>) -> DeviationDetail {
    let failing: Vec<&str> = journal
        .method_results
        .iter()
        .filter(|r| {
            r.activity_type == ActivityType::Action
                && matches!(r.status, ActivityStatus::Failed | ActivityStatus::Timeout)
        })
        .map(|r| r.name.as_str())
        .collect();

    let mut attrs = serde_json::json!({
        "status": journal.status.to_string(),
        "failing_actions": failing,
    });
    if let Some(halt) = &journal.halt {
        attrs["halt"] = serde_json::json!({
            "guard_name": halt.guard_name,
            "observed": halt.observed,
            "safe_condition": halt.safe_condition,
            "breach_count": halt.breach_count,
            "breached_at_ns": halt.breached_at_ns,
        });
    }

    let mut caused_by: Vec<String> = failing
        .iter()
        .filter_map(|name| fault_id_for_action(name, experiment))
        .collect();
    caused_by.sort();
    caused_by.dedup();

    // Halted with no failed action: attribute only when a single fault was
    // injected — the sole fault at the moment the guard tripped.
    if caused_by.is_empty() && journal.halt.is_some() {
        let injected = injected_fault_ids(journal, experiment);
        if injected.len() == 1 {
            attrs["attribution_note"] = serde_json::json!("sole injected fault at guard halt");
            caused_by = injected;
        }
    }

    DeviationDetail {
        attrs,
        caused_by_fault_ids: caused_by,
    }
}

/// The fault id an action activity is keyed under — must stay in lockstep
/// with how `map.rs` keys fault nodes.
fn fault_id_for_action(action_name: &str, experiment: Option<&Experiment>) -> Option<String> {
    match experiment {
        Some(exp) => exp
            .method
            .iter()
            .find(|a| a.activity_type == ActivityType::Action && a.name == action_name)
            .map(|a| match &a.provider {
                Provider::Native {
                    plugin, function, ..
                } => format!("fault:{plugin}::{function}"),
                Provider::Process { .. } => format!("fault:{}", a.name),
            }),
        // Journal-only ingestion keys faults by activity name.
        None => Some(format!("fault:{action_name}")),
    }
}

/// Every fault id the run injected, deduped.
fn injected_fault_ids(journal: &Journal, experiment: Option<&Experiment>) -> Vec<String> {
    let mut ids: Vec<String> = journal
        .method_results
        .iter()
        .filter(|r| r.activity_type == ActivityType::Action)
        .filter_map(|r| fault_id_for_action(&r.name, experiment))
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use tumult_core::types::{
        Activity, ActivityResult, ExperimentStatus, HaltRecord, SpanId, TraceId,
    };

    fn action_result(name: &str, status: ActivityStatus) -> ActivityResult {
        ActivityResult {
            name: name.into(),
            activity_type: ActivityType::Action,
            status,
            started_at_ns: 0,
            duration_ms: 1,
            output: None,
            error: None,
            trace_id: TraceId::default(),
            span_id: SpanId::default(),
        }
    }

    fn native_action(name: &str, plugin: &str, function: &str) -> Activity {
        Activity {
            name: name.into(),
            activity_type: ActivityType::Action,
            provider: Provider::Native {
                plugin: plugin.into(),
                function: function.into(),
                arguments: std::collections::HashMap::new(),
            },
            ..Default::default()
        }
    }

    fn journal(
        status: ExperimentStatus,
        results: Vec<ActivityResult>,
        halt: Option<HaltRecord>,
    ) -> Journal {
        Journal {
            experiment_title: "t".into(),
            experiment_id: "id1".into(),
            status,
            started_at_ns: 0,
            ended_at_ns: 0,
            duration_ms: 0,
            steady_state_before: None,
            steady_state_after: None,
            method_results: results,
            rollback_results: vec![],
            rollback_failures: 0,
            estimate: None,
            baseline_result: None,
            during_result: None,
            post_result: None,
            load_result: None,
            analysis: None,
            regulatory: None,
            halt,
            blast_radius: None,
        }
    }

    fn halted_journal(results: Vec<ActivityResult>) -> Journal {
        journal(
            ExperimentStatus::Halted,
            results,
            Some(HaltRecord {
                guard_name: "p95_latency".into(),
                observed: Some("0.31".into()),
                safe_condition: "range [0, 0.05]".into(),
                breach_count: 3,
                breached_at_ns: 42,
                time_to_halt_ms: 800,
                rollback_ms: 90,
            }),
        )
    }

    #[test]
    fn failed_action_attributes_to_its_fault() {
        let exp = Experiment {
            method: vec![
                native_action("kill-db", "tumult-postgres", "kill_connections"),
                native_action("latency", "tumult-net", "inject_latency"),
            ],
            ..Default::default()
        };
        let journal = journal(
            ExperimentStatus::Deviated,
            vec![
                action_result("kill-db", ActivityStatus::Failed),
                action_result("latency", ActivityStatus::Succeeded),
            ],
            None,
        );
        let detail = deviation_detail(&journal, Some(&exp));
        assert_eq!(
            detail.caused_by_fault_ids,
            vec!["fault:tumult-postgres::kill_connections"]
        );
        assert_eq!(detail.attrs["failing_actions"][0], "kill-db");
    }

    #[test]
    fn halt_with_single_fault_attributes_to_it() {
        let exp = Experiment {
            method: vec![native_action("latency", "tumult-net", "inject_latency")],
            ..Default::default()
        };
        let journal = halted_journal(vec![action_result("latency", ActivityStatus::Succeeded)]);
        let detail = deviation_detail(&journal, Some(&exp));
        assert_eq!(
            detail.caused_by_fault_ids,
            vec!["fault:tumult-net::inject_latency"]
        );
        assert_eq!(detail.attrs["halt"]["guard_name"], "p95_latency");
        assert_eq!(
            detail.attrs["attribution_note"],
            "sole injected fault at guard halt"
        );
    }

    #[test]
    fn halt_with_multiple_faults_stays_unattributed() {
        let exp = Experiment {
            method: vec![
                native_action("latency", "tumult-net", "inject_latency"),
                native_action("cpu", "tumult-stress", "cpu_load"),
            ],
            ..Default::default()
        };
        let journal = halted_journal(vec![
            action_result("latency", ActivityStatus::Succeeded),
            action_result("cpu", ActivityStatus::Succeeded),
        ]);
        let detail = deviation_detail(&journal, Some(&exp));
        assert!(
            detail.caused_by_fault_ids.is_empty(),
            "no guessing under ambiguity"
        );
        assert_eq!(detail.attrs["halt"]["guard_name"], "p95_latency");
    }

    #[test]
    fn journal_only_uses_activity_name_fault_ids() {
        let journal = journal(
            ExperimentStatus::Deviated,
            vec![action_result("kill-db", ActivityStatus::Failed)],
            None,
        );
        let detail = deviation_detail(&journal, None);
        assert_eq!(detail.caused_by_fault_ids, vec!["fault:kill-db"]);
    }
}
