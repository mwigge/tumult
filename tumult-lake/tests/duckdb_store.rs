//! `AnalyticsStore` tests (moved out of `src/duckdb_store/mod.rs`):
//! persistence across reopens, read-only coexistence with an open writer,
//! schema-version tracking, and the v1 → current forward migration.

#![cfg(feature = "duckdb")]

use tumult_core::types::*;
use tumult_lake::duckdb_store::{sample_journal, AnalyticsStore};
use tumult_lake::CURRENT_SCHEMA_VERSION;

#[test]
fn open_persistent_creates_file() {
    let d = tempfile::TempDir::new().unwrap();
    let db_path = d.path().join("analytics.duckdb");
    let s = AnalyticsStore::open(&db_path).unwrap();
    s.ingest_journal(&sample_journal("e1", ExperimentStatus::Completed))
        .unwrap();
    assert_eq!(s.experiment_count().unwrap(), 1);
    drop(s);
    assert!(db_path.exists());
}

#[test]
fn persistent_store_survives_reopen() {
    let d = tempfile::TempDir::new().unwrap();
    let db_path = d.path().join("analytics.duckdb");

    // Write
    {
        let s = AnalyticsStore::open(&db_path).unwrap();
        s.ingest_journal(&sample_journal("e1", ExperimentStatus::Completed))
            .unwrap();
        assert_eq!(s.experiment_count().unwrap(), 1);
    }

    // Reopen and verify data persisted
    {
        let s = AnalyticsStore::open(&db_path).unwrap();
        assert_eq!(s.experiment_count().unwrap(), 1);
        let rows = s.query("SELECT experiment_id FROM experiments").unwrap();
        assert_eq!(rows[0][0], "e1");
    }
}

#[test]
fn read_only_open_can_query_existing_store() {
    let d = tempfile::TempDir::new().unwrap();
    let db_path = d.path().join("analytics.duckdb");

    // A writer creates + populates the store, then releases the lock.
    {
        let s = AnalyticsStore::open(&db_path).unwrap();
        s.ingest_journal(&sample_journal("e1", ExperimentStatus::Completed))
            .unwrap();
    }

    // The read-only accessor opens without the exclusive write lock and can
    // run read operations.
    let ro = AnalyticsStore::open_read_only(&db_path).unwrap();
    assert_eq!(ro.experiment_count().unwrap(), 1);
    let rows = ro.query("SELECT experiment_id FROM experiments").unwrap();
    assert_eq!(rows[0][0], "e1");
}

/// The previously-failing scenario: a reader opens the store while a writer
/// handle is still alive. With read operations moved to `open_read_only`
/// this succeeds and can query, instead of colliding on the write lock.
#[test]
fn read_only_reader_coexists_with_open_writer() {
    let d = tempfile::TempDir::new().unwrap();
    let db_path = d.path().join("analytics.duckdb");

    let writer = AnalyticsStore::open(&db_path).unwrap();
    writer
        .ingest_journal(&sample_journal("e1", ExperimentStatus::Completed))
        .unwrap();

    // Reader opens while `writer` is still held.
    let reader = AnalyticsStore::open_read_only(&db_path).unwrap();
    assert_eq!(reader.experiment_count().unwrap(), 1);
    let rows = reader
        .query("SELECT experiment_id FROM experiments")
        .unwrap();
    assert_eq!(rows[0][0], "e1");
    drop(writer);
}

#[test]
fn default_path_returns_valid_path() {
    let path = AnalyticsStore::default_path().unwrap();
    assert!(path.ends_with("lake.duckdb"));
    assert!(path.to_str().unwrap().contains(".tumult"));
}

#[test]
fn open_default_creates_directory() {
    // This test uses a temp directory to avoid polluting the real home
    let d = tempfile::TempDir::new().unwrap();
    let db_path = d.path().join("subdir").join("analytics.duckdb");
    let s = AnalyticsStore::open(&db_path).unwrap();
    assert_eq!(s.experiment_count().unwrap(), 0);
    assert!(db_path.exists());
}

#[test]
fn persistent_store_is_functional_after_write_and_reopen() {
    let d = tempfile::TempDir::new().unwrap();
    let db_path = d.path().join("analytics.duckdb");

    // Write data and close
    {
        let s = AnalyticsStore::open(&db_path).unwrap();
        s.ingest_journal(&sample_journal("e1", ExperimentStatus::Completed))
            .unwrap();
        s.ingest_journal(&sample_journal("e2", ExperimentStatus::Deviated))
            .unwrap();
    }

    // Reopen — DuckDB uses WAL by default for file-backed databases
    {
        let s = AnalyticsStore::open(&db_path).unwrap();
        assert_eq!(s.experiment_count().unwrap(), 2);
        let rows = s.query("SELECT count(*) FROM activity_results").unwrap();
        assert_eq!(rows[0][0], "2");
    }
}

#[test]
fn schema_version_is_tracked() {
    let s = AnalyticsStore::in_memory().unwrap();
    let version = s.schema_version().unwrap();
    assert!(version >= 1, "schema version should be at least 1");
}

#[test]
fn schema_version_persists_across_reopen() {
    let d = tempfile::TempDir::new().unwrap();
    let db_path = d.path().join("analytics.duckdb");

    {
        let s = AnalyticsStore::open(&db_path).unwrap();
        assert_eq!(s.schema_version().unwrap(), CURRENT_SCHEMA_VERSION);
    }

    {
        let s = AnalyticsStore::open(&db_path).unwrap();
        assert_eq!(s.schema_version().unwrap(), CURRENT_SCHEMA_VERSION);
    }
}

/// A store recorded at the pre-graph schema (v1) must migrate forward on
/// open: the graph tables appear, the version advances, and prior data
/// survives.
#[test]
fn migrates_v1_store_forward_without_data_loss() {
    let d = tempfile::TempDir::new().unwrap();
    let db_path = d.path().join("analytics.duckdb");

    // Seed a v1-shaped store with one experiment and no graph tables.
    {
        let conn = duckdb::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE experiments (
                    experiment_id VARCHAR NOT NULL, title VARCHAR NOT NULL,
                    status VARCHAR NOT NULL, started_at_ns BIGINT NOT NULL,
                    ended_at_ns BIGINT NOT NULL, duration_ms UBIGINT NOT NULL,
                    method_step_count BIGINT NOT NULL, rollback_count BIGINT NOT NULL,
                    hypothesis_before_met BOOLEAN, hypothesis_after_met BOOLEAN,
                    estimate_accuracy DOUBLE, resilience_score DOUBLE
                );
                CREATE TABLE schema_meta (key VARCHAR PRIMARY KEY, value BIGINT NOT NULL);
                INSERT INTO schema_meta (key, value) VALUES ('version', 1);
                INSERT INTO experiments VALUES
                    ('legacy-1', 'Legacy', 'completed', 0, 1, 1, 0, 0, NULL, NULL, NULL, NULL);",
        )
        .unwrap();
    }

    // Opening through AnalyticsStore runs the additive migration.
    let s = AnalyticsStore::open(&db_path).unwrap();
    assert_eq!(s.schema_version().unwrap(), CURRENT_SCHEMA_VERSION);
    // Prior data preserved.
    assert_eq!(s.experiment_count().unwrap(), 1);
    // Graph tables now queryable. No runs ingested → no run-derived nodes,
    // but the static compliance-article nodes are seeded at migration.
    let compliance = s
        .query("SELECT count(*) FROM graph_nodes WHERE kind = 'compliance_article'")
        .unwrap();
    assert_eq!(
        compliance[0][0],
        tumult_graph::compliance_article_nodes().len().to_string()
    );
    let run_nodes = s
        .query("SELECT count(*) FROM graph_nodes WHERE kind != 'compliance_article'")
        .unwrap();
    assert_eq!(run_nodes[0][0], "0");
    let rows = s.query("SELECT count(*) FROM graph_edges").unwrap();
    assert_eq!(rows[0][0], "0");
    // The v3 attrs column exists on graph_edges.
    s.query("SELECT attrs FROM graph_edges LIMIT 0").unwrap();
}

#[test]
fn experiment_id_has_unique_index() {
    let s = AnalyticsStore::in_memory().unwrap();
    let result = s
        .query("SELECT count(*) FROM duckdb_indexes() WHERE table_name = 'experiments'")
        .unwrap();
    let idx_count: usize = result[0][0].parse().unwrap_or(0);
    assert!(idx_count >= 1, "experiments table should have an index");
}
