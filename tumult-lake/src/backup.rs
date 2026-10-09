//! Complete privileged installation backups, including credentials.
//!
//! Uses DuckDB's database copy so all tables, indexes, sequences and views are
//! included, including future schema additions. The daemon must be stopped for
//! CLI backup/restore: native DuckDB cannot share a writer across processes.
//! Portable customer exports use [`crate::lake`] instead.
use crate::file_integrity::{checksum, entry_exists};
use crate::{Store, StoreError};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

#[derive(Serialize, Deserialize)]
struct Manifest {
    format_version: u32,
    sha256: String,
    schema_version: i64,
    credential_policy: String,
}
fn schema_version(reader: &crate::Reader) -> Result<i64, StoreError> {
    reader
        .query_json_rows("SELECT value AS v FROM schema_meta WHERE key = 'version'")?
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
///
/// # Errors
/// Refuses an existing destination or unavailable/locked source. A failed
/// backup has no completed manifest and must not be used for restore.
pub fn create(source: &Path, directory: &Path) -> Result<(), StoreError> {
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
    conn.execute_batch(&format!("ATTACH '{}' AS source_store (READ_ONLY); ATTACH '{}' AS backup_store; COPY FROM DATABASE source_store TO backup_store; CHECKPOINT backup_store; DETACH backup_store; DETACH source_store;", quote(source), quote(&snapshot)))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&snapshot, std::fs::Permissions::from_mode(0o600))?;
    }
    File::open(&snapshot)?.sync_all()?;
    let manifest = Manifest {
        format_version: 1,
        sha256: checksum(&snapshot)?,
        schema_version,
        credential_policy: "complete-installation-including-credentials".into(),
    };
    let mut file = private_file(&directory.join("manifest.json"))?;
    file.write_all(&serde_json::to_vec_pretty(&manifest)?)?;
    file.sync_all()?;
    #[cfg(unix)]
    File::open(directory)?.sync_all()?;
    Ok(())
}

/// Restore a verified complete backup to a new database path. Does not replace
/// an existing database or WAL; restart services only after selecting the new
/// path and reviewing restored schedules/webhook credentials.
///
/// # Errors
/// Rejects incomplete/tampered backups, unsupported versions and existing paths.
pub fn restore(directory: &Path, destination: &Path) -> Result<(), StoreError> {
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
    result
}
