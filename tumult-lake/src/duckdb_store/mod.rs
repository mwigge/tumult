//! `DuckDB` embedded analytics store.
//!
//! Provides both in-memory and persistent (file-backed) analytics stores.
//! Persistent stores use WAL mode for crash safety, deduplicate journals
//! by `experiment_id`, and support schema versioning for future migrations.
//!
//! **Thread safety:** `AnalyticsStore` wraps a single `DuckDB` `Connection` and
//! is NOT thread-safe. For shared access, wrap in `Arc<Mutex<AnalyticsStore>>`.
//!
//! # Concurrency: the single-writer model
//!
//! `DuckDB` is **single-writer per file**. A read-write connection takes an
//! *exclusive* lock on the database file, so at most one process may hold the
//! store open read-write at a time. This matters because Tumult opens the same
//! `~/.tumult/lake.duckdb` from two places — the CLI (`tumult run` ingest,
//! `tumult analyze`, `tumult chaosgraph`, …) and the long-running MCP server.
//!
//! To keep these from sabotaging each other:
//!
//! * **Reads open read-only.** [`AnalyticsStore::open_read_only`] opens with
//!   `access_mode = READ_ONLY`. Read-only opens do not take the exclusive write
//!   lock, so **multiple read-only processes coexist** — the CLI can query the
//!   store while the MCP server also holds it open for reads. Use this for every
//!   read path (`query`, `stats`, coverage, and the `tumult-query` domain reads).
//! * **Writes open read-write.** [`AnalyticsStore::open`] keeps the read-write
//!   open used by ingest and other write paths, and it also initialises/migrates
//!   the schema.
//! * **Two writers still conflict.** Because a read-write open is exclusive, it
//!   blocks *every* other opener (readers included) while it is held. If a write
//!   (or read) open fails because another process holds the store — most often
//!   the MCP server mid-ingest — the opaque `DuckDB` lock error is mapped to the
//!   clear [`AnalyticsError::StoreLocked`], which tells the user to stop the
//!   server or use a separate `--store` path. A short bounded retry absorbs the
//!   brief window while the other process finishes a write.
//!
//! **Encryption limitation:** `DuckDB` does not support transparent
//! encryption-at-rest. The database file is stored in plaintext on disk.
//! Protect sensitive experiment data by relying on filesystem-level encryption
//! (e.g. LUKS, `FileVault`, `BitLocker`) and by restricting the store directory
//! permissions to `0o700` (which [`AnalyticsStore::open`] applies automatically).

use std::path::{Path, PathBuf};
use std::time::Duration;

use duckdb::{params, AccessMode, Config, Connection};

use crate::error::AnalyticsError;

/// Total attempts a write/read open makes before reporting the store as locked.
/// A single retry window absorbs the brief moment another process holds the
/// store while finishing a write; a persistent holder still fails fast.
const OPEN_ATTEMPTS: u32 = 3;

/// Backoff between open attempts when the store is momentarily locked.
const OPEN_BACKOFF: Duration = Duration::from_millis(50);

/// Whether a `DuckDB` error is the file-lock conflict raised when another
/// process already holds the store open. The message is stable across the
/// read-write ("Could not set lock … Conflicting lock is held") and read-only
/// contention paths.
fn is_lock_conflict(err: &duckdb::Error) -> bool {
    matches!(
        err,
        duckdb::Error::DuckDBFailure(_, Some(msg))
            if msg.contains("Could not set lock") || msg.contains("Conflicting lock")
    )
}

/// Open a `DuckDB` connection with a short bounded retry, mapping a persistent
/// lock conflict to [`AnalyticsError::StoreLocked`]. `open` supplies the actual
/// open (read-write or read-only); any non-lock error propagates immediately.
fn open_with_retry(
    path: &Path,
    open: impl Fn() -> Result<Connection, duckdb::Error>,
) -> Result<Connection, AnalyticsError> {
    let mut attempt = 1;
    loop {
        match open() {
            Ok(conn) => return Ok(conn),
            Err(err) if is_lock_conflict(&err) => {
                if attempt >= OPEN_ATTEMPTS {
                    return Err(AnalyticsError::StoreLocked {
                        path: path.to_path_buf(),
                    });
                }
                std::thread::sleep(OPEN_BACKOFF);
                attempt += 1;
            }
            Err(err) => return Err(AnalyticsError::from(err)),
        }
    }
}

pub mod autopilot;
mod graph;
mod ingest;
mod legacy;
mod maintenance;
mod query;
mod topology;
mod types;

pub use types::{AgenticContractAnalytics, AgenticFaultAnalytics, AgenticRunAnalytics, StoreStats};

pub(crate) use ingest::ingest_journal_with_experiment;

/// Embedded `DuckDB` analytics store for experiment journals.
///
/// **Not thread-safe.** Each instance holds a single `DuckDB` connection.
/// For concurrent access, wrap in `Arc<Mutex<AnalyticsStore>>`.
///
/// # Security
///
/// `DuckDB` does not encrypt data at rest by default. The database file at
/// `~/.tumult/lake.duckdb` is stored in plaintext on disk. For
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
/// Use the `TUMULT_LAKE_PATH` environment variable to redirect the persistent
/// store to a path on an encrypted volume when the default location is not
/// suitable.
pub struct AnalyticsStore {
    conn: Connection,
}

impl AnalyticsStore {
    /// Returns the default persistent store path: `~/.tumult/lake.duckdb`
    ///
    /// Resolution order:
    /// 1. `TUMULT_LAKE_PATH` — the unified store path override.
    /// 2. `TUMULT_ANALYTICS_PATH` — deprecated alias (one release of grace;
    ///    a warning is printed when it takes effect).
    /// 3. `~/.tumult/lake.duckdb`.
    ///
    /// # Errors
    ///
    /// Returns [`AnalyticsError::Internal`] if the home directory cannot be
    /// determined and no override is set.
    pub fn default_path() -> Result<PathBuf, AnalyticsError> {
        // Explicit override first — lets scripts and demos isolate a store
        // without threading a flag through every command.
        if let Ok(path) = std::env::var("TUMULT_LAKE_PATH") {
            if !path.is_empty() {
                return Ok(PathBuf::from(path));
            }
        }
        if let Ok(path) = std::env::var("TUMULT_ANALYTICS_PATH") {
            if !path.is_empty() {
                eprintln!(
                    "warning: TUMULT_ANALYTICS_PATH is deprecated; the unified store now \
                     lives at TUMULT_LAKE_PATH (default ~/.tumult/lake.duckdb). Migrate with \
                     `tumult store import-legacy` and unset TUMULT_ANALYTICS_PATH."
                );
                return Ok(PathBuf::from(path));
            }
        }
        let home = dirs_next::home_dir().ok_or_else(|| {
            AnalyticsError::Internal(
                "cannot determine home directory; set TUMULT_LAKE_PATH to an explicit \
                 store path"
                    .to_string(),
            )
        })?;
        Ok(home.join(".tumult").join("lake.duckdb"))
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

    /// Open the persistent store **read-write** (the writer path).
    ///
    /// This takes `DuckDB`'s exclusive write lock, creates the store directory
    /// (mode `0o700` on Unix), and initialises/migrates the schema. Use it for
    /// ingest and every other write path.
    ///
    /// Because the lock is exclusive, this fails while any other process holds
    /// the same store open. When it does, the opaque `DuckDB` lock error is
    /// mapped to [`AnalyticsError::StoreLocked`]. For read-only access that can
    /// coexist with other readers, use [`AnalyticsStore::open_read_only`].
    ///
    /// # Errors
    ///
    /// Returns [`AnalyticsError::StoreLocked`] if another process holds the
    /// store, or another error if the `DuckDB` file cannot be opened or schema
    /// initialisation fails.
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
        let conn = open_with_retry(path, || Connection::open(path))?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    /// Open the persistent store **read-only** (the reader path).
    ///
    /// A read-only connection (`access_mode = READ_ONLY`) does not take the
    /// exclusive write lock, so multiple read-only openers coexist across
    /// processes: the CLI can query the store while the MCP server also holds it
    /// open. Use this for every read operation — [`AnalyticsStore::query`],
    /// [`AnalyticsStore::stats`], coverage, the `tumult-query` domain reads
    /// (`graph_query`, `graph_neighbors`, …), and so on.
    ///
    /// The store must already exist and have been initialised by a writer;
    /// read-only opens neither create nor migrate the schema. If the open fails
    /// because another process holds the store read-write (an exclusive lock
    /// blocks readers too), the error is mapped to
    /// [`AnalyticsError::StoreLocked`].
    ///
    /// # Errors
    ///
    /// Returns [`AnalyticsError::StoreLocked`] if a writer holds the store, or
    /// another error if the `DuckDB` file cannot be opened read-only (e.g. it
    /// does not exist yet).
    #[must_use = "callers must handle file open errors"]
    pub fn open_read_only(path: &Path) -> Result<Self, AnalyticsError> {
        let conn = open_with_retry(path, || {
            let config = Config::default().access_mode(AccessMode::ReadOnly)?;
            Connection::open_with_flags(path, config)
        })?;
        Ok(Self { conn })
    }

    fn init_schema(&self) -> Result<(), AnalyticsError> {
        // The unified v3 migration (telemetry + manual + analytics families,
        // ChaosGraph tables, experiment_runs view, compliance-article seed,
        // schema_meta version) — identical to what `Store::open` runs, so a
        // database opened through either write path has the same schema.
        crate::migrate(&self.conn)?;
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

    /// Borrow the underlying connection — crate-crossing escape hatch for
    /// `tumult-query`'s read-only domain queries. Not public API; the stable
    /// surface is the typed accessor methods plus `tumult-query`.
    #[doc(hidden)]
    #[must_use]
    pub fn __connection(&self) -> &Connection {
        &self.conn
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
    use crate::CURRENT_SCHEMA_VERSION;
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
}
