//! `tumult-ingest` — telemetry ingestion into the kronika store.
//!
//! * **OTLP/gRPC** ([`grpc`]): what tumult's exporter talks
//!   (`OTEL_EXPORTER_OTLP_ENDPOINT=http://host:4317`, bare host, no path).
//! * **OTLP/HTTP protobuf** ([`http`]): what smedja's exporter talks
//!   (`SMEDJA_OTLP_ENDPOINT=http://host:4318`, `/v1/*` paths), plus the
//!   daemon's `GET /healthz`.
//! * **Single-writer channel** ([`writer`]): both servers funnel batches
//!   through one bounded channel onto the store's single `DuckDB` writer.
//! * **Manual import** ([`manual`]): CSV and tumult journal JSON files.

pub mod config;
mod error;
pub mod grpc;
pub mod http;
pub mod manual;
mod writer;

pub use config::Config;
pub use error::IngestError;
pub use manual::{ImportSummary, ManualImporter};
pub use writer::{Batch, IngestWriter};

/// Current time as epoch nanoseconds — the receipt timestamp given to
/// conversions for telemetry that carries no timestamp of its own.
pub(crate) fn now_ns() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as i64)
}
