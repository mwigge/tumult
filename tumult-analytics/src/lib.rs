//! Tumult Analytics — Embedded analytics for chaos experiment journals.
//!
//! Pipeline: Journal (.toon) → Arrow (in-memory) → `DuckDB` (SQL) → Parquet (export)
//!
//! # Overview
//!
//! `tumult-analytics` provides a zero-dependency-server analytics pipeline
//! for experiment journals. Every journal produced by `tumult-core` can be
//! ingested into an embedded `DuckDB` store, queried with SQL, and exported
//! to Parquet, Arrow IPC, or CSV for downstream tooling.
//!
//! # Pipeline stages
//!
//! 1. **Ingest** — [`AnalyticsStore::ingest_journal`] converts a [`tumult_core::types::Journal`]
//!    into Arrow record batches and appends them to the `DuckDB` tables.
//! 2. **Query** — [`AnalyticsStore::query`] executes arbitrary SQL against the
//!    `experiments` and `activity_results` tables.
//! 3. **Export** — [`export_parquet`], [`export_arrow_ipc`], and [`export_csv`]
//!    write query results to files for BI tools or long-term storage.
//!
//! # Getting started
//!
//! See the [data lifecycle guide](https://github.com/tumult-rs/tumult/blob/main/docs/data-lifecycle.md)
//! for an end-to-end walkthrough from experiment execution through analytics.

#[cfg(feature = "duckdb")]
pub mod arrow_convert;
pub mod backend;
#[cfg(feature = "duckdb")]
pub mod duckdb_store;
pub mod error;
#[cfg(feature = "duckdb")]
pub mod export;
pub mod query_row;
pub mod telemetry;

#[cfg(feature = "duckdb")]
pub use arrow_convert::journal_to_record_batch;
pub use backend::{AnalyticsBackend, StoreStats};
#[cfg(feature = "duckdb")]
pub use duckdb_store::{
    AgenticContractAnalytics, AgenticFaultAnalytics, AgenticRunAnalytics, AnalyticsStore,
};
pub use duckdb_store::topology::NodeAttrs;
pub use duckdb_store::autopilot::{ClassHistory, DecisionRecord, DecisionStatus};
// ChaosGraph query result types, re-exported for MCP/CLI consumers so they can
// build responses without depending on tumult-graph directly.
pub use error::AnalyticsError;
#[cfg(feature = "duckdb")]
pub use export::{export_arrow_ipc, export_csv, export_parquet, import_parquet};
pub use query_row::QueryRow;
#[cfg(feature = "duckdb")]
pub use tumult_graph::{EgoGraph, EgoTuple, NodeSummary};
