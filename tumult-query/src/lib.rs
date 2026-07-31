//! `tumult-query` — the read side of the unified lake: domain queries over
//! [`tumult_lake::AnalyticsStore`] for the `ChaosGraph`, the declared
//! topology, and the autopilot decision tables.
//!
//! Writes stay on [`tumult_lake::AnalyticsStore`] (ingest, topology import,
//! coverage-gap refresh, decision persistence). The free functions here only
//! read, so they work just as well on a store opened with
//! [`AnalyticsStore::open_read_only`](tumult_lake::AnalyticsStore::open_read_only)
//! — which is how the TUI and the MCP server's read paths hold the store, so
//! they coexist with a running writer.

pub mod autopilot;
pub mod graph;
pub mod topology;

pub use autopilot::{
    autopilot_class_history, autopilot_decision, autopilot_decisions, autopilot_decisions_since,
    autopilot_last_enacted_on, change_events_since,
};
pub use graph::{graph_neighbors, graph_query, tested_action_names};
pub use topology::{graph_edges_by_rels, graph_nodes_with_attrs, NodeAttrs};

#[cfg(test)]
pub(crate) fn sample_journal(
    id: &str,
    status: tumult_core::types::ExperimentStatus,
) -> tumult_core::types::Journal {
    use tumult_core::types::*;
    Journal {
        experiment_title: format!("Test {id}"),
        experiment_id: id.into(),
        status,
        started_at_ns: 1_774_980_000_000_000_000,
        ended_at_ns: 1_774_980_300_000_000_000,
        duration_ms: 300_000,
        steady_state_before: None,
        steady_state_after: None,
        method_results: vec![ActivityResult {
            name: "action-1".into(),
            activity_type: ActivityType::Action,
            status: ActivityStatus::Succeeded,
            started_at_ns: 1_774_980_135_000_000_000,
            duration_ms: 500,
            output: Some("done".into()),
            error: None,
            trace_id: "t1".into(),
            span_id: "s1".into(),
        }],
        rollback_results: vec![],
        rollback_failures: 0,
        halt: None,
        blast_radius: None,
        estimate: None,
        baseline_result: None,
        during_result: None,
        post_result: None,
        load_result: None,
        analysis: Some(AnalysisResult {
            estimate_accuracy: Some(1.0),
            estimate_recovery_delta_s: None,
            trend: None,
            resilience_score: Some(0.95),
        }),
        regulatory: None,
    }
}
