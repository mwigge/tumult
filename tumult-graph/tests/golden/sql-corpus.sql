-- The SQL corpus of tumult-graph: every statement the crate runs, as its
-- code renders it, each under the file and the item that hold it. A test
-- of the crate prepares each statement on a fresh lake and keeps this
-- file; CI lints it with sqlfluff. Change the SQL in the code, then write
-- this file again and read the difference:
--   TUMULT_BLESS=1 cargo test -p tumult-graph -- sql_corpus

-- tumult-graph/src/sql.rs:CREATE_TABLES
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
CREATE INDEX IF NOT EXISTS idx_graph_nodes_kind ON graph_nodes (kind);

-- tumult-graph/src/sql.rs:MIGRATE_EDGES_ADD_ATTRS
ALTER TABLE graph_edges ADD COLUMN IF NOT EXISTS attrs JSON;

-- tumult-graph/src/sql.rs:UPSERT_NODE
INSERT INTO graph_nodes (id, kind, label, attrs)
VALUES (?, ?, ?, CAST(? AS JSON))
ON CONFLICT (id) DO UPDATE SET
    kind = excluded.kind,
    label = excluded.label,
    attrs = excluded.attrs;

-- tumult-graph/src/sql.rs:UPSERT_NODE_MERGE_ATTRS
INSERT INTO graph_nodes (id, kind, label, attrs)
VALUES (?, ?, ?, CAST(? AS JSON))
ON CONFLICT (id) DO UPDATE SET
    kind = excluded.kind,
    label = excluded.label,
    attrs = json_merge_patch(graph_nodes.attrs, excluded.attrs);

-- tumult-graph/src/sql.rs:DELETE_EDGES_FOR_RUN
DELETE FROM graph_edges WHERE run_id = ?;

-- tumult-graph/src/sql.rs:DELETE_NODES_BY_KIND
DELETE FROM graph_nodes WHERE kind = ?;

-- tumult-graph/src/sql.rs:DELETE_NODE
DELETE FROM graph_nodes WHERE id = ?;

-- tumult-graph/src/sql.rs:INSERT_EDGE
INSERT INTO graph_edges (src, rel, dst, run_id, ts, attrs) VALUES (?, ?, ?, ?, ?, CAST(? AS JSON));

-- tumult-graph/src/sql.rs:INJECTED_FAULTS
SELECT DISTINCT dst FROM graph_edges WHERE rel = 'injects';

-- tumult-graph/src/sql.rs:NODE_EXISTS
SELECT count(*) FROM graph_nodes WHERE id = ?;

-- tumult-graph/src/sql.rs:nodes_by_kind(false)
SELECT id, kind, label FROM graph_nodes WHERE kind = ? ORDER BY id;

-- tumult-graph/src/sql.rs:nodes_by_kind(true)
SELECT id, kind, label FROM graph_nodes WHERE kind = ? AND label ILIKE ? ORDER BY id;

-- tumult-graph/src/sql.rs:edges_by_rels(1)
SELECT src, rel, dst, run_id, ts, COALESCE(CAST(attrs AS VARCHAR), '{}')
         FROM graph_edges WHERE rel IN (?) ORDER BY ts, rowid;

-- tumult-graph/src/sql.rs:edges_by_rels(3)
SELECT src, rel, dst, run_id, ts, COALESCE(CAST(attrs AS VARCHAR), '{}')
         FROM graph_edges WHERE rel IN (?, ?, ?) ORDER BY ts, rowid;

-- tumult-graph/src/sql.rs:NODE_ATTRS_BY_KIND
SELECT id, label, COALESCE(CAST(attrs AS VARCHAR), '{}') FROM graph_nodes WHERE kind = ? ORDER BY id;

-- tumult-graph/src/sql.rs:ego_nodes(false)
WITH RECURSIVE reach(node, d) AS (
    SELECT CAST(? AS VARCHAR), 0
    UNION
    SELECT CASE WHEN e.src = r.node THEN e.dst ELSE e.src END, r.d + 1
    FROM reach r
    JOIN graph_edges e ON (e.src = r.node OR e.dst = r.node)
    WHERE r.d < ?
)
SELECT n.id, n.kind, n.label
FROM graph_nodes n
WHERE n.id IN (SELECT node FROM reach)
ORDER BY n.id;

-- tumult-graph/src/sql.rs:ego_nodes(true)
WITH RECURSIVE reach(node, d) AS (
    SELECT CAST(? AS VARCHAR), 0
    UNION
    SELECT CASE WHEN e.src = r.node THEN e.dst ELSE e.src END, r.d + 1
    FROM reach r
    JOIN graph_edges e ON (e.src = r.node OR e.dst = r.node) AND e.rel = ?
    WHERE r.d < ?
)
SELECT n.id, n.kind, n.label
FROM graph_nodes n
WHERE n.id IN (SELECT node FROM reach)
ORDER BY n.id;

-- tumult-graph/src/sql.rs:ego_edges(false)
WITH RECURSIVE reach(node, d) AS (
    SELECT CAST(? AS VARCHAR), 0
    UNION
    SELECT CASE WHEN e.src = r.node THEN e.dst ELSE e.src END, r.d + 1
    FROM reach r
    JOIN graph_edges e ON (e.src = r.node OR e.dst = r.node)
    WHERE r.d < ?
)
SELECT DISTINCT e.src, e.rel, e.dst
FROM graph_edges e
WHERE e.src IN (SELECT node FROM reach) AND e.dst IN (SELECT node FROM reach)
ORDER BY e.src, e.rel, e.dst;

-- tumult-graph/src/sql.rs:ego_edges(true)
WITH RECURSIVE reach(node, d) AS (
    SELECT CAST(? AS VARCHAR), 0
    UNION
    SELECT CASE WHEN e.src = r.node THEN e.dst ELSE e.src END, r.d + 1
    FROM reach r
    JOIN graph_edges e ON (e.src = r.node OR e.dst = r.node) AND e.rel = ?
    WHERE r.d < ?
)
SELECT DISTINCT e.src, e.rel, e.dst
FROM graph_edges e
WHERE e.src IN (SELECT node FROM reach) AND e.dst IN (SELECT node FROM reach) AND e.rel = ?
ORDER BY e.src, e.rel, e.dst;
