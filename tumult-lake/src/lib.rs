// Imported from kronika (Apache-2.0, same author). Pedantic lints are
// scoped to tumult-native crates: this crate predates the pedantic gate and
// carries intentional patterns it flags (timestamp/score casts, f64
// comparisons). CI still applies -D warnings to it.
#![allow(clippy::pedantic)]

//! `tumult-lake` — the unified embedded `DuckDB` store: chaos/resilience
//! telemetry (spans, logs, metrics), manual evidence, and the journal
//! analytics family (experiments, activities, ChaosGraph, autopilot) in one
//! database file, behind one writer.
//!
//! # Concurrency: the single-writer model
//!
//! `DuckDB` is **single-writer per file**. A read-write open takes an
//! exclusive lock on the database file, so:
//!
//! * **Writes** go through [`Store::writer`] (one per process; the ingest
//!   daemon funnels all writes through a single channel onto it) or
//!   [`AnalyticsStore::open`] for the journal-analytics family.
//! * **Reads** go through [`Store::read_only`] /
//!   [`AnalyticsStore::open_read_only`], opened with
//!   `access_mode = READ_ONLY`, which does not take the exclusive write lock —
//!   multiple readers coexist, including alongside an open writer.
//! * A conflicting second opener gets the opaque `DuckDB` lock error mapped to
//!   the clear [`StoreError::StoreLocked`] / [`AnalyticsError::StoreLocked`].
//!
//! **Encryption limitation:** `DuckDB` does not encrypt at rest. The store
//! directory is created with mode `0o700` (owner-only); place it on an
//! encrypted volume for sensitive data.
//!
//! # Features
//!
//! * `duckdb` (default) — the embedded store. Disable default features to
//!   get only the lightweight backend trait and shared types
//!   ([`AnalyticsBackend`], [`AnalyticsError`], [`QueryRow`], [`StoreStats`],
//!   [`telemetry`]) without compiling the bundled `DuckDB` C++ library —
//!   this is what `tumult-clickhouse` does.

#[cfg(feature = "duckdb")]
pub mod arrow_convert;
pub mod backend;
#[cfg(feature = "duckdb")]
pub mod duckdb_store;
pub mod error;
#[cfg(feature = "duckdb")]
pub mod export;
#[cfg(feature = "duckdb")]
pub mod lake;
#[cfg(feature = "duckdb")]
mod manual;
pub mod query_row;
#[cfg(feature = "duckdb")]
mod rows;
#[cfg(feature = "duckdb")]
mod schema;
pub mod telemetry;

#[cfg(feature = "duckdb")]
use std::path::{Path, PathBuf};
#[cfg(feature = "duckdb")]
use std::time::Duration;

#[cfg(feature = "duckdb")]
use duckdb::{params, AccessMode, Config, Connection};

pub use backend::{AnalyticsBackend, StoreStats};
pub use error::AnalyticsError;
#[cfg(feature = "duckdb")]
pub use error::StoreError;
#[cfg(feature = "duckdb")]
pub use manual::{
    AttachmentKind, ExerciseType, ManualDetail, ManualError, ManualOutcome, NewManualExperiment,
};
pub use query_row::QueryRow;
#[cfg(feature = "duckdb")]
pub use rows::{
    ExperimentRun, ImportBatch, LogRow, MetricGaugeRow, MetricHistogramRow, MetricSumRow, SpanRow,
};
#[cfg(feature = "duckdb")]
pub use schema::CURRENT_SCHEMA_VERSION;

#[cfg(feature = "duckdb")]
pub use arrow_convert::journal_to_record_batch;
#[cfg(feature = "duckdb")]
pub use duckdb_store::autopilot::{
    ChangeEventRecord, ClassHistory, DecisionRecord, DecisionStatus,
};
#[cfg(feature = "duckdb")]
pub use duckdb_store::topology::NodeAttrs;
#[cfg(feature = "duckdb")]
pub use duckdb_store::{
    AgenticContractAnalytics, AgenticFaultAnalytics, AgenticRunAnalytics, AnalyticsStore,
};
#[cfg(feature = "duckdb")]
pub use export::{export_arrow_ipc, export_csv, export_parquet, import_parquet};

#[cfg(feature = "duckdb")]
/// Total attempts an open makes before reporting the store as locked.
const OPEN_ATTEMPTS: u32 = 3;
#[cfg(feature = "duckdb")]
/// Backoff between open attempts while another process finishes a write.
const OPEN_BACKOFF: Duration = Duration::from_millis(50);

#[cfg(feature = "duckdb")]
/// Whether a `DuckDB` error is the file-lock conflict raised when another
/// process already holds the store open.
fn is_lock_conflict(err: &duckdb::Error) -> bool {
    matches!(
        err,
        duckdb::Error::DuckDBFailure(_, Some(msg))
            if msg.contains("Could not set lock") || msg.contains("Conflicting lock")
    )
}

#[cfg(feature = "duckdb")]
/// Open a `DuckDB` connection with a short bounded retry, mapping a persistent
/// lock conflict to [`StoreError::StoreLocked`].
fn open_with_retry(
    path: &Path,
    open: impl Fn() -> Result<Connection, duckdb::Error>,
) -> Result<Connection, StoreError> {
    let mut attempt = 1;
    loop {
        match open() {
            Ok(conn) => return Ok(conn),
            Err(err) if is_lock_conflict(&err) => {
                if attempt >= OPEN_ATTEMPTS {
                    return Err(StoreError::StoreLocked {
                        path: path.to_path_buf(),
                    });
                }
                std::thread::sleep(OPEN_BACKOFF);
                attempt += 1;
            }
            Err(err) => return Err(StoreError::from(err)),
        }
    }
}

#[cfg(feature = "duckdb")]
/// Serialise attribute pairs as a JSON object for binding into a
/// `MAP(VARCHAR, VARCHAR)` column (`CAST(json(?) AS MAP(VARCHAR,VARCHAR))`).
fn attrs_json(attrs: &[(String, String)]) -> Result<String, StoreError> {
    let map: serde_json::Map<String, serde_json::Value> = attrs
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
        .collect();
    Ok(serde_json::Value::Object(map).to_string())
}

#[cfg(feature = "duckdb")]
/// A handle to the store file. Cheap to construct; connections are opened
/// per role via [`Store::writer`] and [`Store::read_only`].
pub struct Store {
    path: PathBuf,
}

#[cfg(feature = "duckdb")]
impl Store {
    /// Open (creating if needed) the store at `path`, run schema migrations,
    /// and return a handle. The store directory is created with mode `0o700`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::StoreLocked`] if another process holds the store
    /// read-write, or another error if the file cannot be opened or migrated.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
            }
        }
        let store = Self {
            path: path.to_path_buf(),
        };
        // Open once read-write to initialise/migrate the schema.
        let writer = store.writer()?;
        writer.migrate()?;
        Ok(store)
    }

    /// Path of the underlying database file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A handle to an existing store file WITHOUT opening any connection.
    /// Use this for read-only access that must not touch the exclusive write
    /// lock (e.g. `kronikad report` while the daemon holds the writer).
    #[must_use]
    pub fn at(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
        }
    }

    /// Open the single-writer read-write connection.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::StoreLocked`] if ANOTHER PROCESS holds a
    /// read-write connection to the same file. (Within one process,
    /// `duckdb-rs` shares a single instance per path, so the daemon keeps
    /// exactly one `Writer` by construction — one channel, one task.)
    pub fn writer(&self) -> Result<Writer, StoreError> {
        let conn = open_with_retry(&self.path, || Connection::open(&self.path))?;
        Ok(Writer { conn })
    }

    /// Open a read-only connection (`access_mode = READ_ONLY`). Multiple
    /// read-only connections coexist across processes, including next to an
    /// open writer. The store must already exist and be migrated.
    ///
    /// The connection pins its snapshot at open: it does NOT observe writes
    /// committed afterwards — open a fresh reader per unit of work (the API
    /// opens one per request; the lake scheduler one per run).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::StoreLocked`] if the open is blocked, or another
    /// error if the file does not exist yet.
    pub fn read_only(&self) -> Result<Reader, StoreError> {
        let conn = open_with_retry(&self.path, || {
            let config = Config::default().access_mode(AccessMode::ReadOnly)?;
            Connection::open_with_flags(&self.path, config)
        })?;
        Ok(Reader { conn })
    }
}

#[cfg(feature = "duckdb")]
/// Run `f` inside a transaction on `conn` (single-writer, so a plain
/// `BEGIN`/`COMMIT` batch is enough).
fn with_tx<T, E: From<StoreError>>(
    conn: &Connection,
    f: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    conn.execute_batch("BEGIN TRANSACTION")
        .map_err(StoreError::from)?;
    match f() {
        Ok(v) => {
            conn.execute_batch("COMMIT").map_err(StoreError::from)?;
            Ok(v)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

#[cfg(feature = "duckdb")]
/// Shared schema migration: the full v3 DDL (telemetry + manual-evidence +
/// analytics families), the ChaosGraph tables, the `experiment_runs` view,
/// the static compliance-article seed, and the `schema_meta` version.
/// Idempotent — every statement is `IF NOT EXISTS` / `ADD COLUMN IF NOT
/// EXISTS` / upsert, and the version only advances. Used by both write
/// paths ([`Writer::migrate`] and [`duckdb_store::AnalyticsStore`]).
pub(crate) fn migrate(conn: &Connection) -> Result<(), duckdb::Error> {
    conn.execute_batch(schema::CREATE_TABLES)?;
    // ChaosGraph node/edge tables (part of schema v3): DDL lives in
    // `tumult_graph::sql`. `IF NOT EXISTS` / `ADD COLUMN IF NOT EXISTS`
    // make both the fresh-install DDL and the additive migration for
    // pre-existing databases.
    conn.execute_batch(tumult_graph::sql::CREATE_TABLES)?;
    conn.execute_batch(tumult_graph::sql::MIGRATE_EDGES_ADD_ATTRS)?;
    conn.execute_batch(schema::CREATE_VIEWS)?;
    // Static `ComplianceArticle` nodes from the citation registry:
    // deterministic and run-independent, seeded at migrate time.
    // Idempotent — nodes upsert on their primary key.
    for node in tumult_graph::compliance_article_nodes() {
        conn.execute(
            tumult_graph::sql::UPSERT_NODE,
            params![
                node.id,
                node.kind.as_str(),
                node.label,
                node.attrs.to_string()
            ],
        )?;
    }
    let mut stmt = conn.prepare("SELECT value FROM schema_meta WHERE key = 'version'")?;
    let version: Option<i64> = stmt.query_row(params![], |row| row.get(0)).ok();
    match version {
        None => {
            conn.execute(
                "INSERT INTO schema_meta (key, value) VALUES ('version', ?)",
                params![schema::CURRENT_SCHEMA_VERSION],
            )?;
        }
        Some(stored) if stored < schema::CURRENT_SCHEMA_VERSION => {
            conn.execute(
                "UPDATE schema_meta SET value = ? WHERE key = 'version'",
                params![schema::CURRENT_SCHEMA_VERSION],
            )?;
        }
        Some(_) => {}
    }
    Ok(())
}

#[cfg(feature = "duckdb")]
/// The write side of the store. Hold at most one per process (the ingest
/// daemon funnels every write through a channel onto a single `Writer`).
pub struct Writer {
    conn: Connection,
}
#[cfg(feature = "duckdb")]
impl Writer {
    fn migrate(&self) -> Result<(), StoreError> {
        migrate(&self.conn).map_err(StoreError::from)
    }

    /// Recorded schema version.
    ///
    /// # Errors
    /// Returns an error if the metadata table cannot be read.
    pub fn schema_version(&self) -> Result<i64, StoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT value FROM schema_meta WHERE key = 'version'")?;
        stmt.query_row(params![], |row| row.get(0))
            .map_err(StoreError::from)
    }

    /// Insert a batch of span rows in one transaction.
    ///
    /// # Errors
    /// Returns an error if the batch fails to insert (the transaction is rolled back).
    pub fn insert_spans(&self, rows: &[SpanRow]) -> Result<(), StoreError> {
        with_tx(&self.conn, || {
            let mut stmt = self.conn.prepare(
                "INSERT INTO spans VALUES (
                    ?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,
                    CAST(json(?) AS MAP(VARCHAR,VARCHAR)),
                    CAST(json(?) AS MAP(VARCHAR,VARCHAR)),
                    CAST(? AS JSON))",
            )?;
            for r in rows {
                // An empty events string is not valid JSON; normalize to `[]`.
                let events = if r.events.is_empty() {
                    "[]"
                } else {
                    r.events.as_str()
                };
                stmt.execute(params![
                    r.ts_ns,
                    r.trace_id,
                    r.span_id,
                    r.parent_span_id,
                    r.span_name,
                    r.span_kind,
                    r.duration_ns,
                    r.status_code,
                    r.status_message,
                    r.service_name,
                    r.service_version,
                    r.experiment_id,
                    r.experiment_name,
                    r.outcome_status,
                    r.fault_type,
                    r.fault_subtype,
                    r.fault_severity,
                    r.blast_radius,
                    r.target_system,
                    r.target_technology,
                    r.target_environment,
                    r.plugin_name,
                    r.hypothesis_met,
                    r.recovery_time_s,
                    attrs_json(&r.span_attrs)?,
                    attrs_json(&r.resource_attrs)?,
                    events,
                ])?;
            }
            Ok(())
        })
    }

    /// Insert a batch of log rows in one transaction.
    ///
    /// # Errors
    /// Returns an error if the batch fails to insert (the transaction is rolled back).
    pub fn insert_logs(&self, rows: &[LogRow]) -> Result<(), StoreError> {
        with_tx(&self.conn, || {
            let mut stmt = self.conn.prepare(
                "INSERT INTO logs VALUES (?,?,?,?,?,?,
                    CAST(json(?) AS MAP(VARCHAR,VARCHAR)),
                    CAST(json(?) AS MAP(VARCHAR,VARCHAR)))",
            )?;
            for r in rows {
                stmt.execute(params![
                    r.ts_ns,
                    r.severity_text,
                    r.body,
                    r.trace_id,
                    r.span_id,
                    r.service_name,
                    attrs_json(&r.log_attrs)?,
                    attrs_json(&r.resource_attrs)?,
                ])?;
            }
            Ok(())
        })
    }

    /// Insert a batch of sum data points in one transaction.
    ///
    /// # Errors
    /// Returns an error if the batch fails to insert (the transaction is rolled back).
    pub fn insert_metric_sums(&self, rows: &[MetricSumRow]) -> Result<(), StoreError> {
        with_tx(&self.conn, || {
            let mut stmt = self.conn.prepare(
                "INSERT INTO metric_sums VALUES (?,?,?,?,?,?,
                    CAST(json(?) AS MAP(VARCHAR,VARCHAR)),
                    CAST(json(?) AS MAP(VARCHAR,VARCHAR)))",
            )?;
            for r in rows {
                stmt.execute(params![
                    r.ts_ns,
                    r.metric_name,
                    r.value,
                    r.experiment_name,
                    r.outcome_status,
                    r.plugin_name,
                    attrs_json(&r.attrs)?,
                    attrs_json(&r.resource_attrs)?,
                ])?;
            }
            Ok(())
        })
    }

    /// Insert a batch of gauge data points in one transaction.
    ///
    /// # Errors
    /// Returns an error if the batch fails to insert (the transaction is rolled back).
    pub fn insert_metric_gauges(&self, rows: &[MetricGaugeRow]) -> Result<(), StoreError> {
        with_tx(&self.conn, || {
            let mut stmt = self.conn.prepare(
                "INSERT INTO metric_gauges VALUES (?,?,?,?,?,?,
                    CAST(json(?) AS MAP(VARCHAR,VARCHAR)),
                    CAST(json(?) AS MAP(VARCHAR,VARCHAR)))",
            )?;
            for r in rows {
                stmt.execute(params![
                    r.ts_ns,
                    r.metric_name,
                    r.value,
                    r.experiment_name,
                    r.outcome_status,
                    r.plugin_name,
                    attrs_json(&r.attrs)?,
                    attrs_json(&r.resource_attrs)?,
                ])?;
            }
            Ok(())
        })
    }

    /// Insert a batch of histogram data points in one transaction.
    ///
    /// # Errors
    /// Returns an error if the batch fails to insert (the transaction is rolled back).
    pub fn insert_metric_histograms(&self, rows: &[MetricHistogramRow]) -> Result<(), StoreError> {
        with_tx(&self.conn, || {
            let mut stmt = self.conn.prepare(
                "INSERT INTO metric_histograms VALUES (?,?,?,?,?,?,
                    CAST(? AS BIGINT[]), CAST(? AS DOUBLE[]),
                    CAST(json(?) AS MAP(VARCHAR,VARCHAR)),
                    CAST(json(?) AS MAP(VARCHAR,VARCHAR)),
                    ?,?,?)",
            )?;
            for r in rows {
                stmt.execute(params![
                    r.ts_ns,
                    r.metric_name,
                    r.count,
                    r.sum,
                    r.min,
                    r.max,
                    serde_json::to_string(&r.bucket_counts)?,
                    serde_json::to_string(&r.explicit_bounds)?,
                    attrs_json(&r.attrs)?,
                    attrs_json(&r.resource_attrs)?,
                    r.experiment_name,
                    r.outcome_status,
                    r.plugin_name,
                ])?;
            }
            Ok(())
        })
    }

    /// Record a manual import batch in `import_batches`.
    ///
    /// # Errors
    /// Returns an error if the row fails to insert.
    pub fn record_import_batch(&self, batch: &ImportBatch) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO import_batches VALUES (?,?,?,?,?)",
            params![
                batch.id,
                batch.source,
                batch.imported_at_ns,
                batch.rows,
                batch.label
            ],
        )?;
        Ok(())
    }

    /// Raw parameterized statement returning affected-row count — crate-internal
    /// escape hatch (lake retention deletes, lake tests).
    pub(crate) fn execute(&self, sql: &str, p: impl duckdb::Params) -> Result<usize, StoreError> {
        Ok(self.conn.execute(sql, p)?)
    }
}

#[cfg(feature = "duckdb")]
/// The read side of the store (read-only `DuckDB` connection).
pub struct Reader {
    conn: Connection,
}

#[cfg(feature = "duckdb")]
impl Reader {
    /// The experiment rollup view: one row per `resilience.experiment` span.
    ///
    /// # Errors
    /// Returns an error if the view cannot be queried.
    pub fn experiment_runs(&self) -> Result<Vec<ExperimentRun>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT experiment_id, experiment_name, started_at_ns, ended_at_ns,
                    duration_ns, outcome_status, hypothesis_met
             FROM experiment_runs ORDER BY started_at_ns",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ExperimentRun {
                experiment_id: r.get(0)?,
                experiment_name: r.get(1)?,
                started_at_ns: r.get(2)?,
                ended_at_ns: r.get(3)?,
                duration_ns: r.get(4)?,
                outcome_status: r.get(5)?,
                hypothesis_met: r.get(6)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Run a read-only SQL query and return each row as a JSON object
    /// (`{column: value}`). Values keep their JSON types (numbers, booleans,
    /// strings, null). Intended for the semantic-metrics layer and reports.
    ///
    /// # Errors
    /// Returns an error if the query fails to prepare or execute.
    pub fn query_json_rows(&self, sql: &str) -> Result<Vec<serde_json::Value>, StoreError> {
        let wrapped = format!("SELECT row_to_json(t) AS j FROM ({sql}) AS t");
        let mut stmt = self.conn.prepare(&wrapped)?;
        let rows = stmt.query_map([], |r| r.get::<usize, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }

    /// Raw batch execution on the read connection — crate-internal, used by
    /// the lake exporter for `COPY … TO … (FORMAT PARQUET)` (reads the store,
    /// writes only parquet files, so it is valid on a read-only connection).
    pub(crate) fn execute_batch(&self, sql: &str) -> Result<(), StoreError> {
        Ok(self.conn.execute_batch(sql)?)
    }
}

#[cfg(all(test, feature = "duckdb"))]
mod tests {
    use super::*;

    fn sample_span(experiment_id: &str) -> SpanRow {
        SpanRow {
            ts_ns: 1_774_980_000_000_000_000,
            trace_id: "abc123".into(),
            span_id: "span-1".into(),
            parent_span_id: None,
            span_name: "resilience.experiment".into(),
            span_kind: "Internal".into(),
            duration_ns: 300_000_000_000,
            status_code: "Ok".into(),
            status_message: String::new(),
            service_name: "tumult".into(),
            service_version: Some("2.18.0".into()),
            experiment_id: Some(experiment_id.into()),
            experiment_name: Some("pg-failover".into()),
            outcome_status: Some("completed".into()),
            fault_type: Some("termination".into()),
            fault_subtype: Some("process-kill".into()),
            fault_severity: Some("major".into()),
            blast_radius: Some("single-instance".into()),
            target_system: Some("database".into()),
            target_technology: Some("postgresql".into()),
            target_environment: Some("staging".into()),
            plugin_name: Some("tumult-ssh".into()),
            hypothesis_met: Some(true),
            recovery_time_s: Some(12.5),
            span_attrs: vec![(
                "resilience.baseline.probe.query_latency.mean".into(),
                "0.042".into(),
            )],
            resource_attrs: vec![("service.namespace".into(), "chaos".into())],
            events: "[]".into(),
        }
    }

    #[test]
    fn open_creates_schema_and_roundtrips_span() {
        let d = tempfile::TempDir::new().unwrap();
        let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();

        let writer = store.writer().unwrap();
        assert_eq!(writer.schema_version().unwrap(), CURRENT_SCHEMA_VERSION);
        writer.insert_spans(&[sample_span("exp-1")]).unwrap();

        let reader = store.read_only().unwrap();
        let runs = reader.experiment_runs().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].experiment_id.as_deref(), Some("exp-1"));
        assert_eq!(runs[0].outcome_status.as_deref(), Some("completed"));
        assert_eq!(runs[0].duration_ns, Some(300_000_000_000));
    }

    #[test]
    fn map_and_json_columns_roundtrip() {
        let d = tempfile::TempDir::new().unwrap();
        let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
        store
            .writer()
            .unwrap()
            .insert_spans(&[sample_span("exp-1")])
            .unwrap();

        let reader = store.read_only().unwrap();
        let rows = reader
            .query_json_rows(
                "SELECT span_attrs['resilience.baseline.probe.query_latency.mean'] AS probe_mean,
                        resource_attrs['service.namespace'] AS ns,
                        fault_type
                 FROM spans",
            )
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["probe_mean"], serde_json::json!("0.042"));
        assert_eq!(rows[0]["ns"], serde_json::json!("chaos"));
        assert_eq!(rows[0]["fault_type"], serde_json::json!("termination"));
    }

    #[test]
    fn histogram_arrays_roundtrip() {
        let d = tempfile::TempDir::new().unwrap();
        let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
        store
            .writer()
            .unwrap()
            .insert_metric_histograms(&[MetricHistogramRow {
                ts_ns: 1,
                metric_name: "tumult.experiment.duration".into(),
                count: 7,
                sum: 42.0,
                min: Some(1.0),
                max: Some(9.0),
                bucket_counts: vec![1, 2, 4],
                explicit_bounds: vec![5.0, 10.0],
                experiment_name: Some("exp".into()),
                outcome_status: Some("success".into()),
                plugin_name: Some("process".into()),
                attrs: vec![],
                resource_attrs: vec![],
            }])
            .unwrap();

        let reader = store.read_only().unwrap();
        let rows = reader
            .query_json_rows("SELECT count, bucket_counts, explicit_bounds FROM metric_histograms")
            .unwrap();
        assert_eq!(rows[0]["count"], serde_json::json!(7));
        assert_eq!(rows[0]["bucket_counts"], serde_json::json!([1, 2, 4]));
        assert_eq!(rows[0]["explicit_bounds"], serde_json::json!([5.0, 10.0]));
        let dims = reader
            .query_json_rows(
                "SELECT experiment_name, outcome_status, plugin_name FROM metric_histograms",
            )
            .unwrap();
        assert_eq!(dims[0]["experiment_name"], serde_json::json!("exp"));
        assert_eq!(dims[0]["plugin_name"], serde_json::json!("process"));
    }

    #[test]
    fn import_batch_is_recorded() {
        let d = tempfile::TempDir::new().unwrap();
        let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
        let writer = store.writer().unwrap();
        writer
            .record_import_batch(&ImportBatch {
                id: "batch-1".into(),
                source: "journal.json".into(),
                imported_at_ns: 1,
                rows: 3,
                label: Some("manual".into()),
            })
            .unwrap();
        let reader = store.read_only().unwrap();
        let rows = reader
            .query_json_rows("SELECT source, rows FROM import_batches")
            .unwrap();
        assert_eq!(rows[0]["rows"], serde_json::json!(3));
    }

    /// `StoreLocked` only triggers CROSS-process: duckdb-rs caches one
    /// in-process database instance per path, so a second `writer()` in the
    /// same process opens another connection to the same instance instead of
    /// hitting the file lock. The ingest daemon still keeps a single Writer
    /// by construction (one channel, one task).
    #[test]
    fn second_writer_in_same_process_shares_the_instance() {
        let d = tempfile::TempDir::new().unwrap();
        let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
        let w1 = store.writer().unwrap();
        w1.insert_spans(&[sample_span("exp-1")]).unwrap();
        let w2 = store.writer().unwrap();
        w2.insert_spans(&[sample_span("exp-2")]).unwrap();
        let reader = store.read_only().unwrap();
        assert_eq!(reader.experiment_runs().unwrap().len(), 2);
    }

    #[test]
    fn experiment_runs_resolves_outcome_from_completed_log() {
        // tumult leaves span.outcome_status NULL; the outcome lives on the
        // `experiment.completed` log record's capitalised `status` attr.
        let d = tempfile::TempDir::new().unwrap();
        let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
        let writer = store.writer().unwrap();
        let mut span = sample_span("exp-log");
        span.outcome_status = None;
        writer.insert_spans(&[span]).unwrap();
        writer
            .insert_logs(&[LogRow {
                ts_ns: 1_774_980_300_000_000_000,
                severity_text: "INFO".into(),
                body: "experiment.completed".into(),
                trace_id: Some("abc123".into()),
                span_id: None,
                service_name: "tumult".into(),
                log_attrs: vec![
                    ("experiment_id".into(), "exp-log".into()),
                    ("status".into(), "Deviated".into()),
                ],
                resource_attrs: vec![],
            }])
            .unwrap();
        let reader = store.read_only().unwrap();
        let runs = reader.experiment_runs().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].outcome_status.as_deref(), Some("Deviated"));
        // The span's own outcome still wins when present.
        let writer = store.writer().unwrap();
        writer.insert_spans(&[sample_span("exp-span")]).unwrap();
        // Fresh reader: read-only connections pin their snapshot at open.
        let reader2 = store.read_only().unwrap();
        let rows = reader2
            .query_json_rows(
                "SELECT experiment_id, outcome_status FROM experiment_runs ORDER BY experiment_id",
            )
            .unwrap();
        assert_eq!(rows[1]["outcome_status"], serde_json::json!("completed"));
    }

    #[test]
    fn read_only_reader_coexists_with_open_writer() {
        let d = tempfile::TempDir::new().unwrap();
        let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
        let writer = store.writer().unwrap();
        writer.insert_spans(&[sample_span("exp-1")]).unwrap();

        let reader = store.read_only().unwrap();
        assert_eq!(reader.experiment_runs().unwrap().len(), 1);
        drop(writer);
    }

    /// Schema v3: a fresh open creates the unified analytics family
    /// (journal detail, agentic, autopilot, ChaosGraph) alongside the
    /// telemetry tables, at the current version, with the static
    /// compliance-article nodes seeded.
    #[test]
    fn v3_open_creates_unified_analytics_family() {
        let d = tempfile::TempDir::new().unwrap();
        let store = Store::open(&d.path().join("lake.duckdb")).unwrap();
        let writer = store.writer().unwrap();
        assert_eq!(writer.schema_version().unwrap(), 3);

        let reader = store.read_only().unwrap();
        for table in [
            "experiments",
            "activity_results",
            "load_results",
            "agentic_runs",
            "agentic_contract_outcomes",
            "agentic_fault_applications",
            "agentic_replay_outcomes",
            "autopilot_decisions",
            "autopilot_events",
            "autopilot_change_events",
            "graph_nodes",
            "graph_edges",
        ] {
            let rows = reader
                .query_json_rows(&format!(
                    "SELECT count(*) AS c FROM information_schema.tables \
                     WHERE table_name = '{table}'"
                ))
                .unwrap();
            assert_eq!(rows[0]["c"], serde_json::json!(1), "missing {table}");
        }

        let articles = reader
            .query_json_rows(
                "SELECT count(*) AS c FROM graph_nodes WHERE kind = 'compliance_article'",
            )
            .unwrap();
        assert_eq!(
            articles[0]["c"],
            serde_json::json!(tumult_graph::compliance_article_nodes().len() as u64)
        );
        // The v3 edges attrs column exists.
        reader
            .query_json_rows("SELECT attrs FROM graph_edges LIMIT 0")
            .unwrap();
    }

    /// A v2-shaped store (telemetry + manual tables only, version 2) gains
    /// the analytics family on open, keeps its data, and advances to v3.
    #[test]
    fn v2_store_migrates_forward_without_data_loss() {
        let d = tempfile::TempDir::new().unwrap();
        let db_path = d.path().join("lake.duckdb");

        // Seed a v2-shaped store: current v2 DDL minus the v3 family, one
        // span row, version recorded as 2.
        {
            let conn = Connection::open(&db_path).unwrap();
            let v2_ddl = schema::CREATE_TABLES
                .split("-- v3: the tumult-analytics family")
                .next()
                .unwrap();
            conn.execute_batch(v2_ddl).unwrap();
            conn.execute(
                "INSERT INTO schema_meta (key, value) VALUES ('version', 2)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO spans VALUES (
                    1, 't', 's', NULL, 'resilience.experiment', 'Internal', 1,
                    'Ok', '', 'tumult', NULL, 'legacy-exp', 'legacy', 'completed',
                    NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL,
                    CAST('{}' AS MAP(VARCHAR,VARCHAR)),
                    CAST('{}' AS MAP(VARCHAR,VARCHAR)), '[]')",
                [],
            )
            .unwrap();
        }

        let store = Store::open(&db_path).unwrap();
        let writer = store.writer().unwrap();
        assert_eq!(writer.schema_version().unwrap(), 3);
        let reader = store.read_only().unwrap();
        // Prior data preserved; analytics family now queryable.
        assert_eq!(reader.experiment_runs().unwrap().len(), 1);
        reader
            .query_json_rows("SELECT count(*) AS c FROM experiments")
            .unwrap();
        let articles = reader
            .query_json_rows(
                "SELECT count(*) AS c FROM graph_nodes WHERE kind = 'compliance_article'",
            )
            .unwrap();
        assert!(articles[0]["c"].as_u64().unwrap() > 0);
    }
}
