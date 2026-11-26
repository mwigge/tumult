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
//!   multiple readers coexist across processes only when no process holds a writer.
//!   Within the daemon process, readers share its writer instance.
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
mod dr;
#[cfg(feature = "duckdb")]
pub mod duckdb_store;
#[cfg(feature = "duckdb")]
mod entities;
pub mod error;
#[cfg(feature = "duckdb")]
pub mod export;
#[cfg(feature = "duckdb")]
mod file_integrity;
#[cfg(feature = "duckdb")]
mod forecast;
#[cfg(feature = "duckdb")]
mod ict;
#[cfg(feature = "duckdb")]
pub mod lake;
#[cfg(feature = "duckdb")]
mod manual;
#[cfg(feature = "duckdb")]
mod perf;
pub mod query_row;
#[cfg(feature = "duckdb")]
mod reader;
#[cfg(feature = "duckdb")]
mod risk;
#[cfg(feature = "duckdb")]
mod rows;
#[cfg(feature = "duckdb")]
mod runs;
#[cfg(feature = "duckdb")]
mod schedules;
#[cfg(feature = "duckdb")]
mod schema;
pub mod scope;
#[cfg(feature = "duckdb")]
mod sightings;
#[cfg(feature = "duckdb")]
mod store;
pub mod telemetry;
#[cfg(feature = "duckdb")]
mod version_order;
#[cfg(feature = "duckdb")]
mod webhooks;
#[cfg(feature = "duckdb")]
mod writer;

#[cfg(feature = "duckdb")]
pub use approvals::{approval_pin, ApprovalDecision, ApprovalRequest, CanonicalPin};
#[cfg(feature = "duckdb")]
pub use auth::{SessionRow, TokenRow, UserRow};
pub use backend::{AnalyticsBackend, StoreStats};
#[cfg(feature = "duckdb")]
pub use dr::{DrScope, DrTestFilter, DrTestPosition, DEFAULT_DR_EXERCISE_TYPES};
#[cfg(feature = "duckdb")]
pub use entities::{
    entity_key, EntityApplied, EntityEventKind, EntityEventRow, EntityEvictions, EntityLimits,
    EntityOutcome, EntityPurge, EvictionReason, WireForm, ENTITIES_PER_ENVIRONMENT,
    ENTITY_INTERVAL_MAX_S, ENTITY_UNREPORTED_AFTER_S, ENTITY_VERSIONS_PER_ENTITY,
};
pub use error::AnalyticsError;
#[cfg(feature = "duckdb")]
pub use error::StoreError;
#[cfg(feature = "duckdb")]
pub use forecast::{
    ForecastLookup, ForecastRow, ModelFilter, FORECAST_PRUNE_BATCH, FORECAST_RETENTION_NS,
    FORECAST_ROWS_PER_SERIES,
};
#[cfg(feature = "duckdb")]
pub use ict::{
    is_critical_or_important, IctFunction, IctFunctionService, IctLegalEntity, IctRegister,
    MaturityDailyRow, PlannedTest, RequirementStatusRow,
};
#[cfg(feature = "duckdb")]
pub use manual::{
    attachments_dir, stored_file, ActionItem, AttachmentKind, AuditProblem, AuditVerdict,
    ExerciseDetail, ExerciseLink, ExerciseStep, ExerciseType, FileError, FileUpload, LinkKind,
    ManualDetail, ManualError, ManualOutcome, NewExercisePlan, NewManualExperiment, Participant,
    PlanDetail, PlanEdit, PlanError, PlanFilter, PlanStatus, Retest, RetestKind, Severity, SignOff,
    SignOffDecision, SignOffOutcome, StagedFile, StepResult, Tester, TesterKind, TlptDetail,
    MAX_FILE_BYTES, MEDIA_TYPES,
};
#[cfg(feature = "duckdb")]
pub use perf::PerfVerdictRow;
pub use query_row::QueryRow;
#[cfg(feature = "duckdb")]
pub use reader::Reader;
#[cfg(feature = "duckdb")]
pub use risk::RiskSuppressionRow;
#[cfg(feature = "duckdb")]
pub use rows::{
    ExperimentRun, ImportBatch, LogRow, MetricGaugeRow, MetricHistogramRow, MetricSumRow, SpanRow,
};
#[cfg(feature = "duckdb")]
pub use runs::{rollback_status, run_state, NewRun, RegisteredDefinition};
#[cfg(feature = "duckdb")]
pub use schedules::ScheduleRow;
#[cfg(feature = "duckdb")]
pub use schema::{
    hypothesis_met_sql, mean_recovery_time_sql, run_completions_sql, run_outcome_sql, run_verdict,
    run_verdict_sql, RunVerdict, CURRENT_SCHEMA_VERSION,
};
#[cfg(feature = "duckdb")]
pub use sightings::{StoredVersions, VersionSighting, REDEPLOY_GAP};
#[cfg(feature = "duckdb")]
pub(crate) use store::{migrate, query_json_rows, query_json_rows_bound, with_tx};
#[cfg(feature = "duckdb")]
pub use store::{EngineVersions, SqlParam, Store};
#[cfg(feature = "duckdb")]
pub use version_order::version_cmp;
#[cfg(feature = "duckdb")]
pub use webhooks::{WebhookDeadLetter, WebhookRow};
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
    RecoveryMetric, RecoveryPoint, RecoveryTrend,
};
#[cfg(feature = "duckdb")]
pub use export::{export_arrow_ipc, export_csv, export_parquet, import_parquet};

#[cfg(all(test, feature = "duckdb"))]
mod tests;

#[cfg(feature = "duckdb")]
pub mod backup;
