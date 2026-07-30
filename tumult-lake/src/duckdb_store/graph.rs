//! `ChaosGraph` persistence over the `graph_nodes` / `graph_edges` tables.
//!
//! The graph model and SQL live in `tumult-graph`; this module is the thin
//! write-side executor that binds parameters against the embedded `DuckDB`
//! connection. The read side (`graph_query`, `graph_neighbors`, …) lives in
//! `tumult-query`.

use duckdb::params;
use tumult_core::types::{Experiment, Journal};
use tumult_graph::{sql, GraphDelta};

use crate::error::AnalyticsError;

use super::AnalyticsStore;

impl AnalyticsStore {
    /// Upsert the graph nodes/edges contributed by a run.
    ///
    /// Pass `Some(experiment)` for the full `Fault = plugin::function` +
    /// `Service` model; `None` derives faults from the journal's action
    /// results. Idempotent: a run's edges are cleared by `run_id` before
    /// re-insert, and nodes upsert on their primary key.
    pub(super) fn populate_graph(
        &self,
        journal: &Journal,
        experiment: Option<&Experiment>,
    ) -> Result<(), AnalyticsError> {
        let delta = tumult_graph::journal_to_graph(journal, experiment);

        for node in &delta.nodes {
            // `serde_json::Value`'s Display impl emits compact JSON text.
            // Merge attrs rather than replace: a run's (usually empty)
            // service attrs must never clobber declared-topology metadata.
            let attrs = node.attrs.to_string();
            self.conn.execute(
                sql::UPSERT_NODE_MERGE_ATTRS,
                params![node.id, node.kind.as_str(), node.label, attrs],
            )?;
        }

        // Clear this run's edges first so re-ingesting never duplicates them.
        self.conn
            .execute(sql::DELETE_EDGES_FOR_RUN, params![journal.experiment_id])?;
        for edge in &delta.edges {
            self.conn.execute(
                sql::INSERT_EDGE,
                params![
                    edge.src,
                    edge.rel.as_str(),
                    edge.dst,
                    journal.experiment_id,
                    journal.started_at_ns,
                    edge.attrs.to_string()
                ],
            )?;
        }
        Ok(())
    }

    /// Replace the coverage-gap sub-graph with a freshly derived [`GraphDelta`].
    ///
    /// The whole `coverage_gap` node set and the sentinel-`run_id` `gap_in`
    /// edges are cleared and re-inserted, so calling this repeatedly is
    /// idempotent and stale gaps are dropped. `FaultDomain` nodes are upserted
    /// (they are stable per plugin and may be shared).
    ///
    /// # Errors
    ///
    /// Returns an error if a delete or insert fails.
    pub fn refresh_coverage_gaps(&self, delta: &GraphDelta) -> Result<(), AnalyticsError> {
        // Clear the previous gap nodes and their edges.
        self.conn.execute(
            sql::DELETE_NODES_BY_KIND,
            params![tumult_graph::NodeKind::CoverageGap.as_str()],
        )?;
        self.conn.execute(
            sql::DELETE_EDGES_FOR_RUN,
            params![tumult_graph::COVERAGE_GAP_RUN_ID],
        )?;

        for node in &delta.nodes {
            self.conn.execute(
                sql::UPSERT_NODE,
                params![
                    node.id,
                    node.kind.as_str(),
                    node.label,
                    node.attrs.to_string()
                ],
            )?;
        }
        for edge in &delta.edges {
            self.conn.execute(
                sql::INSERT_EDGE,
                params![
                    edge.src,
                    edge.rel.as_str(),
                    edge.dst,
                    tumult_graph::COVERAGE_GAP_RUN_ID,
                    0_i64,
                    edge.attrs.to_string()
                ],
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::sample_journal;
    use super::super::AnalyticsStore;
    use tumult_core::types::*;

    fn latency_experiment() -> Experiment {
        Experiment {
            title: "Test e1".into(),
            method: vec![Activity {
                name: "inject-latency".into(),
                activity_type: ActivityType::Action,
                provider: Provider::Native {
                    plugin: "tumult-net".into(),
                    function: "inject_latency".into(),
                    arguments: std::collections::HashMap::from([(
                        "upstream".into(),
                        serde_json::Value::String("demo-app:8080".into()),
                    )]),
                },
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn reingest_does_not_duplicate_graph_nodes_or_edges() {
        let s = AnalyticsStore::in_memory().unwrap();
        let journal = sample_journal("e1", ExperimentStatus::Completed);
        let exp = latency_experiment();
        assert!(s
            .ingest_journal_with_experiment(&journal, Some(&exp))
            .unwrap());
        // Duplicate experiment_id: skipped, no extra graph rows.
        assert!(!s
            .ingest_journal_with_experiment(&journal, Some(&exp))
            .unwrap());

        let node_count = s
            .query("SELECT count(*) FROM graph_nodes WHERE kind NOT IN ('compliance_article', 'coverage_gap', 'fault_domain')")
            .unwrap();
        // exp + fault + service + journal = 4 run-derived nodes.
        assert_eq!(node_count[0][0], "4");
        let edge_count = s
            .query("SELECT count(DISTINCT (src, rel, dst)) FROM graph_edges")
            .unwrap();
        // injects + targets + yielded + observed_on = 4 edges.
        assert_eq!(edge_count[0][0], "4");
    }

    #[test]
    fn evidences_edge_attrs_persist() {
        use tumult_core::types::{RegulatoryMapping, RegulatoryRequirement};
        let s = AnalyticsStore::in_memory().unwrap();
        let mut exp = latency_experiment();
        exp.regulatory = Some(RegulatoryMapping {
            frameworks: vec!["DORA".into()],
            requirements: vec![RegulatoryRequirement {
                id: "Art. 25".into(),
                description: "d".into(),
                evidence: "e".into(),
            }],
        });
        s.ingest_journal_with_experiment(
            &sample_journal("e1", ExperimentStatus::Completed),
            Some(&exp),
        )
        .unwrap();

        let rows = s
            .query("SELECT attrs->>'strength' FROM graph_edges WHERE rel = 'evidences'")
            .unwrap();
        assert!(!rows.is_empty(), "an evidences edge must be recorded");
        assert_eq!(rows[0][0], "direct");
    }
}
