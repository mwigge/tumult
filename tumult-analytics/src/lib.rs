//! Tumult Analytics — Embedded analytics for chaos experiment journals.
//!
//! Provides:
//! - TOON Journal → Arrow RecordBatch conversion
//! - DuckDB embedded SQL queries over journal data
//! - Parquet export for long-term storage
//!
//! The pipeline: Journal (.toon) → Arrow (in-memory) → DuckDB (SQL) → Parquet (export)

pub mod arrow_convert;
pub mod duckdb_store;
pub mod error;
pub mod export;

pub use arrow_convert::journal_to_record_batch;
pub use duckdb_store::AnalyticsStore;
pub use error::AnalyticsError;
pub use export::{export_csv, export_parquet};
