//! `store` subcommand arguments.

use std::path::PathBuf;

#[derive(clap::Subcommand, Debug)]
pub(crate) enum StoreAction {
    /// Show store statistics
    Stats,
    /// Export entire store to Parquet backup
    Backup {
        /// Output directory for backup files
        #[arg(long, default_value = "tumult-backup")]
        output: PathBuf,
    },
    /// Purge experiments older than N days
    Purge {
        /// Number of days to retain
        #[arg(long)]
        older_than_days: u32,
    },
    /// Show store file path
    Path,
    // DuckDB and ClickHouse are product names rendered verbatim in `--help`;
    // backticks would leak into the help text, so silence the doc-markdown
    // lint here.
    #[allow(clippy::doc_markdown)]
    /// Migrate data from DuckDB to ClickHouse
    Migrate,
    /// Import rows from legacy pre-unification databases (an old
    /// tumult-analytics store and/or a kronika lake) into the unified
    /// store. Idempotent — already-imported rows are skipped by natural key.
    ImportLegacy {
        /// Path to the legacy tumult-analytics store (e.g.
        /// ~/.tumult/analytics.duckdb or `TUMULT_ANALYTICS_PATH`)
        #[arg(long)]
        analytics_db: Option<PathBuf>,
        /// Path to the legacy kronika store (`KRONIKA_DB`)
        #[arg(long)]
        kronika_db: Option<PathBuf>,
        /// Target store path (default: ~/.tumult/lake.duckdb, override with
        /// `TUMULT_LAKE_PATH`)
        #[arg(long)]
        store: Option<PathBuf>,
    },
}
