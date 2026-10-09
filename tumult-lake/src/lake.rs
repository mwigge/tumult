//! Portable, manifest-selected Parquet snapshots.
//!
//! Each table snapshot is content addressed and immutable. `_meta.json` is
//! atomically published only after every table has been written, so consumers
//! must read its `snapshots` map, never glob historical files. Late arrivals,
//! tied event timestamps and mutable rows are covered by complete snapshots.
//! Credentials and operational recovery state are deliberately excluded; use
//! [`crate::backup`] for a complete privileged installation backup.
//! Hot retention is disabled until historical queries can read the archive.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::file_integrity::checksum;
use crate::{Reader, StoreError, Writer};
use serde::{Deserialize, Serialize};

#[doc(hidden)]
pub const AUDIT_TABLE: &str = "manual_experiment_audit";
#[doc(hidden)]
pub const MANUAL_TABLE: &str = "manual_experiments";

// An explicit portable-data contract. Adding a platform table requires a
// classification decision; full operational backups discover all schema data.
const TABLES: &[&str] = &[
    "spans",
    "logs",
    "metric_sums",
    "metric_gauges",
    "metric_histograms",
    "manual_experiment_audit",
    "manual_experiments",
    "evidence_attachments",
    "import_batches",
    "experiments",
    "activity_results",
    "load_results",
    "autopilot_decisions",
    "autopilot_events",
    "autopilot_change_events",
    "graph_nodes",
    "graph_edges",
    "agentic_runs",
    "agentic_contract_outcomes",
    "agentic_fault_applications",
    "agentic_replay_outcomes",
    "runs",
    "run_registry",
    "run_audit",
    "run_schedules",
    "approval_requests",
    "approval_decisions",
    "webhook_cursors",
    "webhook_dead_letters",
];
const EXCLUDED: &[&str] = &[
    "users",
    "sessions",
    "tokens",
    "user_env_scopes",
    "webhooks",
    "run_recovery_plans",
    "run_execution_pins",
    "schema_meta",
];

#[doc(hidden)]
pub fn fingerprint_sql(table: &str) -> String {
    format!(
        "SELECT sha256( \
          (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') \
           FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position \
                 FROM information_schema.columns WHERE table_schema = 'main' AND table_name = '{table}') c) \
          || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp \
         FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM {table} t)"
    )
}

#[derive(Clone, Debug)]
pub struct LakeConfig {
    pub dir: PathBuf,
    /// Only zero is supported until hot+cold query views are implemented.
    pub retention_days: u64,
}
impl LakeConfig {
    #[must_use]
    pub fn new(dir: PathBuf, retention_days: u64) -> Self {
        Self {
            dir,
            retention_days,
        }
    }
    #[must_use]
    pub fn from_env(db_path: &Path) -> Self {
        let dir = std::env::var_os("KRONIKA_LAKE_DIR").map_or_else(
            || {
                db_path
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join("lake")
            },
            PathBuf::from,
        );
        let retention_days = std::env::var("KRONIKA_RETENTION_DAYS")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0);
        Self::new(dir, retention_days)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct TableExport {
    pub name: String,
    /// Complete snapshot row count when changed; zero for an unchanged table.
    pub rows: u64,
    /// Compatibility field: snapshot publication time, NOT an event-time cursor.
    pub watermark_ns: i64,
    pub files: Vec<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct ExportReport {
    pub ran_at_ns: i64,
    pub lake_dir: String,
    pub retention_days: u64,
    pub tables: Vec<TableExport>,
}
#[derive(Clone, Debug, Serialize)]
pub struct LakeStatus {
    pub lake_dir: String,
    pub retention_days: u64,
    pub last_export_ns: Option<i64>,
    pub watermarks: BTreeMap<String, i64>,
    pub files: u64,
    pub bytes: u64,
}
#[derive(Default, Serialize, Deserialize)]
struct LakeMeta {
    #[serde(default)]
    format_version: u32,
    last_export_ns: Option<i64>,
    #[serde(default)]
    tables: BTreeMap<String, i64>,
    #[serde(default)]
    fingerprints: BTreeMap<String, String>,
    #[serde(default)]
    snapshots: BTreeMap<String, String>,
    #[serde(default)]
    file_checksums: BTreeMap<String, String>,
    #[serde(default)]
    excluded_tables: Vec<String>,
}
#[doc(hidden)]
pub fn meta_path(dir: &Path) -> PathBuf {
    dir.join("_meta.json")
}
fn read_meta(dir: &Path) -> Result<LakeMeta, StoreError> {
    match std::fs::read_to_string(meta_path(dir)) {
        Ok(raw) => Ok(serde_json::from_str(&raw)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(LakeMeta::default()),
        Err(e) => Err(e.into()),
    }
}
fn write_meta(dir: &Path, meta: &LakeMeta) -> Result<(), StoreError> {
    let temp = dir.join("_meta.json.tmp");
    match std::fs::remove_file(&temp) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    // Never follow a leftover or concurrently planted temporary symlink.
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp)?;
    file.write_all(&serde_json::to_vec_pretty(meta)?)?;
    file.sync_all()?;
    std::fs::rename(temp, meta_path(dir))?;
    #[cfg(unix)]
    File::open(dir)?.sync_all()?;
    Ok(())
}
#[doc(hidden)]
pub fn now_ns() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
}
fn sql_path(path: &Path) -> String {
    path.to_string_lossy().replace('\'', "''")
}

/// Export complete table versions and atomically commit their manifest.
/// Existing v1 archives are rejected: select a new directory and reconcile
/// any previously purged history separately. No old metadata is overwritten.
///
/// # Errors
/// Returns a filesystem/store error without advancing the manifest. Retries
/// reuse verified committed files and rebuild uncommitted or damaged files.
pub fn export(reader: &Reader, cfg: &LakeConfig) -> Result<ExportReport, StoreError> {
    std::fs::create_dir_all(&cfg.dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&cfg.dir, std::fs::Permissions::from_mode(0o700))?;
    }
    // The OS releases the lock on process exit, including crashes. Protects
    // publication across both scheduler/API calls and independent processes.
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(cfg.dir.join(".export.lock"))?;
    lock.lock()?;
    let previous = read_meta(&cfg.dir)?;
    if previous.format_version != 2
        && (previous.last_export_ns.is_some() || !previous.tables.is_empty())
    {
        return Err(StoreError::Internal("legacy lake archive: preserve its files and set KRONIKA_LAKE_DIR to a new empty directory for manifest v2; previously purged rows require separate reconciliation".into()));
    }
    if previous.format_version > 2 {
        return Err(StoreError::Internal(
            "unsupported lake manifest version".into(),
        ));
    }
    let mut meta = LakeMeta {
        format_version: 2,
        excluded_tables: EXCLUDED.iter().map(|s| (*s).into()).collect(),
        ..Default::default()
    };
    let run_ns = now_ns();
    let mut tables = Vec::new();
    // One explicit transaction ensures all tables belong to one database snapshot.
    reader.execute_batch("BEGIN TRANSACTION")?;
    let result = (|| {
        for table in TABLES {
            let fp = reader.query_json_rows(&fingerprint_sql(table))?[0]["fp"]
                .as_str()
                .ok_or_else(|| StoreError::Internal("missing table fingerprint".into()))?
                .to_owned();
            let rel = format!("snapshots/{table}/{fp}.parquet");
            let path = cfg.dir.join(&rel);
            // A content-derived filename alone does not establish file integrity.
            // Legacy/uncommitted files without a verified byte digest are rebuilt.
            let existing_checksum = path.is_file().then(|| checksum(&path)).transpose()?;
            let unchanged = previous.format_version == 2
                && previous.snapshots.get(*table) == Some(&rel)
                && existing_checksum.is_some()
                && previous.file_checksums.get(*table) == existing_checksum.as_ref();
            let rows = reader.query_json_rows(&format!("SELECT count(*) AS n FROM {table}"))?[0]
                ["n"]
                .as_u64()
                .ok_or_else(|| StoreError::Internal("missing table count".into()))?;
            if !unchanged {
                let parent = path.parent().expect("snapshot path has a parent");
                std::fs::create_dir_all(parent)?;
                let temp = path.with_extension("parquet.tmp");
                // A crashed export can leave an incomplete temporary file.
                match std::fs::remove_file(&temp) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
                reader.execute_batch(&format!(
                    "COPY (SELECT * FROM {table}) TO '{}' (FORMAT PARQUET)",
                    sql_path(&temp)
                ))?;
                File::open(&temp)?.sync_all()?;
                std::fs::rename(temp, &path)?;
                #[cfg(unix)]
                File::open(parent)?.sync_all()?;
            }
            let file_checksum = match existing_checksum {
                Some(digest) if unchanged => digest,
                _ => checksum(&path)?,
            };
            meta.file_checksums.insert((*table).into(), file_checksum);
            meta.snapshots.insert((*table).into(), rel.clone());
            meta.fingerprints.insert((*table).into(), fp);
            let published = if unchanged {
                previous.tables.get(*table).copied().unwrap_or(run_ns)
            } else {
                run_ns
            };
            meta.tables.insert((*table).into(), published);
            tables.push(TableExport {
                name: (*table).into(),
                rows: if unchanged { 0 } else { rows },
                watermark_ns: published,
                files: if unchanged { vec![] } else { vec![rel] },
            });
        }
        Ok::<_, StoreError>(())
    })();
    if let Err(error) = result {
        if let Err(rollback) = reader.execute_batch("ROLLBACK") {
            tracing::error!(error = %rollback, "failed to roll back export snapshot");
        }
        return Err(error);
    }
    reader.execute_batch("COMMIT")?;
    meta.last_export_ns = Some(run_ns);
    write_meta(&cfg.dir, &meta)?;
    Ok(ExportReport {
        ran_at_ns: run_ns,
        lake_dir: cfg.dir.display().to_string(),
        retention_days: cfg.retention_days,
        tables,
    })
}

/// Return the single committed version of a portable table. Never glob files.
///
/// # Errors
/// Rejects pre-manifest archives, invalid table names and missing snapshots.
pub fn committed_file(cfg: &LakeConfig, table: &str) -> Result<PathBuf, StoreError> {
    let meta = read_meta(&cfg.dir)?;
    if meta.format_version != 2 {
        return Err(StoreError::Internal(
            "archive requires a v2 snapshot export".into(),
        ));
    }
    let relative = meta
        .snapshots
        .get(table)
        .ok_or_else(|| StoreError::Internal("table is not in the portable export".into()))?;
    let fingerprint = meta
        .fingerprints
        .get(table)
        .ok_or_else(|| StoreError::Internal("missing fingerprint".into()))?;
    if !TABLES.contains(&table)
        || fingerprint.len() != 64
        || !fingerprint.bytes().all(|b| b.is_ascii_hexdigit())
        || relative != &format!("snapshots/{table}/{fingerprint}.parquet")
    {
        return Err(StoreError::Internal(
            "invalid snapshot manifest path".into(),
        ));
    }
    let path = cfg.dir.join(relative);
    if !path.is_file() {
        return Err(StoreError::Internal(
            "committed snapshot file is missing".into(),
        ));
    }
    let expected = meta.file_checksums.get(table).ok_or_else(|| {
        StoreError::Internal(
            "snapshot integrity metadata missing; export again before reading".into(),
        )
    })?;
    if checksum(&path)? != *expected {
        return Err(StoreError::Internal(
            "committed snapshot checksum mismatch; export again to repair from retained rows"
                .into(),
        ));
    }
    Ok(path)
}

/// Retention is unavailable until reports and queries can read cold snapshots.
///
/// # Errors
/// A nonzero policy fails closed; no rows are deleted.
pub fn enforce_retention(
    _writer: &Writer,
    cfg: &LakeConfig,
) -> Result<BTreeMap<String, u64>, StoreError> {
    if cfg.retention_days != 0 {
        return Err(StoreError::Internal("KRONIKA_RETENTION_DAYS must be 0: historical queries do not yet read archived snapshots; no data was deleted".into()));
    }
    Ok(BTreeMap::new())
}

/// Status of the committed archive, excluding historical and uncommitted files.
///
/// # Errors
/// Returns errors for unreadable manifests or missing committed files.
pub fn status(cfg: &LakeConfig) -> Result<LakeStatus, StoreError> {
    let meta = read_meta(&cfg.dir)?;
    let mut bytes = 0;
    for table in meta.snapshots.keys() {
        bytes += std::fs::metadata(committed_file(cfg, table)?)?.len();
    }
    Ok(LakeStatus {
        lake_dir: cfg.dir.display().to_string(),
        retention_days: cfg.retention_days,
        last_export_ns: meta.last_export_ns,
        watermarks: meta.tables,
        files: meta.snapshots.len() as u64,
        bytes,
    })
}
