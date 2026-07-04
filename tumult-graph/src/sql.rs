//! The SQL that persists and queries the graph. `tumult-graph` owns these
//! statements; `tumult-analytics` binds their parameters and executes them
//! against its embedded `DuckDB` connection. Keeping the SQL here (rather than
//! a live connection) is what keeps the dependency direction acyclic.
//!
//! Parameter order is documented on each item and must be honoured by the
//! executor.

/// DDL for the two graph tables. Idempotent (`IF NOT EXISTS`), so it is safe to
/// run on every store open and doubles as the additive v1 → v2 migration.
///
/// The `graph_edges.attrs` column is part of the fresh-install schema (v3).
/// Stores created at v2 gain it via [`MIGRATE_EDGES_ADD_ATTRS`].
pub const CREATE_TABLES: &str = "\
CREATE TABLE IF NOT EXISTS graph_nodes (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    label TEXT NOT NULL,
    attrs JSON
);
CREATE TABLE IF NOT EXISTS graph_edges (
    src TEXT NOT NULL,
    rel TEXT NOT NULL,
    dst TEXT NOT NULL,
    run_id TEXT NOT NULL,
    ts BIGINT NOT NULL,
    attrs JSON
);
CREATE INDEX IF NOT EXISTS idx_graph_edges_src ON graph_edges (src);
CREATE INDEX IF NOT EXISTS idx_graph_edges_dst ON graph_edges (dst);
CREATE INDEX IF NOT EXISTS idx_graph_edges_run ON graph_edges (run_id);
CREATE INDEX IF NOT EXISTS idx_graph_nodes_kind ON graph_nodes (kind);";

/// Additive v2 → v3 migration: add the `attrs` column to an existing
/// `graph_edges` table. Idempotent via `IF NOT EXISTS`, so it is safe to run on
/// every store open (a no-op once applied or on a fresh v3 table).
pub const MIGRATE_EDGES_ADD_ATTRS: &str =
    "ALTER TABLE graph_edges ADD COLUMN IF NOT EXISTS attrs JSON";

/// Upsert a node. Parameters, in order: `id`, `kind`, `label`, `attrs` (JSON
/// text). On id conflict the label/kind/attrs are refreshed.
pub const UPSERT_NODE: &str = "\
INSERT INTO graph_nodes (id, kind, label, attrs)
VALUES (?, ?, ?, CAST(? AS JSON))
ON CONFLICT (id) DO UPDATE SET
    kind = excluded.kind,
    label = excluded.label,
    attrs = excluded.attrs";

/// Delete every edge previously recorded for a run. Parameter: `run_id`.
///
/// Run before re-inserting a run's edges so re-ingesting the same run never
/// duplicates edges. Static sub-graphs (coverage gaps) use a sentinel `run_id`
/// so the same clear-then-insert keeps them idempotent.
pub const DELETE_EDGES_FOR_RUN: &str = "DELETE FROM graph_edges WHERE run_id = ?";

/// Delete every node of a given `kind`. Parameter: `kind`. Used to clear a
/// derived, recomputable node set (coverage gaps) before re-deriving it.
pub const DELETE_NODES_BY_KIND: &str = "DELETE FROM graph_nodes WHERE kind = ?";

/// Insert one edge. Parameters, in order: `src`, `rel`, `dst`, `run_id`, `ts`,
/// `attrs` (JSON text).
pub const INSERT_EDGE: &str =
    "INSERT INTO graph_edges (src, rel, dst, run_id, ts, attrs) VALUES (?, ?, ?, ?, ?, CAST(? AS JSON))";

/// Does a node with this id exist? Parameter: `id`. Selects `count(*)`.
pub const NODE_EXISTS: &str = "SELECT count(*) FROM graph_nodes WHERE id = ?";

/// Query node summaries by kind, optionally filtering the label with a bound
/// `ILIKE` pattern.
///
/// * `with_filter == false` — parameter: `kind`.
/// * `with_filter == true` — parameters, in order: `kind`, `label_pattern`
///   (e.g. `%latency%`).
#[must_use]
pub fn nodes_by_kind(with_filter: bool) -> String {
    let mut sql = String::from("SELECT id, kind, label FROM graph_nodes WHERE kind = ?");
    if with_filter {
        sql.push_str(" AND label ILIKE ?");
    }
    sql.push_str(" ORDER BY id");
    sql
}

/// Common recursive-CTE prefix expanding the neighbourhood of a centre node up
/// to a depth, following edges in both directions. Parameters bound by the
/// caller, in order: `center` (node id), `depth`.
const REACH_CTE: &str = "\
WITH RECURSIVE reach(node, d) AS (
    SELECT CAST(? AS VARCHAR), 0
    UNION
    SELECT CASE WHEN e.src = r.node THEN e.dst ELSE e.src END, r.d + 1
    FROM reach r
    JOIN graph_edges e ON e.src = r.node OR e.dst = r.node
    WHERE r.d < ?
)";

/// Node summaries for every node in the neighbourhood (including the centre).
/// Parameters, in order: `center`, `depth`.
#[must_use]
pub fn ego_nodes() -> String {
    format!(
        "{REACH_CTE}
SELECT n.id, n.kind, n.label
FROM graph_nodes n
WHERE n.id IN (SELECT node FROM reach)
ORDER BY n.id"
    )
}

/// The `(src, rel, dst)` tuples among the neighbourhood's nodes, optionally
/// filtered to a single relation.
///
/// * `with_rel == false` — parameters, in order: `center`, `depth`.
/// * `with_rel == true` — parameters, in order: `center`, `depth`, `rel`.
#[must_use]
pub fn ego_edges(with_rel: bool) -> String {
    let mut sql = format!(
        "{REACH_CTE}
SELECT DISTINCT e.src, e.rel, e.dst
FROM graph_edges e
WHERE e.src IN (SELECT node FROM reach) AND e.dst IN (SELECT node FROM reach)"
    );
    if with_rel {
        sql.push_str(" AND e.rel = ?");
    }
    sql.push_str("\nORDER BY e.src, e.rel, e.dst");
    sql
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nodes_by_kind_appends_filter_only_when_requested() {
        assert!(!nodes_by_kind(false).contains("ILIKE"));
        assert!(nodes_by_kind(true).contains("ILIKE"));
    }

    #[test]
    fn ego_edges_appends_rel_filter_only_when_requested() {
        assert!(!ego_edges(false).contains("e.rel = ?"));
        assert!(ego_edges(true).contains("e.rel = ?"));
        assert!(ego_edges(false).contains("RECURSIVE"));
    }

    #[test]
    fn ego_nodes_selects_from_reach() {
        assert!(ego_nodes().contains("reach"));
        assert!(ego_nodes().contains("graph_nodes"));
    }
}
