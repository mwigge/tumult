//! `ChaosGraph` persistence and queries over the `graph_nodes` / `graph_edges`
//! tables.
//!
//! The graph model and SQL live in `tumult-graph`; this module is the thin
//! executor that binds parameters against the embedded `DuckDB` connection.

use std::collections::HashSet;

use duckdb::params;
use tumult_core::types::{Experiment, Journal};
use tumult_graph::{sql, EgoGraph, EgoTuple, GraphDelta, NodeSummary};

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

    /// Upsert the static `ComplianceArticle` nodes from the citation registry.
    ///
    /// These nodes are deterministic and independent of any run, so they are
    /// seeded at store-open / schema-migration time. Idempotent — nodes upsert
    /// on their primary key, so re-opening a store never duplicates them.
    pub(super) fn populate_compliance_articles(&self) -> Result<(), AnalyticsError> {
        for node in tumult_graph::compliance_article_nodes() {
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
        Ok(())
    }

    /// Distinct tested action names — the `activity_results.name` values of
    /// `action` activities. This is the "tested" set the coverage-gap
    /// derivation subtracts the plugin catalog against.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails.
    #[must_use = "callers must use the returned set of tested action names"]
    pub fn tested_action_names(&self) -> Result<HashSet<String>, AnalyticsError> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT name FROM activity_results WHERE activity_type = 'action'")?;
        let rows = stmt
            .query_map(params![], |row| row.get::<_, String>(0))?
            .collect::<Result<HashSet<_>, _>>()?;
        Ok(rows)
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

    /// Return node summaries of a given `kind`, optionally filtered by a
    /// case-insensitive label substring.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails.
    #[must_use = "callers must use the returned node summaries"]
    pub fn graph_query(
        &self,
        kind: &str,
        filter: Option<&str>,
    ) -> Result<Vec<NodeSummary>, AnalyticsError> {
        let sql = sql::nodes_by_kind(filter.is_some());
        let mut stmt = self.conn.prepare(&sql)?;
        let map_row = |row: &duckdb::Row<'_>| {
            Ok(NodeSummary {
                id: row.get(0)?,
                kind: row.get(1)?,
                label: row.get(2)?,
            })
        };
        let rows = if let Some(filter) = filter {
            let pattern = format!("%{filter}%");
            stmt.query_map(params![kind, pattern], map_row)?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(params![kind], map_row)?
                .collect::<Result<Vec<_>, _>>()?
        };
        Ok(rows)
    }

    /// Whether a node with `id` exists in the graph.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails.
    #[must_use = "callers must use the returned existence flag"]
    pub fn graph_node_exists(&self, id: &str) -> Result<bool, AnalyticsError> {
        let mut stmt = self.conn.prepare(sql::NODE_EXISTS)?;
        let count: i64 = stmt.query_row(params![id], |row| row.get(0))?;
        Ok(count > 0)
    }

    /// Return the ego sub-graph of `node_id`: the nodes reachable within
    /// `depth` (following edges in both directions) plus the `(src)-[rel]->(dst)`
    /// tuples among them, optionally filtered to a single relation.
    ///
    /// Returns `Ok(None)` when the node does not exist, so callers can surface a
    /// clean "unknown node" error.
    ///
    /// # Errors
    ///
    /// Returns an error if a query fails.
    #[must_use = "callers must use the returned ego sub-graph"]
    pub fn graph_neighbors(
        &self,
        node_id: &str,
        rel: Option<&str>,
        depth: i64,
    ) -> Result<Option<EgoGraph>, AnalyticsError> {
        if !self.graph_node_exists(node_id)? {
            return Ok(None);
        }
        let depth = depth.max(1);

        // A rel-filtered query follows only that relation, so the returned node
        // set is just what's reachable through it (e.g. the fault), not the
        // whole accumulating neighbourhood. Param order matches the SQL:
        // nodes  with_rel -> [center, rel, depth];  without -> [center, depth].
        let nodes_sql = sql::ego_nodes(rel.is_some());
        let mut stmt = self.conn.prepare(&nodes_sql)?;
        let map_node = |row: &duckdb::Row<'_>| {
            Ok(NodeSummary {
                id: row.get(0)?,
                kind: row.get(1)?,
                label: row.get(2)?,
            })
        };
        let nodes = if let Some(rel) = rel {
            stmt.query_map(params![node_id, rel, depth], map_node)?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(params![node_id, depth], map_node)?
                .collect::<Result<Vec<_>, _>>()?
        };

        // edges  with_rel -> [center, rel, depth, rel];  without -> [center, depth].
        let edges_sql = sql::ego_edges(rel.is_some());
        let mut stmt = self.conn.prepare(&edges_sql)?;
        let map_edge = |row: &duckdb::Row<'_>| {
            Ok(EgoTuple {
                src: row.get(0)?,
                rel: row.get(1)?,
                dst: row.get(2)?,
            })
        };
        let edges = if let Some(rel) = rel {
            stmt.query_map(params![node_id, rel, depth, rel], map_edge)?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(params![node_id, depth], map_edge)?
                .collect::<Result<Vec<_>, _>>()?
        };

        Ok(Some(EgoGraph {
            center: node_id.to_string(),
            nodes,
            edges,
        }))
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
    fn ingest_populates_graph_tables() {
        let s = AnalyticsStore::in_memory().unwrap();
        s.ingest_journal_with_experiment(
            &sample_journal("e1", ExperimentStatus::Completed),
            Some(&latency_experiment()),
        )
        .unwrap();

        let nodes = s.graph_query("experiment", None).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].id, "exp:Test e1");

        let faults = s.graph_query("fault", None).unwrap();
        assert_eq!(faults[0].label, "tumult-net::inject_latency");

        let services = s.graph_query("service", None).unwrap();
        assert_eq!(services[0].id, "svc:demo-app");
    }

    #[test]
    fn neighbors_of_experiment_returns_fault_service_and_journal() {
        let s = AnalyticsStore::in_memory().unwrap();
        s.ingest_journal_with_experiment(
            &sample_journal("e1", ExperimentStatus::Completed),
            Some(&latency_experiment()),
        )
        .unwrap();

        let ego = s
            .graph_neighbors("exp:Test e1", None, 1)
            .unwrap()
            .expect("experiment node must exist");
        let kinds: std::collections::HashSet<&str> =
            ego.nodes.iter().map(|n| n.kind.as_str()).collect();
        assert!(kinds.contains("fault"));
        assert!(kinds.contains("service"));
        assert!(kinds.contains("journal"));

        let rels: std::collections::HashSet<&str> =
            ego.edges.iter().map(|e| e.rel.as_str()).collect();
        assert!(rels.contains("injects"));
        assert!(rels.contains("targets"));
        assert!(rels.contains("yielded"));
    }

    #[test]
    fn neighbors_rel_filter_restricts_edges() {
        let s = AnalyticsStore::in_memory().unwrap();
        s.ingest_journal_with_experiment(
            &sample_journal("e1", ExperimentStatus::Completed),
            Some(&latency_experiment()),
        )
        .unwrap();

        let ego = s
            .graph_neighbors("exp:Test e1", Some("injects"), 1)
            .unwrap()
            .unwrap();
        assert!(ego.edges.iter().all(|e| e.rel == "injects"));
        assert!(!ego.edges.is_empty());
    }

    #[test]
    fn neighbors_unknown_node_is_none() {
        let s = AnalyticsStore::in_memory().unwrap();
        assert!(s.graph_neighbors("exp:nope", None, 1).unwrap().is_none());
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
    fn compliance_articles_seeded_on_open() {
        let s = AnalyticsStore::in_memory().unwrap();
        let articles = s.graph_query("compliance_article", None).unwrap();
        assert_eq!(
            articles.len(),
            tumult_graph::compliance_article_nodes().len()
        );
        // A well-known article id is present.
        assert!(articles.iter().any(|n| n.id == "compliance:DORA/Art.25"));
    }

    #[test]
    fn refresh_coverage_gaps_is_idempotent_and_queryable() {
        use tumult_graph::AvailableAction;
        let s = AnalyticsStore::in_memory().unwrap();
        let available = [
            AvailableAction::new("tumult-net", "inject_latency"),
            AvailableAction::new("tumult-net", "drop_packets"),
        ];
        let tested = std::collections::HashSet::new();
        let delta = tumult_graph::coverage_gap_delta(&available, &tested);

        s.refresh_coverage_gaps(&delta).unwrap();
        let gaps = s.graph_query("coverage_gap", None).unwrap();
        assert_eq!(gaps.len(), 2);

        // Re-running does not duplicate.
        s.refresh_coverage_gaps(&delta).unwrap();
        let gaps = s.graph_query("coverage_gap", None).unwrap();
        assert_eq!(gaps.len(), 2);

        // gap_in edge to the fault domain is traversable.
        let ego = s
            .graph_neighbors("gap:tumult-net::drop_packets", None, 1)
            .unwrap()
            .expect("gap node exists");
        assert!(ego
            .edges
            .iter()
            .any(|e| e.rel == "gap_in" && e.dst == "domain:tumult-net"));
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
