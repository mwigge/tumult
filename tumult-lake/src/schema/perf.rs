//! v18 performance DDL: `perf_verdicts`, one row per regression verdict
//! computed for a target between two `service.version`s. Additive and
//! index-free apart from the per-target lookup index, under the same
//! crash-robustness rule as the v5 run tables; a recomputation appends a
//! new row (`computed_at_ns`) rather than updating in place. `envs` is the
//! environment list the evidence was read under (NULL when unscoped), so
//! history can be filtered by a principal's scopes.
//!
//! Added later without a schema version, by the same additive pattern as
//! `envs`: the version a run recorded for its target on `experiments`,
//! how a load was executed on `load_results`, and the
//! `service_version_sightings` rollup (see `crate::sightings`). The new
//! columns are appended, so they follow the Arrow layouts in
//! `arrow_convert` position for position, and the rollup carries no index
//! because its rows are updated in place.

/// `perf_verdicts`: the two versions, the probe series compared, the
/// per-side percentiles, the bootstrap interval and the verdict.
pub(super) const DDL: &str = "
CREATE TABLE IF NOT EXISTS perf_verdicts (
    target VARCHAR NOT NULL, probe VARCHAR NOT NULL,
    baseline_version VARCHAR NOT NULL, candidate_version VARCHAR NOT NULL,
    quantile VARCHAR NOT NULL,
    baseline_n BIGINT NOT NULL, candidate_n BIGINT NOT NULL,
    baseline_p50 DOUBLE, baseline_p95 DOUBLE, baseline_p99 DOUBLE,
    candidate_p50 DOUBLE, candidate_p95 DOUBLE, candidate_p99 DOUBLE,
    delta DOUBLE, delta_pct DOUBLE, ci_low DOUBLE, ci_high DOUBLE,
    confidence DOUBLE NOT NULL, margin DOUBLE NOT NULL,
    verdict VARCHAR NOT NULL, reasons JSON NOT NULL,
    computed_at_ns BIGINT NOT NULL,
    envs JSON
);
ALTER TABLE perf_verdicts ADD COLUMN IF NOT EXISTS envs JSON;
CREATE INDEX IF NOT EXISTS idx_perf_verdicts_target ON perf_verdicts (target);
ALTER TABLE experiments ADD COLUMN IF NOT EXISTS target_version VARCHAR;
ALTER TABLE experiments ADD COLUMN IF NOT EXISTS version_source VARCHAR;
ALTER TABLE load_results ADD COLUMN IF NOT EXISTS aborted BOOLEAN;
ALTER TABLE load_results ADD COLUMN IF NOT EXISTS truncated BOOLEAN;
ALTER TABLE load_results ADD COLUMN IF NOT EXISTS planned_windows INTEGER;
ALTER TABLE load_results ADD COLUMN IF NOT EXISTS completed_windows INTEGER;
ALTER TABLE load_results ADD COLUMN IF NOT EXISTS unanswered_windows INTEGER;
ALTER TABLE load_results ADD COLUMN IF NOT EXISTS lost_to_unanswered INTEGER;
ALTER TABLE load_results ADD COLUMN IF NOT EXISTS sampled_duration_s DOUBLE;
CREATE TABLE IF NOT EXISTS service_version_sightings (
    service_name VARCHAR NOT NULL, environment VARCHAR,
    service_version VARCHAR NOT NULL,
    first_seen_ns BIGINT NOT NULL, last_seen_ns BIGINT NOT NULL
);
";

#[cfg(test)]
mod tests {
    use crate::Store;

    /// `experiments` and `load_results` as the release before this one
    /// created them, with a row each.
    fn earlier_shape(db: &std::path::Path) {
        let conn = duckdb::Connection::open(db).expect("open");
        for ddl in [
            super::super::telemetry::DDL,
            super::super::analytics::DDL,
            super::super::objectives::DDL,
        ] {
            conn.execute_batch(ddl).expect("earlier ddl");
        }
        conn.execute_batch(
            "INSERT INTO load_results VALUES \
             ('old-run', 'k6', 0, 1, 1.0, 1, 1.0, 1.0, 1.0, 1.0, 0.0, 1, true); \
             INSERT INTO experiments (experiment_id, title, status, started_at_ns, \
             ended_at_ns, duration_ms, method_step_count, rollback_count) \
             VALUES ('old-run', 'old', 'completed', 0, 1, 1, 0, 0);",
        )
        .expect("earlier rows");
    }

    #[test]
    fn tables_of_the_earlier_shape_gain_the_run_columns_and_keep_their_rows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("lake.duckdb");
        earlier_shape(&db);

        let store = Store::open(&db).expect("the old lake opens");
        let reader = store.read_only().expect("reader");
        let one = |sql: &str| -> i64 {
            reader.query_json_rows(sql).expect("query")[0]["n"]
                .as_i64()
                .expect("count")
        };
        assert_eq!(
            one("SELECT count(*) AS n FROM experiments \
                 WHERE experiment_id = 'old-run' AND target_version IS NULL \
                 AND version_source IS NULL"),
            1
        );
        assert_eq!(
            one("SELECT count(*) AS n FROM load_results \
                 WHERE experiment_id = 'old-run' AND aborted IS NULL AND truncated IS NULL \
                 AND planned_windows IS NULL AND completed_windows IS NULL \
                 AND unanswered_windows IS NULL AND lost_to_unanswered IS NULL \
                 AND sampled_duration_s IS NULL"),
            1
        );
        assert_eq!(
            one("SELECT count(*) AS n FROM service_version_sightings"),
            0
        );
        // No schema version was spent on it.
        drop(reader);
        let writer = store.writer().expect("writer");
        assert_eq!(
            writer.schema_version().expect("version"),
            crate::CURRENT_SCHEMA_VERSION
        );
    }
}
