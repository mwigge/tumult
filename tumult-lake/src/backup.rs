//! Complete privileged installation backups, including credentials.
//!
//! Uses DuckDB's database copy so all tables, indexes, sequences and views are
//! included, including future schema additions. The daemon must be stopped for
//! CLI backup/restore: native DuckDB cannot share a writer across processes.
//! Portable customer exports use [`crate::lake`] instead.
//!
//! The files attached to manual records travel with the database: each is
//! copied, hash-checked, to `attachments/<first 2 hex>/<sha256>` in the
//! backup and named in the `attachments` map of the manifest. A restore
//! brings them back, hash-checked, and never writes over a file that is
//! there. A backup made before files were attached has no such map and
//! restores with no files.
use crate::file_integrity::{checksum, entry_exists};
use crate::manual::files::{bring_back, carry, FILE_HASHES_SQL};
use crate::sql;
use crate::{attachments_dir, Store, StoreError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

#[derive(Serialize, Deserialize)]
struct Manifest {
    format_version: u32,
    sha256: String,
    schema_version: i64,
    credential_policy: String,
    /// The files attached to manual records: each SHA-256 with its path in
    /// the backup. Left out when there are none, as in a backup made
    /// before files were attached.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    attachments: BTreeMap<String, String>,
}

/// What a backup holds of the files attached to manual records.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackupReport {
    /// The files the backup holds.
    pub attachments: usize,
    /// The hashes of files whose stored file is missing or does not hash
    /// to its name, so the backup does not hold them.
    pub missing_attachments: Vec<String>,
}
fn schema_version(reader: &crate::Reader) -> Result<i64, StoreError> {
    reader
        .query_json_rows(sql::store::SCHEMA_VERSION_AS_V)?
        .first()
        .and_then(|row| row["v"].as_i64())
        .ok_or_else(|| StoreError::Internal("missing schema version".into()))
}
fn quote(path: &Path) -> String {
    path.to_string_lossy().replace('\'', "''")
}
fn private_file(path: &Path) -> Result<File, StoreError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}

/// Create a complete operational backup in a new directory. It contains
/// password hashes, tokens and webhook secrets; protect it as the live DB.
/// The files attached to manual records are copied with it; a stored file
/// that is missing or changed is logged and named in the report.
///
/// # Errors
/// Refuses an existing destination or unavailable/locked source. A failed
/// backup has no completed manifest and must not be used for restore.
pub fn create(source: &Path, directory: &Path) -> Result<BackupReport, StoreError> {
    if !source.is_file() {
        return Err(StoreError::Internal("backup source does not exist".into()));
    }
    // Verify and obtain the source lock before creating the destination.
    let source_reader = Store::at(source).read_only()?;
    let schema_version = schema_version(&source_reader)?;
    std::fs::create_dir(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
    }
    let snapshot = directory.join("store.duckdb");
    let conn = duckdb::Connection::open_in_memory()?;
    conn.execute_batch(&snapshot_sql(&quote(source), &quote(&snapshot)))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&snapshot, std::fs::Permissions::from_mode(0o600))?;
    }
    File::open(&snapshot)?.sync_all()?;
    // The source is held read-only, so no write can come between the copy
    // of the database and the list of its files.
    let hashes: Vec<String> = source_reader
        .query_json_rows(FILE_HASHES_SQL)?
        .iter()
        .filter_map(|row| row["h"].as_str().map(str::to_owned))
        .collect();
    let (attachments, missing) = carry(&attachments_dir(source), directory, &hashes)?;
    let manifest = Manifest {
        format_version: 1,
        sha256: checksum(&snapshot)?,
        schema_version,
        credential_policy: "complete-installation-including-credentials".into(),
        attachments,
    };
    let mut file = private_file(&directory.join("manifest.json"))?;
    file.write_all(&serde_json::to_vec_pretty(&manifest)?)?;
    file.sync_all()?;
    #[cfg(unix)]
    File::open(directory)?.sync_all()?;
    Ok(BackupReport {
        attachments: manifest.attachments.len(),
        missing_attachments: missing,
    })
}

/// Restore a verified complete backup to a new database path. Does not replace
/// an existing database or WAL; restart services only after selecting the new
/// path and reviewing restored schedules/webhook credentials. The files
/// attached to manual records are brought into the attachments folder next
/// to `destination` first, each hash-checked, and a file that is there
/// already is never written again. Returns the files copied.
///
/// # Errors
/// Rejects incomplete/tampered backups, unsupported versions and existing
/// paths, and a file of the backup that is missing or does not hash to its
/// name; then no database is restored.
pub fn restore(directory: &Path, destination: &Path) -> Result<usize, StoreError> {
    let manifest: Manifest =
        serde_json::from_slice(&std::fs::read(directory.join("manifest.json"))?)?;
    if manifest.format_version != 1
        || manifest.credential_policy != "complete-installation-including-credentials"
    {
        return Err(StoreError::Internal(
            "unsupported operational backup manifest".into(),
        ));
    }
    if entry_exists(destination)?
        || entry_exists(Path::new(&format!("{}.wal", destination.display())))?
    {
        return Err(StoreError::Internal(
            "restore destination or WAL already exists; choose a new store path".into(),
        ));
    }
    let source = directory.join("store.duckdb");
    if std::fs::symlink_metadata(&source)?.file_type().is_symlink() {
        return Err(StoreError::Internal(
            "backup database must not be a symlink".into(),
        ));
    }
    if checksum(&source)? != manifest.sha256 {
        return Err(StoreError::Internal("backup checksum mismatch".into()));
    }
    let reader = Store::at(&source).read_only()?;
    if schema_version(&reader)? != manifest.schema_version {
        return Err(StoreError::Internal(
            "backup schema version mismatch".into(),
        ));
    }
    let files = bring_back(
        directory,
        &manifest.attachments,
        &attachments_dir(destination),
    )?;
    let temp = destination.with_extension("restore.tmp");
    let mut out = private_file(&temp)?;
    let result = (|| {
        std::io::copy(&mut File::open(&source)?, &mut out)?;
        out.sync_all()?;
        // Verify copied bytes as well, catching a concurrent change of source.
        if checksum(&temp)? != manifest.sha256 {
            return Err(StoreError::Internal("backup changed during restore".into()));
        }
        if entry_exists(Path::new(&format!("{}.wal", destination.display())))? {
            return Err(StoreError::Internal(
                "restore WAL appeared during copy".into(),
            ));
        }
        // hard_link publishes atomically without replacing a concurrently-created file.
        std::fs::hard_link(&temp, destination)?;
        #[cfg(unix)]
        File::open(
            destination
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new(".")),
        )?
        .sync_all()?;
        Ok(())
    })();
    if let Err(cleanup) = std::fs::remove_file(&temp) {
        tracing::error!(error = %cleanup, "failed to remove restore temporary file");
    }
    result.map(|()| files)
}

/// Copy the database at `source` into a new one at `snapshot`, both paths
/// quoted.
fn snapshot_sql(source: &str, snapshot: &str) -> String {
    format!("ATTACH '{source}' AS source_store (READ_ONLY); ATTACH '{snapshot}' AS backup_store; COPY FROM DATABASE source_store TO backup_store; CHECKPOINT backup_store; DETACH backup_store; DETACH source_store;")
}

/// The statements of a backup, for the SQL corpus.
#[cfg(test)]
pub(crate) fn sql_corpus() -> Vec<tumult_test_utils::sql_corpus::Statement> {
    vec![
        tumult_test_utils::sql_statement!(snapshot_sql("lake.duckdb", "backup/store.duckdb"))
            .unprepared("it attaches two stores and copies one into the other")
            .unlinted("sqlfluff has no grammar for ATTACH, COPY FROM DATABASE and DETACH"),
    ]
}
