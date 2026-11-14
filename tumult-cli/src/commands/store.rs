use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use tumult_otel::logging::{bounded, err};

use super::validate_path_no_symlink;

// ── Import command ──────────────────────────────────────────

/// # Errors
///
/// Returns an error if the directory is invalid, the parquet files are missing,
/// or the import operation fails.
#[must_use = "callers must handle import errors"]
pub fn cmd_import(parquet_dir: &Path) -> Result<()> {
    use tumult_lake::AnalyticsStore;

    validate_path_no_symlink(parquet_dir)?;

    if !parquet_dir.is_dir() {
        bail!("not a directory: {}", parquet_dir.display());
    }

    let exp_path = parquet_dir.join("experiments.parquet");
    let act_path = parquet_dir.join("activities.parquet");

    if !exp_path.exists() {
        bail!("experiments.parquet not found in {}", parquet_dir.display());
    }
    if !act_path.exists() {
        bail!("activities.parquet not found in {}", parquet_dir.display());
    }

    let db_path =
        AnalyticsStore::default_path().context("failed to determine analytics store path")?;
    let store = AnalyticsStore::open(&db_path)?;
    store.import_tables(&exp_path, &act_path)?;

    let stats = store.stats()?;
    crate::outln!("Imported from: {}", parquet_dir.display());
    crate::outln!(
        "Store now contains: {} experiments, {} activities",
        stats.experiment_count,
        stats.activity_count
    );
    Ok(())
}

// ── Store management commands ───────────────────────────────

/// # Errors
///
/// Returns an error if the store cannot be opened or the stats query fails.
#[must_use = "callers must handle store stats errors"]
pub fn cmd_store_stats() -> Result<()> {
    use tumult_lake::AnalyticsStore;

    let db_path =
        AnalyticsStore::default_path().context("failed to determine analytics store path")?;
    if !db_path.exists() {
        crate::outln!("No persistent store found at: {}", db_path.display());
        crate::outln!("Run an experiment to create it automatically.");
        return Ok(());
    }

    let store = AnalyticsStore::open(&db_path)?;
    let stats = store.stats()?;
    let version = store.schema_version()?;

    crate::outln!("Store: {}", db_path.display());
    crate::outln!("Schema version: {version}");
    crate::outln!("Experiments: {}", stats.experiment_count);
    crate::outln!("Activities: {}", stats.activity_count);

    if let Ok(size) = std::fs::metadata(&db_path) {
        // u64 → f64: file size in MB for display; precision loss is acceptable.
        #[allow(clippy::cast_precision_loss)]
        let mb = size.len() as f64 / (1024.0 * 1024.0);
        crate::outln!("File size: {mb:.2} MB");
    }

    Ok(())
}

/// Back up the complete installation, including credentials. Stop the daemon first.
///
/// # Errors
/// Refuses an existing destination or locked/unavailable source.
#[must_use = "callers must handle backup errors"]
pub fn cmd_store_backup(output_dir: &Path) -> Result<()> {
    let db_path = tumult_lake::AnalyticsStore::default_path()?;
    if !db_path.exists() {
        bail!("no persistent store found at: {}", db_path.display());
    }
    tumult_lake::backup::create(&db_path, output_dir)?;
    crate::outln!(
        "Complete operational backup: {} (includes credentials)",
        output_dir.display()
    );
    Ok(())
}

/// Restore a complete backup to a new database path.
///
/// # Errors
/// Rejects corrupted backups and existing destinations.
#[must_use = "callers must handle restore errors"]
pub fn cmd_store_restore(input: &Path, output: &Path) -> Result<()> {
    tumult_lake::backup::restore(input, output)?;
    crate::outln!("Restored database: {}", output.display());
    Ok(())
}

/// # Errors
///
/// Returns an error if the store cannot be opened or the purge operation fails.
#[must_use = "callers must handle purge errors"]
pub fn cmd_store_purge(older_than_days: u32) -> Result<()> {
    use tumult_lake::AnalyticsStore;

    let db_path =
        AnalyticsStore::default_path().context("failed to determine analytics store path")?;
    if !db_path.exists() {
        bail!("no persistent store found at: {}", db_path.display());
    }

    let store = AnalyticsStore::open(&db_path)?;
    let purged = store.purge_older_than_days(older_than_days)?;

    let stats = store.stats()?;
    if purged == 0 {
        crate::outln!("No experiments older than {older_than_days} days found");
    } else {
        crate::outln!("Purged {purged} experiment(s) older than {older_than_days} days");
    }
    crate::outln!(
        "Remaining: {} experiments, {} activities",
        stats.experiment_count,
        stats.activity_count
    );
    Ok(())
}

/// # Errors
///
/// Returns an error if the store path cannot be determined or the metadata
/// cannot be read.
#[must_use = "callers must handle store path errors"]
pub fn cmd_store_path() -> Result<()> {
    use tumult_lake::AnalyticsStore;

    let db_path =
        AnalyticsStore::default_path().context("failed to determine analytics store path")?;
    crate::outln!("{}", db_path.display());
    if db_path.exists() {
        if let Ok(size) = std::fs::metadata(&db_path) {
            // u64 → f64: file size in MB for display; precision loss is acceptable.
            #[allow(clippy::cast_precision_loss)]
            let mb = size.len() as f64 / (1024.0 * 1024.0);
            crate::outln!("Size: {mb:.2} MB");
        }
    } else {
        crate::outln!("(not yet created)");
    }
    Ok(())
}

// ── Import-legacy command ───────────────────────────────────

/// Merge pre-unification databases into the unified store.
///
/// Sources come from the explicit flags; with neither flag given, the legacy
/// defaults that exist are picked up (`TUMULT_ANALYTICS_PATH`,
/// `~/.tumult/analytics.duckdb`, `KRONIKA_DB`).
///
/// # Errors
///
/// Returns an error if no source is found, the target store cannot be
/// opened, or a merge fails.
#[must_use = "callers must handle import errors"]
pub fn cmd_store_import_legacy(
    analytics_db: Option<&Path>,
    kronika_db: Option<&Path>,
    store_path: Option<&Path>,
) -> Result<()> {
    use tumult_lake::AnalyticsStore;

    let mut sources: Vec<PathBuf> = Vec::new();
    if let Some(p) = analytics_db {
        sources.push(p.to_path_buf());
    }
    if let Some(p) = kronika_db {
        sources.push(p.to_path_buf());
    }
    if sources.is_empty() {
        for candidate in [
            std::env::var("TUMULT_ANALYTICS_PATH").ok(),
            std::env::var("HOME")
                .ok()
                .map(|h| format!("{h}/.tumult/analytics.duckdb")),
            std::env::var("KRONIKA_DB").ok(),
        ]
        .into_iter()
        .flatten()
        {
            if !candidate.is_empty() && Path::new(&candidate).exists() {
                sources.push(PathBuf::from(candidate));
            }
        }
    }
    if sources.is_empty() {
        bail!(
            "no legacy stores found — pass --analytics-db and/or --kronika-db\n\
             (the old tumult-analytics store defaults to ~/.tumult/analytics.duckdb; \
             the kronika lake was $KRONIKA_DB)"
        );
    }

    let db_path = store_path.map_or_else(
        || AnalyticsStore::default_path().context("failed to determine analytics store path"),
        |p| Ok(p.to_path_buf()),
    )?;
    let store = AnalyticsStore::open(&db_path)?;

    for source in &sources {
        validate_path_no_symlink(source)?;
        // Never import a store into itself.
        if source.canonicalize().ok() == db_path.canonicalize().ok() {
            crate::outln!("Skipping {} — that IS the unified store", source.display());
            continue;
        }
        let report = store
            .import_legacy(source)
            .with_context(|| format!("importing legacy store {}", source.display()))?;
        crate::outln!("{}:", source.display());
        if report.is_empty() {
            crate::outln!("  (no known tables found)");
        }
        for (table, n) in &report {
            crate::outln!("  {table}: {n} row(s) imported");
        }
    }
    crate::outln!("Unified store: {}", db_path.display());
    Ok(())
}

// ── Migrate command ─────────────────────────────────────────

/// # Errors
///
/// Returns an error if `ClickHouse` is not configured, the `DuckDB` store
/// cannot be opened, or the migration fails.
#[must_use = "callers must handle migration errors"]
pub async fn cmd_store_migrate() -> Result<()> {
    use tumult_lake::{AnalyticsBackend, AnalyticsStore};

    if !tumult_clickhouse::ClickHouseConfig::is_configured() {
        bail!(
            "TUMULT_CLICKHOUSE_URL not set. Set it to migrate DuckDB → ClickHouse.\n\
             Example: TUMULT_CLICKHOUSE_URL=http://localhost:8123 tumult store migrate"
        );
    }

    let db_path =
        AnalyticsStore::default_path().context("failed to determine analytics store path")?;
    if !db_path.exists() {
        bail!("no DuckDB store found at: {}", db_path.display());
    }

    let duckdb = AnalyticsStore::open(&db_path)?;
    let duckdb_count = duckdb.experiment_count()?;
    if duckdb_count == 0 {
        crate::outln!("DuckDB store is empty — nothing to migrate.");
        return Ok(());
    }

    crate::outln!("Migrating {duckdb_count} experiments from DuckDB to ClickHouse...");

    let config = tumult_clickhouse::ClickHouseConfig::from_env();
    let ch_store = tumult_clickhouse::ClickHouseStore::connect(&config)
        .await
        .context("failed to connect to ClickHouse")?;

    // Read all experiments from DuckDB and re-ingest into ClickHouse
    let rows = duckdb.query("SELECT experiment_id FROM experiments ORDER BY started_at_ns")?;

    let mut migrated = 0;
    let mut skipped = 0;

    for row in &rows {
        let experiment_id = &row[0];
        // Validate experiment_id is safe for interpolation: UUIDs contain only
        // hex digits and hyphens; reject anything else before building the query.
        let safe_id = if experiment_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            experiment_id.as_str()
        } else {
            tracing::warn!(
                name: "cli.migrate.experiment_id.invalid",
                { resilience.experiment.id = %bounded(experiment_id) },
                "experiment skipped, its id is not safe to query with"
            );
            skipped += 1;
            continue;
        };

        let ch_exists = ch_store
            .query(&format!(
                "SELECT count() FROM experiments WHERE experiment_id = '{safe_id}'"
            ))
            .with_context(|| format!("ClickHouse query failed for experiment_id: {safe_id}"))?;

        let already_exists = ch_exists
            .first()
            .and_then(|r| r.first())
            .is_some_and(|v| v != "0");

        if already_exists {
            skipped += 1;
        } else {
            migrated += 1;
        }
    }

    // Export from DuckDB to temp Parquet, import into ClickHouse via Arrow
    let tmp_dir = std::env::temp_dir().join("tumult-migrate");
    tokio::fs::create_dir_all(&tmp_dir).await?;
    let exp_path = tmp_dir.join("experiments.parquet");
    let act_path = tmp_dir.join("activities.parquet");
    duckdb.export_tables(&exp_path, &act_path)?;

    ch_store
        .query(&format!(
            "INSERT INTO experiments SELECT * FROM file('{}', Parquet)",
            exp_path.display()
        ))
        .unwrap_or_else(|e| {
            // Best-effort — ClickHouse may not support file() in all configurations.
            // Log the error but do not abort the migration.
            tracing::warn!(
                name: "cli.migrate.parquet_insert_failed",
                { file.path = %exp_path.display(), error.message = %err(&e) },
                "ClickHouse INSERT from Parquet failed, migration continues"
            );
            vec![]
        });

    crate::outln!("Migration complete: {migrated} to migrate, {skipped} already in ClickHouse");
    crate::outln!("DuckDB store retained at: {}", db_path.display());

    let ch_stats = ch_store.stats()?;
    crate::outln!(
        "ClickHouse now has: {} experiments, {} activities",
        ch_stats.experiment_count,
        ch_stats.activity_count
    );

    Ok(())
}
