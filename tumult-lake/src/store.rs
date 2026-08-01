//! The store handle ([`Store`]), connection open helpers, the shared schema
//! migration, and crate-internal query utilities (split out of the crate
//! root).

use std::path::{Path, PathBuf};
use std::time::Duration;

use duckdb::{params, AccessMode, Config, Connection};

use crate::error::StoreError;
use crate::reader::Reader;
use crate::schema;
use crate::writer::Writer;

/// Total attempts an open makes before reporting the store as locked.
const OPEN_ATTEMPTS: u32 = 3;
/// Backoff between open attempts while another process finishes a write.
const OPEN_BACKOFF: Duration = Duration::from_millis(50);

/// Whether a `DuckDB` error is the file-lock conflict raised when another
/// process already holds the store open.
fn is_lock_conflict(err: &duckdb::Error) -> bool {
    matches!(
        err,
        duckdb::Error::DuckDBFailure(_, Some(msg))
            if msg.contains("Could not set lock") || msg.contains("Conflicting lock")
    )
}

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

/// Serialise attribute pairs as a JSON object for binding into a
/// `MAP(VARCHAR, VARCHAR)` column (`CAST(json(?) AS MAP(VARCHAR,VARCHAR))`).
pub(crate) fn attrs_json(attrs: &[(String, String)]) -> Result<String, StoreError> {
    let map: serde_json::Map<String, serde_json::Value> = attrs
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
        .collect();
    Ok(serde_json::Value::Object(map).to_string())
}

/// A handle to the store file. Cheap to construct; connections are opened
/// per role via [`Store::writer`] and [`Store::read_only`].
pub struct Store {
    path: PathBuf,
}

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

/// Run `f` inside a transaction on `conn` (single-writer, so a plain
/// `BEGIN`/`COMMIT` batch is enough).
pub(crate) fn with_tx<T, E: From<StoreError>>(
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

/// Shared schema migration: the full v3 DDL (telemetry + manual-evidence +
/// analytics families), the ChaosGraph tables, the `experiment_runs` view,
/// the static compliance-article seed, and the `schema_meta` version.
/// Idempotent — every statement is `IF NOT EXISTS` / `ADD COLUMN IF NOT
/// EXISTS` / upsert, and the version only advances. Used by both write
/// paths ([`Writer::migrate`] and [`crate::duckdb_store::AnalyticsStore`]).
pub(crate) fn migrate(conn: &Connection) -> Result<(), duckdb::Error> {
    conn.execute_batch(schema::CREATE_TABLES)?;
    // v4 → v5: rebuild the run tables index-free (crash-robustness — see
    // schema.rs). Conditional on the stored version actually being 4: older
    // databases have no run tables to rebuild, and fresh ones got the v5
    // shape from CREATE_TABLES above.
    let stored: Option<i64> = {
        let mut stmt = conn.prepare("SELECT value FROM schema_meta WHERE key = 'version'")?;
        stmt.query_row(params![], |row| row.get(0)).ok()
    };
    if stored == Some(4) {
        conn.execute_batch(schema::MIGRATE_V5_RUN_TABLES_INDEX_FREE)?;
    }
    // v2–v7 → v8: rebuild `manual_experiments` index-free (same crash-
    // robustness rule as v5 — see schema.rs). Conditional on the stored
    // version actually being 2..=7: v0/v1 databases have no manual tables to
    // rebuild, and fresh ones got the v8 shape from CREATE_TABLES above.
    if stored.is_some_and(|v| (2..8).contains(&v)) {
        conn.execute_batch(schema::MIGRATE_V8_MANUAL_EXPERIMENTS_INDEX_FREE)?;
    }
    // < v6: seed the `legacy` backfill identity so pre-auth free-text
    // `entered_by` / `reviewed_by` values on manual-experiment rows
    // semantically belong to a real (but un-loginable: password_hash '!',
    // disabled) user and the audit hash chain is never rewritten. Guarded
    // strictly on the stored version — idempotent by version, like the v5
    // rebuild; fresh databases (stored = None) have no legacy rows and get
    // no legacy user.
    if stored.is_some_and(|v| v < 6) {
        conn.execute(
            "INSERT INTO users VALUES ('legacy', 'legacy', '!', 'viewer', false, true, 0)",
            [],
        )?;
    }
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

/// Shared JSON-rows query: each row as a JSON object (`{column: value}`)
/// via DuckDB's `row_to_json`. Backs [`Reader::query_json_rows`] (public)
/// and [`Writer::query_json_rows`] (crate-internal retention checks).
pub(crate) fn query_json_rows(
    conn: &Connection,
    sql: &str,
) -> Result<Vec<serde_json::Value>, StoreError> {
    let wrapped = format!("SELECT row_to_json(t) AS j FROM ({sql}) AS t");
    let mut stmt = conn.prepare(&wrapped)?;
    let rows = stmt.query_map([], |r| r.get::<usize, String>(0))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(serde_json::from_str(&row?)?);
    }
    Ok(out)
}
