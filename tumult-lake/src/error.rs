//! Error types for the unified store: [`StoreError`] for the telemetry /
//! manual-evidence family ([`crate::Store`], [`crate::Writer`],
//! [`crate::Reader`]) and [`AnalyticsError`] for the journal-analytics
//! family ([`crate::duckdb_store::AnalyticsStore`], the
//! [`crate::backend::AnalyticsBackend`] trait and its ClickHouse
//! implementation).

#[cfg(feature = "duckdb")]
use std::path::PathBuf;

/// Errors raised by [`crate::Store`], [`crate::Writer`] and [`crate::Reader`].
#[cfg(feature = "duckdb")]
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// Another process holds the store open read-write (or a writer blocks
    /// readers). DuckDB is single-writer per file; see the crate docs.
    #[error(
        "store at {} is locked by another process holding it open read-write; \
         stop the other process or use a different TUMULT_LAKE_PATH path",
        path.display()
    )]
    StoreLocked { path: PathBuf },

    #[error("duckdb error: {0}")]
    DuckDb(#[from] duckdb::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("internal store error: {0}")]
    Internal(String),
}

/// Errors raised by the analytics family: the embedded
/// [`crate::duckdb_store::AnalyticsStore`], the export/import helpers, and
/// external backends behind [`crate::backend::AnalyticsBackend`].
#[derive(Debug, thiserror::Error)]
pub enum AnalyticsError {
    #[cfg(feature = "duckdb")]
    #[error("arrow error: {0}")]
    Arrow(#[from] arrow::error::ArrowError),

    /// A write/read open failed because another process already holds the
    /// `DuckDB` store's lock. `DuckDB` is single-writer per file: a read-write
    /// holder is exclusive and blocks every other opener. This is typically the
    /// running Tumult MCP server or the tumultd daemon holding the same store.
    #[cfg(feature = "duckdb")]
    #[error(
        "analytics store at {} is held by another process (likely the running Tumult MCP \
         server or tumultd): DuckDB allows only one writer per store. Stop the other process, \
         or point this command at a separate store path (e.g. --store <path>), then retry.",
        .path.display()
    )]
    StoreLocked { path: PathBuf },

    #[cfg(feature = "duckdb")]
    #[error("parquet error: {0}")]
    Parquet(#[from] parquet::errors::ParquetError),

    #[cfg(feature = "duckdb")]
    #[error("duckdb error: {0}")]
    DuckDb(#[from] duckdb::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("clickhouse error: {0}")]
    ClickHouse(String),

    #[error("internal analytics error: {0}")]
    Internal(String),
}
