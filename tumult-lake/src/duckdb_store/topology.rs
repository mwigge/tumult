//! Declared-topology persistence. Executor only — the model, SQL and
//! derivations live in `tumult-graph`, and the edge/node readbacks that feed
//! lineage and recommendation computation live in `tumult-query`.

use duckdb::params;
use tumult_graph::{sql, GraphDelta, TOPOLOGY_RUN_ID};

use crate::error::AnalyticsError;

use super::AnalyticsStore;

impl AnalyticsStore {
    /// Replace the declared-topology sub-graph with a freshly parsed delta.
    ///
    /// `depends_on` edges live under the sentinel [`TOPOLOGY_RUN_ID`] and are
    /// cleared before insert, so re-import is idempotent. Service nodes are
    /// upserted with full attr replacement — the topology document is the
    /// authority on `owner`/`tier`/`declared`, so import wins over whatever
    /// attrs accumulated. Run-derived service nodes for services *not* in the
    /// document are left untouched (they may carry run history).
    ///
    /// # Errors
    ///
    /// Returns an error if a delete or insert fails.
    pub fn refresh_topology(&self, delta: &GraphDelta) -> Result<(), AnalyticsError> {
        self.conn
            .execute(sql::DELETE_EDGES_FOR_RUN, params![TOPOLOGY_RUN_ID])?;
        for node in &delta.nodes {
            self.conn.execute(
                sql::UPSERT_NODE,
                params![
                    node.id,
                    node.kind.as_str(),
                    node.label,
                    node.attrs.to_string()
                ],
            )?;
        }
        for edge in &delta.edges {
            self.conn.execute(
                sql::INSERT_EDGE,
                params![
                    edge.src,
                    edge.rel.as_str(),
                    edge.dst,
                    TOPOLOGY_RUN_ID,
                    0_i64,
                    edge.attrs.to_string()
                ],
            )?;
        }
        Ok(())
    }
}
