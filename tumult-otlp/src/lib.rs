//! `tumult-otlp` — pure translation from OTLP protobuf requests
//! (`opentelemetry-proto` tonic types) to `tumult-lake` row structs.
//!
//! The translation promotes the low-cardinality, high-selectivity
//! `resilience.*` attributes of the tumult metadata standard (v2.0) — plus
//! `service.name`/`service.version` from the resource — into the
//! materialized columns of the wide tables. Everything else (e.g. the
//! dynamic `resilience.baseline.probe.{name}.*` keys) stays in the
//! `MAP(VARCHAR, VARCHAR)` attribute columns.
//!
//! All functions are pure; I/O lives in `tumult-ingest`.

pub mod common;
mod logs;
mod metrics;
mod traces;

pub use logs::logs_request_to_rows;
pub use metrics::{metrics_request_to_rows, MetricRows};
pub use traces::trace_request_to_spans;
