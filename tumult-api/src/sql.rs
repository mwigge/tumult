//! The SQL statements of the API that hold no value chosen at run time,
//! as named consts. A handler takes its statement from here, so that the
//! SQL corpus test can collect it. A statement built from a request is a
//! builder function next to its handler instead.

/// The registered gameday definitions, newest first, without their source.
pub(crate) const GAMEDAYS: &str = "SELECT id, name, content_hash, registered_at_ns, registered_by \
    FROM run_registry WHERE kind = 'gameday' \
    ORDER BY registered_at_ns DESC LIMIT 500";

/// The sum and gauge points of the experiment `$1` from `$2` to `$3`.
pub(crate) const METRIC_POINTS: &str =
    "SELECT 'sum' AS kind, ts_ns, metric_name, value, outcome_status, plugin_name \
    FROM metric_sums \
    WHERE experiment_name = $1 AND ts_ns BETWEEN $2 AND $3 \
    UNION ALL \
    SELECT 'gauge', ts_ns, metric_name, value, outcome_status, plugin_name \
    FROM metric_gauges \
    WHERE experiment_name = $1 AND ts_ns BETWEEN $2 AND $3 \
    ORDER BY 2 LIMIT 1000";

/// The experiment definitions, but for gamedays, with their number of runs.
pub(crate) const DEFINITIONS: &str = "SELECT rg.id, rg.name, \
    (SELECT COUNT(*) FROM runs r WHERE r.registry_id = rg.id) AS runs \
    FROM run_registry rg WHERE rg.kind IS DISTINCT FROM 'gameday' ORDER BY rg.name";

/// The edges from experiments to the compliance articles they evidence or map to.
pub(crate) const EVIDENCE_EDGES: &str = "SELECT DISTINCT src, rel, dst FROM graph_edges \
    WHERE rel IN ('evidences', 'maps_to_compliance')";

/// The declared dependencies between services.
pub(crate) const DEPENDS_ON_EDGES: &str =
    "SELECT DISTINCT src, dst FROM graph_edges WHERE rel = 'depends_on'";

/// The statements of this module, for the SQL corpus.
#[cfg(test)]
pub(crate) fn sql_corpus() -> Vec<tumult_test_utils::sql_corpus::Statement> {
    tumult_test_utils::sql_statements![
        GAMEDAYS,
        METRIC_POINTS,
        DEFINITIONS,
        EVIDENCE_EDGES,
        DEPENDS_ON_EDGES,
    ]
}
