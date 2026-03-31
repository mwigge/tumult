//! Tumult `ClickHouse` — External analytics backend shared with `SigNoz`.
//!
//! When a `ClickHouse` instance is available (e.g., `SigNoz`'s `ClickHouse`),
//! Tumult can write experiment data alongside `OTel` traces/metrics/logs,
//! enabling cross-correlation queries in a single database.
//!
//! # Dual-mode analytics
//!
//! | Mode | Backend | When |
//! |------|---------|------|
//! | Embedded (default) | `DuckDB` | `~/.tumult/analytics.duckdb` — works offline |
//! | External | `ClickHouse` | `TUMULT_CLICKHOUSE_URL` set — shared with `SigNoz` |
//!
//! # Schema
//!
//! Creates a `tumult` database with `experiments` and `activity_results`
//! tables using `MergeTree` engines. Compatible with `ClickHouse`'s columnar
//! storage and `SigNoz`'s existing `signoz_*` databases.

pub mod config;
pub mod error;
pub mod store;
pub(crate) mod telemetry;

pub use config::ClickHouseConfig;
pub use error::ClickHouseError;
pub use store::ClickHouseStore;
