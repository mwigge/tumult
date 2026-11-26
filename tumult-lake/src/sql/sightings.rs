//! The statements of the service version sightings (`sightings.rs`).

pub(crate) const AHEAD_STORED: &str = "SELECT service_name, target_environment, service_version,
           count(*) AS spans, min(ts_ns) AS first_ns
    FROM spans
    WHERE ts_ns > ? AND ts_ns <= ?
      AND service_version IS NOT NULL AND service_version <> ''
    GROUP BY service_name, target_environment, service_version
    ORDER BY spans DESC, first_ns
    LIMIT 1";

pub(crate) const UPDATE: &str = "UPDATE service_version_sightings
    SET first_seen_ns = ?, last_seen_ns = ?, last_deployed_ns = ?,
        deployments = ?, sanitized = ?, first_anchor_ns = ?
    WHERE service_name = ? AND service_version = ?
      AND environment IS NOT DISTINCT FROM ?";

pub(crate) const INSERT: &str = "INSERT INTO service_version_sightings
       (service_name, environment, service_version, first_seen_ns, last_seen_ns,
        last_deployed_ns, deployments, sanitized, first_anchor_ns)
    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)";

pub(crate) const BACKFILL_STATE: &str = "SELECT done, cursor_ns FROM sightings_backfill LIMIT 1";

pub(crate) const BACKFILL_RESTART: &str = "DELETE FROM service_version_sightings;
    DELETE FROM sightings_backfill;
    INSERT INTO sightings_backfill (done, cursor_ns) VALUES (false, NULL);";

pub(crate) const NEXT_VERSIONED_SPAN: &str = "SELECT min(ts_ns) FROM spans
    WHERE ts_ns > ? AND service_version IS NOT NULL AND service_version <> ''";

pub(crate) const BACKFILL_DONE: &str = "UPDATE sightings_backfill SET done = true";

pub(crate) const BACKFILL_SLICE: &str = "SELECT service_name, target_environment, service_version,
           min(ts_ns) AS first_ns, max(ts_ns) AS last_ns
    FROM spans
    WHERE ts_ns > ? AND ts_ns <= ?
      AND service_version IS NOT NULL AND service_version <> ''
    GROUP BY service_name, target_environment, service_version, ts_ns // ?
    ORDER BY first_ns";

pub(crate) const SET_BACKFILL_CURSOR: &str = "UPDATE sightings_backfill SET cursor_ns = ?";

pub(crate) const OBSERVED_VERSIONS: &str = "SELECT service_version, count(*) FROM spans
    WHERE service_name = ? AND ts_ns BETWEEN ? AND ?
      AND target_environment IS NOT DISTINCT FROM ?
      AND service_version IS NOT NULL AND service_version <> ''
      AND (upper(span_kind) IN ('SERVER', 'CONSUMER')
           OR parent_span_id IS NULL OR parent_span_id = '')
      AND (? IS NULL OR resource_attrs['service.namespace'] = ?)
    GROUP BY service_version";

/// The statements of this module, for the SQL corpus.
#[cfg(test)]
pub(super) fn sql_corpus() -> Vec<tumult_test_utils::sql_corpus::Statement> {
    tumult_test_utils::sql_statements![
        AHEAD_STORED,
        UPDATE,
        INSERT,
        BACKFILL_STATE,
        BACKFILL_RESTART,
        NEXT_VERSIONED_SPAN,
        BACKFILL_DONE,
        BACKFILL_SLICE,
        SET_BACKFILL_CURSOR,
        OBSERVED_VERSIONS
    ]
}
