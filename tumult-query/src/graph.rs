//! `ChaosGraph` read queries over the `graph_nodes` / `graph_edges` tables.
//!
//! The graph model and SQL live in `tumult-graph`; this module is the thin
//! read executor that binds parameters against the store's `DuckDB`
//! connection. Writes (run ingest, coverage-gap refresh) stay on
//! [`tumult_lake::AnalyticsStore`].

use std::collections::HashSet;

use duckdb::params;
use tumult_graph::{sql, EgoGraph, EgoTuple, NodeSummary};
use tumult_lake::{AnalyticsError, AnalyticsStore};

/// Distinct tested action names — the `activity_results.name` values of
/// `action` activities. This is the "tested" set the coverage-gap
/// derivation subtracts the plugin catalog against.
///
/// # Errors
///
/// Returns an error if the query fails.
#[must_use = "callers must use the returned set of tested action names"]
pub fn tested_action_names(store: &AnalyticsStore) -> Result<HashSet<String>, AnalyticsError> {
    let mut stmt = store
        .__connection()
        .prepare("SELECT DISTINCT name FROM activity_results WHERE activity_type = 'action'")?;
    let rows = stmt
        .query_map(params![], |row| row.get::<_, String>(0))?
        .collect::<Result<HashSet<_>, _>>()?;
    Ok(rows)
}

/// Return node summaries of a given `kind`, optionally filtered by a
/// case-insensitive label substring.
///
/// # Errors
///
/// Returns an error if the query fails.
#[must_use = "callers must use the returned node summaries"]
pub fn graph_query(
    store: &AnalyticsStore,
    kind: &str,
    filter: Option<&str>,
) -> Result<Vec<NodeSummary>, AnalyticsError> {
    let sql = sql::nodes_by_kind(filter.is_some());
    let mut stmt = store.__connection().prepare(&sql)?;
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
fn node_exists(store: &AnalyticsStore, id: &str) -> Result<bool, AnalyticsError> {
    let mut stmt = store.__connection().prepare(sql::NODE_EXISTS)?;
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
    store: &AnalyticsStore,
    node_id: &str,
    rel: Option<&str>,
    depth: i64,
) -> Result<Option<EgoGraph>, AnalyticsError> {
    if !node_exists(store, node_id)? {
        return Ok(None);
    }
    let depth = depth.max(1);
    let conn = store.__connection();

    // A rel-filtered query follows only that relation, so the returned node
    // set is just what's reachable through it (e.g. the fault), not the
    // whole accumulating neighbourhood. Param order matches the SQL:
    // nodes  with_rel -> [center, rel, depth];  without -> [center, depth].
    let nodes_sql = sql::ego_nodes(rel.is_some());
    let mut stmt = conn.prepare(&nodes_sql)?;
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
    let mut stmt = conn.prepare(&edges_sql)?;
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

#[cfg(test)]
mod tests {
    use tumult_core::types::*;

    use super::*;
    use crate::sample_journal;

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

        let nodes = graph_query(&s, "experiment", None).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].id, "exp:Test e1");

        let faults = graph_query(&s, "fault", None).unwrap();
        assert_eq!(faults[0].label, "tumult-net::inject_latency");

        let services = graph_query(&s, "service", None).unwrap();
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

        let ego = graph_neighbors(&s, "exp:Test e1", None, 1)
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

        let ego = graph_neighbors(&s, "exp:Test e1", Some("injects"), 1)
            .unwrap()
            .unwrap();
        assert!(ego.edges.iter().all(|e| e.rel == "injects"));
        assert!(!ego.edges.is_empty());
    }

    #[test]
    fn neighbors_unknown_node_is_none() {
        let s = AnalyticsStore::in_memory().unwrap();
        assert!(graph_neighbors(&s, "exp:nope", None, 1).unwrap().is_none());
    }

    #[test]
    fn tested_action_names_reflects_ingested_actions() {
        let s = AnalyticsStore::in_memory().unwrap();
        assert!(tested_action_names(&s).unwrap().is_empty());

        s.ingest_journal_with_experiment(
            &sample_journal("e1", ExperimentStatus::Completed),
            Some(&latency_experiment()),
        )
        .unwrap();

        let names = tested_action_names(&s).unwrap();
        assert!(names.contains("action-1"));
    }

    #[test]
    fn graph_query_filter_matches_label_case_insensitively() {
        let s = AnalyticsStore::in_memory().unwrap();
        s.ingest_journal_with_experiment(
            &sample_journal("e1", ExperimentStatus::Completed),
            Some(&latency_experiment()),
        )
        .unwrap();

        let hits = graph_query(&s, "service", Some("DEMO-APP")).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "svc:demo-app");

        let misses = graph_query(&s, "service", Some("no-such-service")).unwrap();
        assert!(misses.is_empty());
    }

    #[test]
    fn compliance_articles_seeded_on_open() {
        let s = AnalyticsStore::in_memory().unwrap();
        let articles = graph_query(&s, "compliance_article", None).unwrap();
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
        let gaps = graph_query(&s, "coverage_gap", None).unwrap();
        assert_eq!(gaps.len(), 2);

        // Re-running does not duplicate.
        s.refresh_coverage_gaps(&delta).unwrap();
        let gaps = graph_query(&s, "coverage_gap", None).unwrap();
        assert_eq!(gaps.len(), 2);

        // gap_in edge to the fault domain is traversable.
        let ego = graph_neighbors(&s, "gap:tumult-net::drop_packets", None, 1)
            .unwrap()
            .expect("gap node exists");
        assert!(ego
            .edges
            .iter()
            .any(|e| e.rel == "gap_in" && e.dst == "domain:tumult-net"));
    }
}
