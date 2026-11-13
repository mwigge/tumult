//! OpenTelemetry Protocol with Apache Arrow (OTel-Arrow, "OTAP") receiver.
//!
//! The Collector's `otelarrow` exporter opens one bidirectional gRPC stream
//! per signal (`ArrowTracesService/ArrowTraces`, `ArrowLogsService/ArrowLogs`,
//! `ArrowMetricsService/ArrowMetrics`) and sends `BatchArrowRecords`: a batch
//! id plus one Arrow IPC payload per OTAP table (spans, span attributes,
//! resource attributes, events, ...). Every batch is answered with a
//! `BatchStatus` carrying the same batch id.
//!
//! This module decodes those tables back into the OTLP request types
//! (`ExportTraceServiceRequest` and friends) and hands them to the same
//! `tumult-otlp` conversion and the same single-writer channel as the OTLP
//! services, so rows are identical whichever carrier delivered them.
//!
//! The wire format follows the OTAP schema published in the
//! open-telemetry/otel-arrow repository:
//!
//! * root tables (`SPANS`, `LOGS`, `UNIVARIATE_METRICS`) carry `resource`
//!   and `scope` struct columns with per-batch ids, and an `id` of their own;
//! * child tables reference their parent through `parent_id`;
//! * ids are delta-encoded unless the field's `encoding` metadata says
//!   `plain`; attribute tables are sorted by (key, value) and the parent id is
//!   a delta from the previous row whenever key and value repeat; span events
//!   use the same trick keyed on the event name;
//! * string columns may be dictionary-encoded and payload buffers may be
//!   zstd-compressed at the IPC level.
//!
//! Each gRPC stream keeps one IPC reader per `schema_id`: the first payload
//! for a schema carries the Arrow schema message, later payloads only record
//! batches (and dictionary deltas).

mod arrays;
mod attrs;
mod common;
mod decode;
mod logs;
mod metrics;
pub mod proto;
pub mod service;
mod traces;

#[cfg(test)]
mod test_encoder;
#[cfg(test)]
mod tests;

pub use decode::{OtapBatch, StreamConsumer};
pub use logs::to_logs_request;
pub use metrics::to_metrics_request;
pub use traces::to_trace_request;

/// Why an OTAP batch could not be turned into OTLP. Reported to the exporter
/// in the batch's `BatchStatus`; the stream stays open.
#[derive(Debug, thiserror::Error)]
pub enum OtapError {
    #[error("unknown OTAP payload type {0}")]
    UnknownPayloadType(i32),

    #[error("{payload} payload for schema {schema_id} carries no record batch")]
    EmptyPayload {
        payload: &'static str,
        schema_id: String,
    },

    #[error("arrow IPC decode failed for {payload}: {source}")]
    Ipc {
        payload: &'static str,
        #[source]
        source: arrow::error::ArrowError,
    },

    #[error("OTAP batch has no {0} table")]
    MissingTable(&'static str),

    #[error("{table}.{column}: {reason}")]
    Column {
        table: &'static str,
        column: String,
        reason: String,
    },

    #[error("unsupported OTAP payload {0}")]
    Unsupported(&'static str),
}
