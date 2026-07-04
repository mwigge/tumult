//! Analytics error types.

#[cfg(feature = "duckdb")]
use std::path::PathBuf;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum AnalyticsError {
    #[cfg(feature = "duckdb")]
    #[error("arrow error: {0}")]
    Arrow(#[from] arrow::error::ArrowError),

    /// A write/read open failed because another process already holds the
    /// `DuckDB` store's lock. `DuckDB` is single-writer per file: a read-write
    /// holder is exclusive and blocks every other opener. This is typically the
    /// running Tumult MCP server holding the same store.
    #[cfg(feature = "duckdb")]
    #[error(
        "analytics store at {} is held by another process (likely the running Tumult MCP \
         server): DuckDB allows only one writer per store. Stop the MCP server, or point this \
         command at a separate store path (e.g. --store <path>), then retry.",
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
