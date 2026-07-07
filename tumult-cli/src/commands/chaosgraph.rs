//! `chaosgraph` subcommand: human-facing access to the `ChaosGraph` knowledge
//! graph that backs the MCP `chaosgraph_*` tools.
//!
//! `ChaosGraph` collapses each accumulated chaos run into a handful of typed
//! `(src)-[rel]->(dst)` tuples over the persistent analytics store. Until now it
//! was reachable only through the MCP server; these commands expose the exact
//! same query functions to an operator at a terminal. They read the analytics
//! store at `~/.tumult/analytics.duckdb` (override with `--store`) and render a
//! readable text summary, or the underlying structured object with
//! `--format json`.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use tumult_mcp::tools::StructuredReport;

/// Resolve the analytics store path, defaulting to the persistent store.
/// Shared with the `topology` commands.
pub(crate) fn resolve_store(store: Option<&Path>) -> PathBuf {
    store.map_or_else(
        tumult_analytics::AnalyticsStore::default_path,
        Path::to_path_buf,
    )
}

/// Render a tool report either as its readable text or as structured JSON.
/// Shared with the `topology` commands.
pub(crate) fn emit(report: &StructuredReport, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(&report.structured)?);
    } else {
        print!("{}", report.text);
    }
    Ok(())
}

/// `chaosgraph query`: list graph nodes of `kind` (e.g. `experiment`, `fault`,
/// `service`, `journal`), optionally narrowed by a label `filter` substring.
///
/// # Errors
///
/// Returns an error if the store is missing, cannot be opened, or the query
/// fails.
pub fn cmd_chaosgraph_query(
    store: Option<&Path>,
    kind: &str,
    filter: Option<&str>,
    json: bool,
) -> Result<()> {
    let path = resolve_store(store);
    let report = tumult_mcp::tools::chaosgraph_query(&path.to_string_lossy(), kind, filter)
        .map_err(|e| anyhow!(e.to_string()))?;
    emit(&report, json)
}

/// `chaosgraph neighbors`: the ego sub-graph around `node` — every node within
/// `depth` hops plus the connecting edges, optionally filtered to a single
/// relation with `--rel`.
///
/// # Errors
///
/// Returns an error if the store is missing, cannot be opened, the node id is
/// unknown, or the query fails.
pub fn cmd_chaosgraph_neighbors(
    store: Option<&Path>,
    node: &str,
    rel: Option<&str>,
    depth: u32,
    json: bool,
) -> Result<()> {
    let path = resolve_store(store);
    let report = tumult_mcp::tools::chaosgraph_neighbors(&path.to_string_lossy(), node, rel, depth)
        .map_err(|e| anyhow!(e.to_string()))?;
    emit(&report, json)
}

/// `chaosgraph coverage-gaps`: plugin-catalog actions never exercised by a
/// tested run, optionally filtered by fault `--domain` (plugin) and annotated
/// with a `--framework`'s still-unevidenced articles.
///
/// # Errors
///
/// Returns an error if the store is missing, cannot be opened, an unknown
/// framework is given, or the derivation fails.
pub fn cmd_chaosgraph_coverage_gaps(
    store: Option<&Path>,
    framework: Option<&str>,
    domain: Option<&str>,
    refresh: bool,
    json: bool,
) -> Result<()> {
    let path = resolve_store(store);
    let report = tumult_mcp::tools::chaosgraph_coverage_gaps(
        &path.to_string_lossy(),
        framework,
        domain,
        refresh,
    )
    .map_err(|e| anyhow!(e.to_string()))?;
    emit(&report, json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tumult_core::types::{
        Activity, ActivityType, Experiment, ExperimentStatus, Journal, Provider,
    };

    fn seed_store(dir: &Path) -> PathBuf {
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
    fn query_and_neighbors_succeed_against_seeded_store() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = seed_store(dir.path());

        // Text and JSON renderings both succeed.
        cmd_chaosgraph_query(Some(&db), "experiment", None, false).unwrap();
        cmd_chaosgraph_query(Some(&db), "fault", None, true).unwrap();
        cmd_chaosgraph_neighbors(Some(&db), "exp:Latency drill", None, 1, false).unwrap();
        cmd_chaosgraph_coverage_gaps(Some(&db), None, None, false, false).unwrap();
    }

    #[test]
    fn missing_store_is_a_clean_error() {
        let err = cmd_chaosgraph_query(
            Some(Path::new("/nonexistent/tumult-test.duckdb")),
            "experiment",
            None,
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("store not found"));
    }

    #[test]
    fn unknown_node_is_a_clean_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = seed_store(dir.path());
        let err =
            cmd_chaosgraph_neighbors(Some(&db), "exp:does-not-exist", None, 1, false).unwrap_err();
        assert!(err.to_string().contains("node not found"));
    }
}
