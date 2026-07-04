//! `ChaosGraph` query tools: node lookup by kind and ego-neighbourhood
//! expansion over the persistent analytics store's graph tables.

use std::fmt::Write as _;
use std::path::Path;

use crate::error::ToolError;
use crate::tools::StructuredReport;

/// Open the analytics store read-write at `store_path`, erroring cleanly if
/// absent. Use only for paths that write (e.g. refreshing coverage gaps); it
/// takes the exclusive lock and so contends with a running server.
fn open_store(store_path: &str) -> Result<tumult_analytics::AnalyticsStore, ToolError> {
    let path = Path::new(store_path);
    if !path.exists() {
        return Err(ToolError::NotFound(format!(
            "store not found: {store_path}"
        )));
    }
    tumult_analytics::AnalyticsStore::open(path).map_err(|e| ToolError::Store(e.to_string()))
}

/// Open the analytics store read-only, erroring cleanly if absent. Read-only
/// opens do not take the exclusive lock, so a query coexists with the running
/// MCP server (and with a CLI `tumult chaosgraph` reading the same store).
fn open_store_ro(store_path: &str) -> Result<tumult_analytics::AnalyticsStore, ToolError> {
    let path = Path::new(store_path);
    if !path.exists() {
        return Err(ToolError::NotFound(format!(
            "store not found: {store_path}"
        )));
    }
    tumult_analytics::AnalyticsStore::open_read_only(path)
        .map_err(|e| ToolError::Store(e.to_string()))
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
    let store = open_store_ro(store_path)?;
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
    let store = open_store_ro(store_path)?;
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

/// `chaosgraph_coverage_gaps`: plugin-catalog actions that have never appeared
/// in a tested run, optionally filtered by fault domain (plugin) and annotated,
/// when a framework is given, with that framework's still-unevidenced articles.
///
/// Side effect: refreshes the persistent `CoverageGap` / `FaultDomain` nodes and
/// `gap_in` edges in the store's graph so `chaosgraph_query`/`_neighbors` can
/// see them. Read-only with respect to the analytics *history* (it derives from
/// existing data and only rewrites the derived coverage-gap sub-graph).
///
/// The structured object is
/// `{count, gaps:[{id,plugin,action,domain}], framework?, unevidenced_articles?}`.
///
/// # Errors
///
/// Returns a [`ToolError`] if the store does not exist, cannot be opened, the
/// derivation fails, or an unknown `framework` is given.
pub fn chaosgraph_coverage_gaps(
    store_path: &str,
    framework: Option<&str>,
    domain: Option<&str>,
) -> Result<StructuredReport, ToolError> {
    let store = open_store(store_path)?;

    // Available capabilities from the plugin catalog.
    let plugins = tumult_plugin::discovery::discover_all_plugins().unwrap_or_default();
    let available: Vec<tumult_graph::AvailableAction> = plugins
        .iter()
        .flat_map(|p| {
            p.actions
                .iter()
                .map(move |a| tumult_graph::AvailableAction::new(&p.name, &a.name))
        })
        .collect();

    // Tested actions from the store, then derive the gap sub-graph and persist
    // it so the graph tools can navigate it.
    let tested = store
        .tested_action_names()
        .map_err(|e| ToolError::Store(e.to_string()))?;
    let delta = tumult_graph::coverage_gap_delta(&available, &tested);
    store
        .refresh_coverage_gaps(&delta)
        .map_err(|e| ToolError::Store(e.to_string()))?;

    // Optional domain (plugin) filter, applied to the returned list only.
    let domain_filter = domain.map(str::to_ascii_lowercase);
    let mut gaps: Vec<serde_json::Value> = Vec::new();
    for node in &delta.nodes {
        if node.kind != tumult_graph::NodeKind::CoverageGap {
            continue;
        }
        let plugin = node.attrs["plugin"].as_str().unwrap_or_default();
        let action = node.attrs["action"].as_str().unwrap_or_default();
        if let Some(ref want) = domain_filter {
            if !plugin.to_ascii_lowercase().contains(want) {
                continue;
            }
        }
        gaps.push(serde_json::json!({
            "id": node.id,
            "plugin": plugin,
            "action": action,
            "domain": format!("domain:{plugin}"),
        }));
    }
    gaps.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));

    let mut structured = serde_json::Map::new();
    structured.insert("count".into(), serde_json::json!(gaps.len()));
    structured.insert("gaps".into(), serde_json::Value::Array(gaps.clone()));

    let mut text = format!("coverage gaps: {} untested action(s)", gaps.len());
    if let Some(d) = domain {
        let _ = write!(text, "  (domain filter: {d})");
    }
    text.push('\n');
    for gap in &gaps {
        let _ = writeln!(
            text,
            "  {}  (domain {})",
            gap["id"].as_str().unwrap_or_default(),
            gap["plugin"].as_str().unwrap_or_default()
        );
    }

    // Framework annotation: which of the framework's articles have no
    // `evidences` edge yet.
    if let Some(fw) = framework {
        let parsed = tumult_core::compliance::ComplianceFramework::parse(fw)
            .map_err(ToolError::InvalidInput)?;
        let evidenced = evidenced_article_ids(&store)?;
        let unevidenced: Vec<serde_json::Value> = tumult_core::compliance::CITATIONS
            .iter()
            .filter(|c| c.framework == parsed)
            .map(|c| {
                (
                    tumult_graph::compliance_article_id(c.framework, c.control_id),
                    c,
                )
            })
            .filter(|(id, _)| !evidenced.contains(id))
            .map(|(id, c)| {
                serde_json::json!({
                    "id": id,
                    "control_id": c.control_id,
                    "strength": c.strength.as_str(),
                })
            })
            .collect();

        let _ = writeln!(
            text,
            "\nframework {}: {} article(s) still unevidenced",
            parsed.as_report_str(),
            unevidenced.len()
        );
        for art in &unevidenced {
            let _ = writeln!(text, "  {}", art["id"].as_str().unwrap_or_default());
        }
        structured.insert(
            "framework".into(),
            serde_json::json!(parsed.as_report_str()),
        );
        structured.insert(
            "unevidenced_articles".into(),
            serde_json::Value::Array(unevidenced),
        );
    }

    Ok(StructuredReport {
        text: crate::tools::cap_text(text, "filter by domain or framework"),
        structured,
    })
}

/// The set of `ComplianceArticle` node ids that have at least one `evidences`
/// edge pointing at them in the store.
fn evidenced_article_ids(
    store: &tumult_analytics::AnalyticsStore,
) -> Result<std::collections::HashSet<String>, ToolError> {
    let rows = store
        .query("SELECT DISTINCT dst FROM graph_edges WHERE rel = 'evidences'")
        .map_err(|e| ToolError::Store(e.to_string()))?;
    Ok(rows
        .into_iter()
        .filter_map(|row| row.into_iter().next())
        .collect())
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

    /// Seed a store from a process-provider experiment (like the demo's
    /// demo-postgres) and confirm a service node + `targets` edge are produced.
    #[test]
    fn process_provider_experiment_yields_service_node_and_targets_edge() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = dir.path().join("analytics.duckdb");
        let store = tumult_analytics::AnalyticsStore::open(&db).unwrap();
        let exp = Experiment {
            title: "Demo — PostgreSQL connection kill".into(),
            method: vec![Activity {
                name: "kill-connections".into(),
                activity_type: ActivityType::Action,
                provider: Provider::Process {
                    path: "sh".into(),
                    arguments: vec![
                        "-c".into(),
                        "docker exec demo-postgres psql -U demo -c 'SELECT 1'".into(),
                    ],
                    env: std::collections::HashMap::new(),
                    timeout_s: Some(15.0),
                },
                ..Default::default()
            }],
            ..Default::default()
        };
        let journal = Journal {
            experiment_title: "Demo — PostgreSQL connection kill".into(),
            experiment_id: "demo-postgres-1".into(),
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
        drop(store);

        let services = chaosgraph_query(db.to_str().unwrap(), "service", None).unwrap();
        assert_eq!(services.structured["nodes"][0]["id"], "svc:demo-postgres");

        let ego = chaosgraph_neighbors(
            db.to_str().unwrap(),
            "exp:Demo — PostgreSQL connection kill",
            Some("targets"),
            1,
        )
        .unwrap();
        let edges = ego.structured["edges"].as_array().unwrap();
        assert!(edges
            .iter()
            .any(|e| e["dst"] == "svc:demo-postgres" && e["rel"] == "targets"));
    }

    #[test]
    fn coverage_gaps_round_trip_reports_and_persists_gaps() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = dir.path().join("analytics.duckdb");
        // Create the store so the tool can open it.
        drop(tumult_analytics::AnalyticsStore::open(&db).unwrap());

        let report = chaosgraph_coverage_gaps(db.to_str().unwrap(), None, None).unwrap();
        // Structured content conforms: count + gaps present.
        assert!(report.structured.contains_key("count"));
        assert!(report.structured["gaps"].is_array());
        // No framework filter → no framework annotation keys.
        assert!(!report.structured.contains_key("framework"));

        // With a framework filter, the unevidenced-articles list is present and
        // (with no runs) contains that framework's articles.
        let report = chaosgraph_coverage_gaps(db.to_str().unwrap(), Some("dora"), None).unwrap();
        assert_eq!(report.structured["framework"], "DORA");
        assert!(report.structured["unevidenced_articles"]
            .as_array()
            .is_some_and(|a| !a.is_empty()));

        // Unknown framework is rejected.
        let err = chaosgraph_coverage_gaps(db.to_str().unwrap(), Some("hipaa"), None).unwrap_err();
        assert!(err.to_string().contains("hipaa"));
    }
}
