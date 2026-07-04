//! Analytics error types.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum AnalyticsError {
    #[cfg(feature = "duckdb")]
    #[error("arrow error: {0}")]
    Arrow(#[from] arrow::error::ArrowError),

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
