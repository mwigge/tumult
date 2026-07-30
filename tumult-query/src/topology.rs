//! Declared-topology readbacks: the edge/node queries that feed lineage and
//! recommendation computation. Executor only — the model, SQL and
//! derivations live in `tumult-graph`; the write side (`refresh_topology`)
//! stays on [`tumult_lake::AnalyticsStore`].

use duckdb::params;
use tumult_graph::{sql, EdgeRecord};
use tumult_lake::{AnalyticsError, AnalyticsStore};

/// A node id/label with its raw attrs JSON, as read back for lineage.
#[derive(Debug, Clone)]
pub struct NodeAttrs {
    pub id: String,
    pub label: String,
    /// Attrs as JSON text (`{}` when absent).
    pub attrs: String,
}

/// Full edge rows for a set of relations, oldest first.
///
/// # Errors
///
/// Returns an error if the query fails.
#[must_use = "callers must use the returned edge records"]
pub fn graph_edges_by_rels(
    store: &AnalyticsStore,
    rels: &[&str],
) -> Result<Vec<EdgeRecord>, AnalyticsError> {
    if rels.is_empty() {
        return Ok(Vec::new());
    }
    let query = sql::edges_by_rels(rels.len());
    let mut stmt = store.__connection().prepare(&query)?;
    let params_vec: Vec<&dyn duckdb::ToSql> =
        rels.iter().map(|r| r as &dyn duckdb::ToSql).collect();
    let rows = stmt
        .query_map(params_vec.as_slice(), |row| {
            Ok(EdgeRecord {
                src: row.get(0)?,
                rel: row.get(1)?,
                dst: row.get(2)?,
                run_id: row.get(3)?,
                ts: row.get(4)?,
                attrs: row.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Node id, label and attrs JSON for every node of a kind.
///
/// # Errors
///
/// Returns an error if the query fails.
#[must_use = "callers must use the returned nodes"]
pub fn graph_nodes_with_attrs(
    store: &AnalyticsStore,
    kind: &str,
) -> Result<Vec<NodeAttrs>, AnalyticsError> {
    let mut stmt = store.__connection().prepare(sql::NODE_ATTRS_BY_KIND)?;
    let rows = stmt
        .query_map(params![kind], |row| {
            Ok(NodeAttrs {
                id: row.get(0)?,
                label: row.get(1)?,
                attrs: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use tumult_lake::AnalyticsStore;

    use super::*;
    use crate::sample_journal;

    const DEMO_TOPOLOGY: &str = r#"
        [[service]]
        name = "gateway"
        depends_on = ["api"]
        tier = "edge"

        [[service]]
        name = "api"
        depends_on = ["db"]
        owner = "team-core"

        [[service]]
        name = "db"
        tier = "data"
    "#;

    fn import(store: &AnalyticsStore) {
        let doc = tumult_graph::parse_topology(DEMO_TOPOLOGY).unwrap();
        store
            .refresh_topology(&tumult_graph::topology_delta(&doc))
            .unwrap();
    }

    #[test]
    fn import_is_idempotent_and_traversable() {
        let s = AnalyticsStore::in_memory().unwrap();
        import(&s);
        import(&s);

        let services = crate::graph::graph_query(&s, "service", None).unwrap();
        assert_eq!(services.len(), 3);

        let edges = graph_edges_by_rels(&s, &["depends_on"]).unwrap();
        assert_eq!(edges.len(), 2);

        let ego = crate::graph::graph_neighbors(&s, "svc:api", Some("depends_on"), 1)
            .unwrap()
            .expect("declared service exists");
        assert!(ego.edges.iter().any(|e| e.dst == "svc:db"));
    }

    #[test]
    fn run_ingest_does_not_clobber_topology_attrs() {
        use tumult_core::types::*;

        let s = AnalyticsStore::in_memory().unwrap();
        import(&s);

        // A run that targets the declared "api" service with empty attrs.
        let exp = Experiment {
            title: "api latency".into(),
            method: vec![Activity {
                name: "inject-latency".into(),
                activity_type: ActivityType::Action,
                provider: Provider::Native {
                    plugin: "tumult-net".into(),
                    function: "inject_latency".into(),
                    arguments: std::collections::HashMap::from([(
                        "upstream".into(),
                        serde_json::Value::String("api:8080".into()),
                    )]),
                },
                ..Default::default()
            }],
            ..Default::default()
        };
        s.ingest_journal_with_experiment(
            &sample_journal("run1", ExperimentStatus::Completed),
            Some(&exp),
        )
        .unwrap();

        let nodes = graph_nodes_with_attrs(&s, "service").unwrap();
        let api = nodes.iter().find(|n| n.id == "svc:api").unwrap();
        let attrs: serde_json::Value = serde_json::from_str(&api.attrs).unwrap();
        assert_eq!(
            attrs["owner"], "team-core",
            "topology attrs must survive run ingest"
        );
        assert_eq!(attrs["declared"], true);
    }

    #[test]
    fn empty_rels_returns_empty() {
        let s = AnalyticsStore::in_memory().unwrap();
        assert!(graph_edges_by_rels(&s, &[]).unwrap().is_empty());
    }
}
