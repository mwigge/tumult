//! Analytics error types.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum AnalyticsError {
    #[error("arrow error: {0}")]
    Arrow(#[from] arrow::error::ArrowError),

    #[error("parquet error: {0}")]
    Parquet(#[from] parquet::errors::ParquetError),

    #[error("duckdb error: {0}")]
    DuckDb(#[from] duckdb::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
