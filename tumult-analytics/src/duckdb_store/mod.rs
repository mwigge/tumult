//! `DuckDB` embedded analytics store.
//!
//! Provides both in-memory and persistent (file-backed) analytics stores.
//! Persistent stores use WAL mode for crash safety, deduplicate journals
//! by `experiment_id`, and support schema versioning for future migrations.
//!
//! **Thread safety:** `AnalyticsStore` wraps a single `DuckDB` `Connection` and
//! is NOT thread-safe. For shared access, wrap in `Arc<Mutex<AnalyticsStore>>`.
//!
//! **Encryption limitation:** `DuckDB` does not support transparent
//! encryption-at-rest. The database file is stored in plaintext on disk.
//! Protect sensitive experiment data by relying on filesystem-level encryption
//! (e.g. LUKS, `FileVault`, `BitLocker`) and by restricting the store directory
//! permissions to `0o700` (which [`AnalyticsStore::open`] applies automatically).

use std::path::{Path, PathBuf};

use duckdb::{params, Connection};

use crate::error::AnalyticsError;

mod graph;
mod ingest;
mod maintenance;
mod query;
mod types;

pub use types::{AgenticContractAnalytics, AgenticFaultAnalytics, AgenticRunAnalytics, StoreStats};

/// Schema history:
/// * v1 — experiments, activity/load results, agentic tables.
/// * v2 — `ChaosGraph` `graph_nodes` / `graph_edges` (additive, no data loss).
const CURRENT_SCHEMA_VERSION: i64 = 2;

/// Embedded `DuckDB` analytics store for experiment journals.
///
/// **Not thread-safe.** Each instance holds a single `DuckDB` connection.
/// For concurrent access, wrap in `Arc<Mutex<AnalyticsStore>>`.
///
/// # Security
///
/// `DuckDB` does not encrypt data at rest by default. The database file at
/// `~/.tumult/analytics.duckdb` is stored in plaintext on disk. For
/// environments where experiment data is sensitive, place the store on an
/// encrypted volume:
///
/// - **Linux**: LUKS full-disk or directory encryption (`fscrypt`, `ecryptfs`)
/// - **macOS**: `FileVault 2` (whole-disk) or an encrypted APFS volume
/// - **Windows**: `BitLocker` or an encrypted home directory
///
/// The store directory is automatically created with mode `0o700` (owner
/// read/write/execute only) by [`AnalyticsStore::open`], limiting access to
/// the process owner. However, directory permissions are not a substitute
/// for encryption — a privileged user or physical attacker can still access
/// the file without encryption.
///
/// Use the `TUMULT_STORE_PATH` environment variable to redirect the persistent
/// store to a path on an encrypted volume when the default location is not
/// suitable.
pub struct AnalyticsStore {
    conn: Connection,
}

impl AnalyticsStore {
    /// Returns the default persistent store path: `~/.tumult/analytics.duckdb`
    ///
    /// # Panics
    ///
    /// Panics if the home directory cannot be determined.
    #[must_use]
    pub fn default_path() -> PathBuf {
        let home = dirs_next::home_dir().expect("cannot determine home directory");
        home.join(".tumult").join("analytics.duckdb")
    }

    /// # Errors
    ///
    /// Returns an error if the in-memory `DuckDB` connection or schema initialisation fails.
    #[must_use = "callers must handle connection or schema errors"]
    pub fn in_memory() -> Result<Self, AnalyticsError> {
        let conn = Connection::open_in_memory()?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    /// # Errors
    ///
    /// Returns an error if the `DuckDB` file cannot be opened or schema initialisation fails.
    #[must_use = "callers must handle file open or schema errors"]
    pub fn open(path: &Path) -> Result<Self, AnalyticsError> {
        // Ensure parent directory exists with restricted permissions
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
            }
        }
        let conn = Connection::open(path)?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<(), AnalyticsError> {
        self.create_tables()?;
        self.ensure_schema_version()?;
        Ok(())
    }

    fn create_tables(&self) -> Result<(), AnalyticsError> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS experiments (
                experiment_id VARCHAR NOT NULL, title VARCHAR NOT NULL,
                status VARCHAR NOT NULL, started_at_ns BIGINT NOT NULL,
                ended_at_ns BIGINT NOT NULL, duration_ms UBIGINT NOT NULL,
                method_step_count BIGINT NOT NULL, rollback_count BIGINT NOT NULL,
                hypothesis_before_met BOOLEAN, hypothesis_after_met BOOLEAN,
                estimate_accuracy DOUBLE, resilience_score DOUBLE
            );
            CREATE UNIQUE INDEX IF NOT EXISTS idx_experiments_id
                ON experiments (experiment_id);
            CREATE TABLE IF NOT EXISTS activity_results (
                experiment_id VARCHAR NOT NULL, name VARCHAR NOT NULL,
                activity_type VARCHAR NOT NULL, status VARCHAR NOT NULL,
                started_at_ns BIGINT NOT NULL, duration_ms UBIGINT NOT NULL,
                output VARCHAR, error VARCHAR, phase VARCHAR NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_activities_experiment_id
                ON activity_results (experiment_id);
            CREATE TABLE IF NOT EXISTS load_results (
                experiment_id VARCHAR NOT NULL, tool VARCHAR NOT NULL,
                started_at_ns BIGINT NOT NULL, ended_at_ns BIGINT NOT NULL,
                duration_s DOUBLE NOT NULL, vus INTEGER NOT NULL,
                throughput_rps DOUBLE NOT NULL, latency_p50_ms DOUBLE NOT NULL,
                latency_p95_ms DOUBLE NOT NULL, latency_p99_ms DOUBLE NOT NULL,
                error_rate DOUBLE NOT NULL, total_requests UBIGINT NOT NULL,
                thresholds_met BOOLEAN NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_load_experiment_id
                ON load_results (experiment_id);
            CREATE TABLE IF NOT EXISTS agentic_runs (
                run_id VARCHAR PRIMARY KEY, experiment_id VARCHAR NOT NULL,
                target_type VARCHAR NOT NULL, scenario VARCHAR NOT NULL,
                resilience_score DOUBLE NOT NULL, trace_id VARCHAR, replay_id VARCHAR
            );
            CREATE INDEX IF NOT EXISTS idx_agentic_runs_experiment_id
                ON agentic_runs (experiment_id);
            CREATE TABLE IF NOT EXISTS agentic_contract_outcomes (
                run_id VARCHAR NOT NULL, scenario VARCHAR NOT NULL,
                contract_type VARCHAR NOT NULL, passed BOOLEAN NOT NULL,
                reason VARCHAR, severity DOUBLE NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_agentic_contracts_run_id
                ON agentic_contract_outcomes (run_id);
            CREATE TABLE IF NOT EXISTS agentic_fault_applications (
                run_id VARCHAR NOT NULL, scenario VARCHAR NOT NULL,
                fault_type VARCHAR NOT NULL, applied BOOLEAN NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_agentic_faults_run_id
                ON agentic_fault_applications (run_id);
            CREATE TABLE IF NOT EXISTS agentic_replay_outcomes (
                run_id VARCHAR NOT NULL, replay_id VARCHAR NOT NULL,
                scenario VARCHAR NOT NULL, passed BOOLEAN NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_agentic_replay_run_id
                ON agentic_replay_outcomes (run_id);
            CREATE TABLE IF NOT EXISTS schema_meta (
                key VARCHAR PRIMARY KEY, value BIGINT NOT NULL
            );",
        )?;
        // ChaosGraph node/edge tables (schema v2). `IF NOT EXISTS` makes this
        // both the fresh-install DDL and the additive v1 → v2 migration: an
        // existing store simply gains the two tables, keeping all prior data.
        self.conn.execute_batch(tumult_graph::sql::CREATE_TABLES)?;
        Ok(())
    }

    fn ensure_schema_version(&self) -> Result<(), AnalyticsError> {
        let mut stmt = self
            .conn
            .prepare("SELECT value FROM schema_meta WHERE key = 'version'")?;
        // Read as i64 directly — the column is now BIGINT, no String round-trip.
        let version: Option<i64> = stmt.query_row(params![], |row| row.get(0)).ok();

        match version {
            None => {
                self.conn.execute(
                    "INSERT INTO schema_meta (key, value) VALUES ('version', ?)",
                    // Bind i64 directly — avoids a String allocation and type mismatch.
                    params![CURRENT_SCHEMA_VERSION],
                )?;
            }
            Some(stored) if stored < CURRENT_SCHEMA_VERSION => {
                // Migrations are additive and already applied by
                // `create_tables` (every DDL is `IF NOT EXISTS`); here we only
                // advance the recorded version so the upgrade is observable.
                self.conn.execute(
                    "UPDATE schema_meta SET value = ? WHERE key = 'version'",
                    params![CURRENT_SCHEMA_VERSION],
                )?;
            }
            Some(_) => {}
        }
        Ok(())
    }

    /// # Errors
    ///
    /// Returns an error if the schema version cannot be read.
    #[must_use = "callers must use the returned schema version"]
    pub fn schema_version(&self) -> Result<i64, AnalyticsError> {
        let mut stmt = self
            .conn
            .prepare("SELECT value FROM schema_meta WHERE key = 'version'")?;
        // Column is BIGINT — no String parse round-trip needed.
        stmt.query_row(params![], |row| row.get(0))
            .map_err(AnalyticsError::from)
    }
}

#[cfg(test)]
pub(crate) fn sample_journal(
    id: &str,
    status: tumult_core::types::ExperimentStatus,
) -> tumult_core::types::Journal {
    use tumult_core::types::*;
    Journal {
        experiment_title: format!("Test {id}"),
        experiment_id: id.into(),
        status,
        started_at_ns: 1_774_980_000_000_000_000,
        ended_at_ns: 1_774_980_300_000_000_000,
        duration_ms: 300_000,
        steady_state_before: None,
        steady_state_after: None,
        method_results: vec![ActivityResult {
            name: "action-1".into(),
            activity_type: ActivityType::Action,
            status: ActivityStatus::Succeeded,
            started_at_ns: 1_774_980_135_000_000_000,
            duration_ms: 500,
            output: Some("done".into()),
            error: None,
            trace_id: "t1".into(),
            span_id: "s1".into(),
        }],
        rollback_results: vec![],
        rollback_failures: 0,
        halt: None,
        blast_radius: None,
        estimate: None,
        baseline_result: None,
        during_result: None,
        post_result: None,
        load_result: None,
        analysis: Some(AnalysisResult {
            estimate_accuracy: Some(1.0),
            estimate_recovery_delta_s: None,
            trend: None,
            resilience_score: Some(0.95),
        }),
        regulatory: None,
    }
}

#[cfg(test)]
mod tests {
    use super::sample_journal;
    use super::AnalyticsStore;
    use super::CURRENT_SCHEMA_VERSION;
    use tumult_core::types::*;

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
    fn default_path_returns_valid_path() {
        let path = AnalyticsStore::default_path();
        assert!(path.ends_with("analytics.duckdb"));
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
        // Graph tables now queryable.
        let rows = s.query("SELECT count(*) FROM graph_nodes").unwrap();
        assert_eq!(rows[0][0], "0");
        let rows = s.query("SELECT count(*) FROM graph_edges").unwrap();
        assert_eq!(rows[0][0], "0");
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
}
