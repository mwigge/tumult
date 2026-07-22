//! `tumult_chaosgraph_cypher` — arbitrary read-only openCypher over the
//! whole `ChaosGraph`.
//!
//! Architecture: no second store. The graph is snapshotted out of `DuckDB`
//! (the only source of truth) and rebuilt inside an in-memory `GrafeoDB`
//! engine per call — at `ChaosGraph` volumes that is milliseconds, and it
//! makes the Cypher engine fully disposable. Mutating clauses are rejected
//! before execution; results are row-capped.

#![allow(clippy::missing_errors_doc)]

use tumult_cypher::{GraphSnapshot, SnapshotEdge, SnapshotNode};

use crate::error::ToolError;
use crate::tools::StructuredReport;

/// Node kinds mirrored into the snapshot (grafeo node label = kind).
const NODE_KINDS: &[&str] = &[
    "experiment",
    "fault",
    "service",
    "journal",
    "deviation",
    "compliance_article",
    "coverage_gap",
    "fault_domain",
];

/// Edge relations mirrored into the snapshot.
const EDGE_RELS: &[&str] = &[
    "targets",
    "injects",
    "yielded",
    "observed_on",
    "exhibited",
    "evidences",
    "maps_to_compliance",
    "gap_in",
    "depends_on",
    "caused_by",
];

fn open_store_ro(store_path: &str) -> Result<tumult_analytics::AnalyticsStore, ToolError> {
    let path = std::path::Path::new(store_path);
    if !path.exists() {
        return Err(ToolError::NotFound(format!(
            "analytics store not found at {store_path}"
        )));
    }
    tumult_analytics::AnalyticsStore::open_read_only(path)
        .map_err(|e| ToolError::Store(e.to_string()))
}

/// Snapshot the entire `ChaosGraph` from the analytics store.
fn snapshot(store: &tumult_analytics::AnalyticsStore) -> Result<GraphSnapshot, ToolError> {
    let mut nodes = Vec::new();
    for kind in NODE_KINDS {
        for node in store
            .graph_nodes_with_attrs(kind)
            .map_err(|e| ToolError::Store(e.to_string()))?
        {
            nodes.push(SnapshotNode {
                id: node.id,
                kind: (*kind).to_string(),
                label: node.label,
                attrs: serde_json::from_str(&node.attrs)
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new())),
            });
        }
    }
    let edges = store
        .graph_edges_by_rels(EDGE_RELS)
        .map_err(|e| ToolError::Store(e.to_string()))?
        .into_iter()
        .map(|e| SnapshotEdge {
            src: e.src,
            rel: e.rel,
            dst: e.dst,
            run_id: e.run_id,
            ts: e.ts,
            attrs: serde_json::from_str(&e.attrs)
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new())),
        })
        .collect();
    Ok(GraphSnapshot { nodes, edges })
}

/// Execute a read-only openCypher query against a fresh graph snapshot.
pub fn chaosgraph_cypher(
    store_path: &str,
    query: &str,
    row_cap: Option<u32>,
) -> Result<StructuredReport, ToolError> {
    let store = open_store_ro(store_path)?;
    let snap = snapshot(&store)?;
    let cap = row_cap.map_or(tumult_cypher::DEFAULT_ROW_CAP, |c| c.max(1) as usize);

    let table = tumult_cypher::run_cypher_capped(&snap, query, cap)
        .map_err(|e| ToolError::InvalidInput(e.to_string()))?;

    let mut text = format!(
        "{} row(s){}  columns: {}\n",
        table.rows.len(),
        if table.truncated { " (truncated)" } else { "" },
        table.columns.join(", ")
    );
    for row in &table.rows {
        let rendered: Vec<String> = row
            .iter()
            .map(|v| match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect();
        text.push_str(&rendered.join(" | "));
        text.push('\n');
    }

    Ok(StructuredReport {
        text: crate::tools::cap_text(text, "add LIMIT or a WHERE filter"),
        structured: {
            let mut map = serde_json::Map::new();
            map.insert("columns".into(), serde_json::json!(table.columns));
            map.insert("rows".into(), serde_json::json!(table.rows));
            map.insert("truncated".into(), serde_json::json!(table.truncated));
            map.insert(
                "graph".into(),
                serde_json::json!({ "nodes": snap.nodes.len(), "edges": snap.edges.len() }),
            );
            map
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded_store(dir: &std::path::Path) -> String {
        let store_path = dir.join("analytics.duckdb");
        let store = tumult_analytics::AnalyticsStore::open(&store_path).unwrap();
        let doc = tumult_graph::parse_topology(
            "[[service]]\nname = \"api\"\ndepends_on = [\"db\"]\n\n[[service]]\nname = \"db\"\n",
        )
        .unwrap();
        store
            .refresh_topology(&tumult_graph::topology_delta(&doc))
            .unwrap();
        drop(store);
        store_path.to_string_lossy().into_owned()
    }

    #[test]
    fn cypher_query_over_declared_topology() {
        let tmp = tempfile::tempdir().unwrap();
        let store_path = seeded_store(tmp.path());
        let report = chaosgraph_cypher(
            &store_path,
            "MATCH (a:service)-[:depends_on]->(b:service) RETURN a.id, b.id",
            None,
        )
        .unwrap();
        assert!(report.text.contains("svc:api"));
        assert!(report.text.contains("svc:db"));
        assert_eq!(report.structured["truncated"], false);
    }

    #[test]
    fn mutation_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let store_path = seeded_store(tmp.path());
        let err = chaosgraph_cypher(&store_path, "CREATE (n:service {id: 'x'})", None)
            .expect_err("mutating cypher must be rejected");
        assert!(
            err.to_string().to_lowercase().contains("mutat"),
            "got: {err}"
        );
    }

    #[test]
    fn missing_store_is_not_found() {
        let err = chaosgraph_cypher("/nonexistent/store.duckdb", "MATCH (n) RETURN n", None)
            .expect_err("missing store");
        assert!(matches!(err, ToolError::NotFound(_)));
    }
}
