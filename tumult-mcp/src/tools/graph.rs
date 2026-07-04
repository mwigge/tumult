//! `ChaosGraph` query tools: node lookup by kind and ego-neighbourhood
//! expansion over the persistent analytics store's graph tables.

use std::fmt::Write as _;
use std::path::Path;

use crate::error::ToolError;
use crate::tools::StructuredReport;

/// Open the analytics store at `store_path`, erroring cleanly if absent.
fn open_store(store_path: &str) -> Result<tumult_analytics::AnalyticsStore, ToolError> {
    let path = Path::new(store_path);
    if !path.exists() {
        return Err(ToolError::NotFound(format!(
            "store not found: {store_path}"
        )));
    }
    tumult_analytics::AnalyticsStore::open(path).map_err(|e| ToolError::Store(e.to_string()))
}

/// `chaosgraph_query`: matching node ids + one-line summaries for a kind.
///
/// The structured object is `{kind, count, nodes:[{id,kind,label}]}`.
///
/// # Errors
///
/// Returns a [`ToolError`] if the store does not exist, cannot be opened, or
/// the query fails.
pub fn chaosgraph_query(
    store_path: &str,
    kind: &str,
    filter: Option<&str>,
) -> Result<StructuredReport, ToolError> {
    let store = open_store(store_path)?;
    let kind = kind.trim().to_ascii_lowercase();
    let nodes = store
        .graph_query(&kind, filter)
        .map_err(|e| ToolError::Store(e.to_string()))?;

    let node_values: Vec<serde_json::Value> = nodes
        .iter()
        .map(|n| serde_json::json!({ "id": n.id, "kind": n.kind, "label": n.label }))
        .collect();

    let mut structured = serde_json::Map::new();
    structured.insert("kind".into(), serde_json::json!(kind));
    structured.insert("count".into(), serde_json::json!(nodes.len()));
    structured.insert("nodes".into(), serde_json::Value::Array(node_values));

    let mut text = format!("kind: {kind}  ({} node(s))\n", nodes.len());
    for n in &nodes {
        let _ = writeln!(text, "  {}  {}", n.id, n.label);
    }

    Ok(StructuredReport {
        text: crate::tools::cap_text(text, "narrow with filter"),
        structured,
    })
}

/// `chaosgraph_neighbors`: the ego sub-graph of a node as compact
/// `(src)-[rel]->(dst)` tuples plus the labels of every node involved.
///
/// The structured object is
/// `{node_id, depth, nodes:[{id,kind,label}], edges:[{src,rel,dst}]}`.
///
/// # Errors
///
/// Returns a [`ToolError`] if the store does not exist, cannot be opened, the
/// query fails, or the node id is unknown.
pub fn chaosgraph_neighbors(
    store_path: &str,
    node_id: &str,
    rel: Option<&str>,
    depth: u32,
) -> Result<StructuredReport, ToolError> {
    let store = open_store(store_path)?;
    let depth = i64::from(depth.max(1));
    let ego = store
        .graph_neighbors(node_id, rel, depth)
        .map_err(|e| ToolError::Store(e.to_string()))?
        .ok_or_else(|| ToolError::NotFound(format!("node not found: {node_id}")))?;

    let node_values: Vec<serde_json::Value> = ego
        .nodes
        .iter()
        .map(|n| serde_json::json!({ "id": n.id, "kind": n.kind, "label": n.label }))
        .collect();
    let edge_values: Vec<serde_json::Value> = ego
        .edges
        .iter()
        .map(|e| serde_json::json!({ "src": e.src, "rel": e.rel, "dst": e.dst }))
        .collect();

    let mut structured = serde_json::Map::new();
    structured.insert("node_id".into(), serde_json::json!(ego.center));
    structured.insert("depth".into(), serde_json::json!(depth));
    structured.insert("nodes".into(), serde_json::Value::Array(node_values));
    structured.insert("edges".into(), serde_json::Value::Array(edge_values));

    let mut text = format!(
        "center: {}  (depth {depth}, {} node(s), {} edge(s))\nnodes:\n",
        ego.center,
        ego.nodes.len(),
        ego.edges.len()
    );
    for n in &ego.nodes {
        let _ = writeln!(text, "  {} ({}) {}", n.id, n.kind, n.label);
    }
    text.push_str("edges:\n");
    for e in &ego.edges {
        let _ = writeln!(text, "  ({})-[{}]->({})", e.src, e.rel, e.dst);
    }

    Ok(StructuredReport {
        text: crate::tools::cap_text(text, "reduce depth or add rel filter"),
        structured,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tumult_core::types::*;

    fn seed_store(dir: &std::path::Path) -> std::path::PathBuf {
        let db = dir.join("analytics.duckdb");
        let store = tumult_analytics::AnalyticsStore::open(&db).unwrap();
        let exp = Experiment {
            title: "Latency drill".into(),
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
        };
        let journal = Journal {
            experiment_title: "Latency drill".into(),
            experiment_id: "run-1".into(),
            status: ExperimentStatus::Completed,
            started_at_ns: 1,
            ended_at_ns: 2,
            duration_ms: 1,
            steady_state_before: None,
            steady_state_after: None,
            method_results: vec![],
            rollback_results: vec![],
            rollback_failures: 0,
            estimate: None,
            baseline_result: None,
            during_result: None,
            post_result: None,
            load_result: None,
            analysis: None,
            regulatory: None,
            halt: None,
            blast_radius: None,
        };
        store
            .ingest_journal_with_experiment(&journal, Some(&exp))
            .unwrap();
        db
    }

    #[test]
    fn query_returns_matching_nodes() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = seed_store(dir.path());
        let report = chaosgraph_query(db.to_str().unwrap(), "experiment", None).unwrap();
        assert_eq!(report.structured["count"], 1);
        assert_eq!(report.structured["nodes"][0]["id"], "exp:Latency drill");
    }

    #[test]
    fn neighbors_returns_ego_graph() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = seed_store(dir.path());
        let report =
            chaosgraph_neighbors(db.to_str().unwrap(), "exp:Latency drill", None, 1).unwrap();
        assert_eq!(report.structured["node_id"], "exp:Latency drill");
        let edges = report.structured["edges"].as_array().unwrap();
        assert!(!edges.is_empty());
        assert!(report.text.contains("injects"));
    }

    #[test]
    fn neighbors_unknown_node_errors() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = seed_store(dir.path());
        let err = chaosgraph_neighbors(db.to_str().unwrap(), "exp:nope", None, 1).unwrap_err();
        assert!(err.to_string().contains("node not found"));
    }

    #[test]
    fn missing_store_errors() {
        let err = chaosgraph_query("/nonexistent/x.duckdb", "experiment", None).unwrap_err();
        assert!(err.to_string().contains("store not found"));
    }
}
