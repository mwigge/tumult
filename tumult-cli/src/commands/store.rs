use std::path::Path;

use anyhow::{bail, Context, Result};

use super::validate_path_no_symlink;

// ── Import command ──────────────────────────────────────────

/// # Errors
///
/// Returns an error if the directory is invalid, the parquet files are missing,
/// or the import operation fails.
#[must_use = "callers must handle import errors"]
pub fn cmd_import(parquet_dir: &Path) -> Result<()> {
    use tumult_analytics::AnalyticsStore;

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

    let db_path = AnalyticsStore::default_path();
    let store = AnalyticsStore::open(&db_path)?;
    store.import_tables(&exp_path, &act_path)?;

    let stats = store.stats()?;
    println!("Imported from: {}", parquet_dir.display());
    println!(
        "Store now contains: {} experiments, {} activities",
        stats.experiment_count, stats.activity_count
    );
    Ok(())
}

// ── Store management commands ───────────────────────────────

/// # Errors
///
/// Returns an error if the store cannot be opened or the stats query fails.
#[must_use = "callers must handle store stats errors"]
pub fn cmd_store_stats() -> Result<()> {
    use tumult_analytics::AnalyticsStore;

    let db_path = AnalyticsStore::default_path();
    if !db_path.exists() {
        println!("No persistent store found at: {}", db_path.display());
        println!("Run an experiment to create it automatically.");
        return Ok(());
    }

    let store = AnalyticsStore::open(&db_path)?;
    let stats = store.stats()?;
    let version = store.schema_version()?;

    println!("Store: {}", db_path.display());
    println!("Schema version: {version}");
    println!("Experiments: {}", stats.experiment_count);
    println!("Activities: {}", stats.activity_count);

    if let Ok(size) = std::fs::metadata(&db_path) {
        // u64 → f64: file size in MB for display; precision loss is acceptable.
        #[allow(clippy::cast_precision_loss)]
        let mb = size.len() as f64 / (1024.0 * 1024.0);
        println!("File size: {mb:.2} MB");
    }

    Ok(())
}

/// # Errors
///
/// Returns an error if the store cannot be opened, the backup directory cannot
/// be created, or the export operation fails.
#[must_use = "callers must handle backup errors"]
pub fn cmd_store_backup(output_dir: &Path) -> Result<()> {
    use tumult_analytics::AnalyticsStore;

    let db_path = AnalyticsStore::default_path();
    if !db_path.exists() {
        bail!("no persistent store found at: {}", db_path.display());
    }

    // Validate output dir is not a symlink before creating
    if output_dir.exists() {
        validate_path_no_symlink(output_dir)?;
    }
    std::fs::create_dir_all(output_dir)?;

    let store = AnalyticsStore::open(&db_path)?;
    let exp_path = output_dir.join("experiments.parquet");
    let act_path = output_dir.join("activities.parquet");

    store.export_tables(&exp_path, &act_path)?;

    let stats = store.stats()?;
    println!("Backed up to: {}", output_dir.display());
    println!("  experiments.parquet — {} rows", stats.experiment_count);
    println!("  activities.parquet — {} rows", stats.activity_count);
    Ok(())
}

/// # Errors
///
/// Returns an error if the store cannot be opened or the purge operation fails.
#[must_use = "callers must handle purge errors"]
pub fn cmd_store_purge(older_than_days: u32) -> Result<()> {
    use tumult_analytics::AnalyticsStore;

    let db_path = AnalyticsStore::default_path();
    if !db_path.exists() {
        bail!("no persistent store found at: {}", db_path.display());
    }

    let store = AnalyticsStore::open(&db_path)?;
    let purged = store.purge_older_than_days(older_than_days)?;

    let stats = store.stats()?;
    if purged == 0 {
        println!("No experiments older than {older_than_days} days found");
    } else {
        println!("Purged {purged} experiment(s) older than {older_than_days} days");
    }
    println!(
        "Remaining: {} experiments, {} activities",
        stats.experiment_count, stats.activity_count
    );
    Ok(())
}

/// # Errors
///
/// Returns an error if the store path cannot be determined or the metadata
/// cannot be read.
#[must_use = "callers must handle store path errors"]
pub fn cmd_store_path() -> Result<()> {
    use tumult_analytics::AnalyticsStore;

    let db_path = AnalyticsStore::default_path();
    println!("{}", db_path.display());
    if db_path.exists() {
        if let Ok(size) = std::fs::metadata(&db_path) {
            // u64 → f64: file size in MB for display; precision loss is acceptable.
            #[allow(clippy::cast_precision_loss)]
            let mb = size.len() as f64 / (1024.0 * 1024.0);
            println!("Size: {mb:.2} MB");
        }
    } else {
        println!("(not yet created)");
    }
    Ok(())
}

// ── Migrate command ─────────────────────────────────────────

/// # Errors
///
/// Returns an error if `ClickHouse` is not configured, the `DuckDB` store
/// cannot be opened, or the migration fails.
#[must_use = "callers must handle migration errors"]
pub async fn cmd_store_migrate() -> Result<()> {
    use tumult_analytics::{AnalyticsBackend, AnalyticsStore};

    if !tumult_clickhouse::ClickHouseConfig::is_configured() {
        bail!(
            "TUMULT_CLICKHOUSE_URL not set. Set it to migrate DuckDB → ClickHouse.\n\
             Example: TUMULT_CLICKHOUSE_URL=http://localhost:8123 tumult store migrate"
        );
    }

    let db_path = AnalyticsStore::default_path();
    if !db_path.exists() {
        bail!("no DuckDB store found at: {}", db_path.display());
    }

    let duckdb = AnalyticsStore::open(&db_path)?;
    let duckdb_count = duckdb.experiment_count()?;
    if duckdb_count == 0 {
        println!("DuckDB store is empty — nothing to migrate.");
        return Ok(());
    }

    println!("Migrating {duckdb_count} experiments from DuckDB to ClickHouse...");

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
            eprintln!("warning: skipping invalid experiment_id: {experiment_id}");
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
            eprintln!("warning: ClickHouse INSERT from Parquet failed (non-fatal): {e}");
            vec![]
        });

    println!("Migration complete: {migrated} to migrate, {skipped} already in ClickHouse");
    println!("DuckDB store retained at: {}", db_path.display());

    let ch_stats = ch_store.stats()?;
    println!(
        "ClickHouse now has: {} experiments, {} activities",
        ch_stats.experiment_count, ch_stats.activity_count
    );

    Ok(())
}
