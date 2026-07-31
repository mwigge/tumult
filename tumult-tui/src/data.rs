//! Read-only data access over [`tumult_lake::AnalyticsStore`].
//!
//! Every function here opens (or reuses) a **read-only** store handle so the
//! TUI coexists with a running MCP server or `tumult run` ingest — it never
//! takes the exclusive write lock. Queries use only real columns from the
//! `experiments` / `activity_results` tables and the `ChaosGraph`
//! `graph_query` API.

use std::path::Path;

use tumult_lake::AnalyticsStore;

use crate::error::TuiError;
use crate::model::{ActivityRow, ExperimentRow, GraphNodeRow};

/// The `ChaosGraph` node kinds the browser cycles through.
pub const GRAPH_KINDS: [&str; 6] = [
    "experiment",
    "fault",
    "service",
    "journal",
    "coverage_gap",
    "compliance_article",
];

/// A read-only snapshot of the store, re-taken on every refresh so newly
/// ingested experiments appear without holding the file open across ticks.
pub struct Snapshot {
    pub experiments: Vec<ExperimentRow>,
    pub experiment_count: usize,
    pub activity_count: usize,
    pub schema_version: i64,
}

/// Open the store read-only and load the full history snapshot.
///
/// # Errors
///
/// Returns an error if the store cannot be opened read-only (e.g. it does not
/// exist yet, or a writer holds the exclusive lock) or a query fails.
pub fn load_snapshot(path: &Path) -> Result<Snapshot, TuiError> {
    let store = AnalyticsStore::open_read_only(path).map_err(|source| TuiError::OpenReadOnly {
        path: path.to_path_buf(),
        source,
    })?;
    let experiments = load_experiments(&store)?;
    let stats = store.stats().map_err(TuiError::StoreStats)?;
    let schema_version = store.schema_version().unwrap_or(0);
    Ok(Snapshot {
        experiments,
        experiment_count: stats.experiment_count,
        activity_count: stats.activity_count,
        schema_version,
    })
}

/// Load every experiment (most-recent first) joined with its non-succeeded
/// activity count as the `deviations` column.
fn load_experiments(store: &AnalyticsStore) -> Result<Vec<ExperimentRow>, TuiError> {
    let sql = "SELECT e.experiment_id, e.title, e.status, e.started_at_ns, e.duration_ms, \
                      e.resilience_score, e.method_step_count, \
                      COALESCE(d.dev, 0) AS deviations \
               FROM experiments e \
               LEFT JOIN ( \
                   SELECT experiment_id, count(*) AS dev \
                   FROM activity_results \
                   WHERE status <> 'succeeded' \
                   GROUP BY experiment_id \
               ) d ON d.experiment_id = e.experiment_id \
               ORDER BY e.started_at_ns DESC";
    let rows = store.query(sql).map_err(TuiError::ExperimentsHistory)?;
    Ok(rows
        .iter()
        .filter_map(|r| ExperimentRow::from_columns(r))
        .collect())
}

/// Load the activity timeline for a single experiment, ordered by start time.
///
/// # Errors
///
/// Returns an error if the store cannot be opened read-only or the query fails.
pub fn load_activities(path: &Path, experiment_id: &str) -> Result<Vec<ActivityRow>, TuiError> {
    let store = AnalyticsStore::open_read_only(path)?;
    // Bind the id as a parameter to avoid any SQL-injection surface.
    let sql = "SELECT name, activity_type, status, duration_ms, phase, output \
               FROM activity_results WHERE experiment_id = ? ORDER BY started_at_ns";
    let rows = store
        .query_with_param(sql, experiment_id)
        .map_err(TuiError::ActivityTimeline)?;
    Ok(rows
        .iter()
        .filter_map(|r| ActivityRow::from_columns(r))
        .collect())
}

/// Load `ChaosGraph` nodes of a given kind (optionally filtered by a label
/// substring) via `tumult_query`'s `graph_query`.
///
/// # Errors
///
/// Returns an error if the store cannot be opened read-only or the query fails.
pub fn load_graph_nodes(
    path: &Path,
    kind: &str,
    filter: Option<&str>,
) -> Result<Vec<GraphNodeRow>, TuiError> {
    let store = AnalyticsStore::open_read_only(path)?;
    let nodes =
        tumult_query::graph_query(&store, kind, filter).map_err(|source| TuiError::GraphNodes {
            kind: kind.to_string(),
            source,
        })?;
    Ok(nodes
        .into_iter()
        .map(|n| GraphNodeRow {
            id: n.id,
            kind: n.kind,
            label: n.label,
        })
        .collect())
}

/// The neighbour node ids/labels of a graph node (depth 1), for the detail pane.
///
/// # Errors
///
/// Returns an error if the store cannot be opened read-only or the query fails.
pub fn load_graph_neighbors(path: &Path, node_id: &str) -> Result<Vec<String>, TuiError> {
    let store = AnalyticsStore::open_read_only(path)?;
    let ego = tumult_query::graph_neighbors(&store, node_id, None, 1)
        .map_err(TuiError::GraphNeighbours)?;
    Ok(ego
        .map(|g| {
            g.edges
                .into_iter()
                .map(|e| format!("{} —{}→ {}", e.src, e.rel, e.dst))
                .collect()
        })
        .unwrap_or_default())
}
