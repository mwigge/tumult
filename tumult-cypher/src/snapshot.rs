//! Snapshot types: the caller-supplied, engine-agnostic view of `ChaosGraph`.
//!
//! These mirror the `ChaosGraph` row shape (nodes with kind/label/attrs, edges
//! with `rel/run_id/ts/attrs`) rather than any grafeo type, so the query engine
//! stays a private implementation detail and callers never depend on it.

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// A self-contained slice of `ChaosGraph` to query.
///
/// The caller (typically tumult-analytics or an MCP tool handler) selects
/// these rows out of `DuckDB`; this crate never talks to `DuckDB` itself.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GraphSnapshot {
    /// Nodes, keyed by their `ChaosGraph` string id (must be unique).
    pub nodes: Vec<SnapshotNode>,
    /// Edges referencing nodes by string id. An edge whose endpoint is
    /// missing from `nodes` fails the build — silently dropping edges would
    /// make traversal results quietly wrong.
    pub edges: Vec<SnapshotEdge>,
}

/// One `ChaosGraph` node.
///
/// Grafeo mapping: `kind` becomes the node label (so Cypher patterns read
/// `(s:service)`), and properties are `{id, label}` plus the flattened
/// top-level `attrs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotNode {
    /// `ChaosGraph` node id, e.g. `"svc:checkout"`. Unique within a snapshot.
    pub id: String,
    /// `ChaosGraph` kind: one of `experiment`, `fault`, `service`, `journal`,
    /// `deviation`, `compliance_article`, `coverage_gap`, `fault_domain`.
    /// Not validated here — unknown kinds simply become unknown labels.
    pub kind: String,
    /// Human-readable display label (distinct from the grafeo *node label*,
    /// which is `kind`).
    pub label: String,
    /// Free-form attributes. Top-level scalar entries become individual node
    /// properties; nested objects/arrays are stringified as JSON text.
    pub attrs: JsonValue,
}

/// One `ChaosGraph` edge.
///
/// Grafeo mapping: `rel` becomes the relationship type (so patterns read
/// `-[:depends_on]->`), and properties are `{run_id, ts}` plus flattened
/// `attrs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotEdge {
    /// Source node id (must exist in [`GraphSnapshot::nodes`]).
    pub src: String,
    /// Relationship token: one of `targets`, `injects`, `yielded`,
    /// `observed_on`, `exhibited`, `evidences`, `maps_to_compliance`,
    /// `gap_in`, `depends_on`, `caused_by`. Not validated here.
    pub rel: String,
    /// Destination node id (must exist in [`GraphSnapshot::nodes`]).
    pub dst: String,
    /// Run that produced this edge; exposed as relationship property
    /// `run_id`.
    pub run_id: String,
    /// Timestamp (epoch millis in `ChaosGraph`); exposed as relationship
    /// property `ts`.
    pub ts: i64,
    /// Free-form attributes, flattened the same way as node attrs.
    pub attrs: JsonValue,
}
