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

    #[error("journal parse error: {0}")]
    JournalParse(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("query error: {0}")]
    Query(String),
}
