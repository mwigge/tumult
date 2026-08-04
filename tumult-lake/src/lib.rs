// Imported from kronika (Apache-2.0, same author). Pedantic lints are
// scoped to tumult-native crates: this crate predates the pedantic gate and
// carries intentional patterns it flags (timestamp/score casts, f64
// comparisons). CI still applies -D warnings to it.
#![allow(clippy::pedantic)]

//! `tumult-lake` — the unified embedded `DuckDB` store: chaos/resilience
//! telemetry (spans, logs, metrics), manual evidence, the daemon-run tables,
//! auth identities (users, sessions, tokens), and the journal
//! analytics family (experiments, activities, ChaosGraph, autopilot) in one
//! database file, behind one writer.
//!
//! # Concurrency: the single-writer model
//!
//! `DuckDB` is **single-writer per file**. A read-write open takes an
//! exclusive lock on the database file, so:
//!
//! * **Writes** go through [`Store::writer`] (one per process; the ingest
//!   daemon funnels all writes through a single channel onto it) or
//!   [`AnalyticsStore::open`] for the journal-analytics family.
//! * **Reads** go through [`Store::read_only`] /
//!   [`AnalyticsStore::open_read_only`], opened with
//!   `access_mode = READ_ONLY`, which does not take the exclusive write lock —
//!   multiple readers coexist, including alongside an open writer.
//! * A conflicting second opener gets the opaque `DuckDB` lock error mapped to
//!   the clear [`StoreError::StoreLocked`] / [`AnalyticsError::StoreLocked`].
//!
//! **Encryption limitation:** `DuckDB` does not encrypt at rest. The store
//! directory is created with mode `0o700` (owner-only); place it on an
//! encrypted volume for sensitive data.
//!
//! # Features
//!
//! * `duckdb` (default) — the embedded store. Disable default features to
//!   get only the lightweight backend trait and shared types
//!   ([`AnalyticsBackend`], [`AnalyticsError`], [`QueryRow`], [`StoreStats`],
//!   [`telemetry`]) without compiling the bundled `DuckDB` C++ library —
//!   this is what `tumult-clickhouse` does.

#[cfg(feature = "duckdb")]
pub mod approvals;
#[cfg(feature = "duckdb")]
pub mod arrow_convert;
#[cfg(feature = "duckdb")]
pub mod auth;
pub mod backend;
#[cfg(feature = "duckdb")]
pub mod duckdb_store;
pub mod error;
#[cfg(feature = "duckdb")]
pub mod export;
#[cfg(feature = "duckdb")]
pub mod lake;
#[cfg(feature = "duckdb")]
mod manual;
pub mod query_row;
#[cfg(feature = "duckdb")]
mod reader;
#[cfg(feature = "duckdb")]
mod rows;
#[cfg(feature = "duckdb")]
mod runs;
#[cfg(feature = "duckdb")]
mod schedules;
#[cfg(feature = "duckdb")]
mod schema;
#[cfg(feature = "duckdb")]
mod store;
pub mod telemetry;
#[cfg(feature = "duckdb")]
mod writer;

#[cfg(feature = "duckdb")]
pub use approvals::{approval_pin, ApprovalDecision, ApprovalRequest, CanonicalPin};
#[cfg(feature = "duckdb")]
pub use auth::{SessionRow, TokenRow, UserRow};
pub use backend::{AnalyticsBackend, StoreStats};
pub use error::AnalyticsError;
#[cfg(feature = "duckdb")]
pub use error::StoreError;
#[cfg(feature = "duckdb")]
pub use manual::{
    AttachmentKind, ExerciseType, ManualDetail, ManualError, ManualOutcome, NewManualExperiment,
};
pub use query_row::QueryRow;
#[cfg(feature = "duckdb")]
pub use reader::Reader;
#[cfg(feature = "duckdb")]
pub use rows::{
    ExperimentRun, ImportBatch, LogRow, MetricGaugeRow, MetricHistogramRow, MetricSumRow, SpanRow,
};
#[cfg(feature = "duckdb")]
pub use runs::{rollback_status, run_state, NewRun, RegisteredDefinition};
#[cfg(feature = "duckdb")]
pub use schedules::ScheduleRow;
#[cfg(feature = "duckdb")]
pub use schema::CURRENT_SCHEMA_VERSION;
#[cfg(feature = "duckdb")]
pub use store::Store;
#[cfg(feature = "duckdb")]
pub(crate) use store::{migrate, query_json_rows, with_tx};
#[cfg(feature = "duckdb")]
pub use writer::Writer;

#[cfg(feature = "duckdb")]
pub use arrow_convert::journal_to_record_batch;
#[cfg(feature = "duckdb")]
pub use duckdb_store::autopilot::{
    ChangeEventRecord, ClassHistory, DecisionRecord, DecisionStatus,
};
#[cfg(feature = "duckdb")]
pub use duckdb_store::{
    AgenticContractAnalytics, AgenticFaultAnalytics, AgenticRunAnalytics, AnalyticsStore,
};
#[cfg(feature = "duckdb")]
pub use export::{export_arrow_ipc, export_csv, export_parquet, import_parquet};

#[cfg(all(test, feature = "duckdb"))]
mod tests;
