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
//! Each gRPC stream keeps one IPC reader per payload type: the first payload
//! for a `schema_id` carries the Arrow schema message, later payloads only record
//! batches (and dictionary deltas).
//!
//! Everything a payload declares (row counts, body lengths, decompressed
//! sizes, ids) is checked against [`limits::OtapLimits`] before memory is
//! committed to it; see [`decode`] and the `hostile` tests. A batch that
//! cannot be decoded ends its stream: the IPC state after it is unknown.

mod admission;
mod arrays;
mod attrs;
mod common;
mod decode;
mod decode_gate;
mod ipc;
pub mod limits;
mod logs;
mod metrics;
// Generator output, kept byte for byte (tests/otap_proto_drift.rs).
#[rustfmt::skip]
pub mod proto;
pub mod service;
mod traces;

#[cfg(any(test, feature = "otap-client"))]
pub mod craft;
#[cfg(test)]
mod fuzz;
#[cfg(test)]
mod hostile;
#[cfg(test)]
mod hostile_ipc;
#[cfg(test)]
mod hostile_shares;
#[cfg(test)]
mod test_encoder;
#[cfg(test)]
mod tests;

pub use decode::{OtapBatch, StreamConsumer};
pub use limits::OtapLimits;
pub use logs::to_logs_request;
pub use metrics::to_metrics_request;
pub use traces::to_trace_request;

/// Why an OTAP batch could not be turned into OTLP. Reported to the exporter
/// in the batch's `BatchStatus`.
#[derive(Debug, thiserror::Error)]
pub enum OtapError {
    #[error("unknown OTAP payload type {0}")]
    UnknownPayloadType(i32),

    #[error("{payload} payload for schema {schema_id} carries no record batch")]
    EmptyPayload {
        payload: &'static str,
        schema_id: String,
    },

    /// The arrow error stays out of the message (it goes to the exporter);
    /// the server log carries it through `source`.
    #[error("arrow IPC decode failed for {payload}")]
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

    #[error("{what} exceeds the limit of {limit}")]
    Limit { what: &'static str, limit: usize },

    #[error("malformed {payload} payload: {reason}")]
    Malformed {
        payload: &'static str,
        reason: String,
    },

    #[error("batch carries more than one {0} payload")]
    DuplicatePayload(&'static str),

    #[error("an earlier batch on this stream failed to decode")]
    Poisoned,

    /// What the stream makes the receiver keep between batches is over a
    /// limit. `shared` when the limit is the one all streams share, so the
    /// same batch may well fit later or on another stream.
    #[error("{what} exceeds the limit of {limit}")]
    StreamState {
        what: &'static str,
        limit: usize,
        shared: bool,
    },

    #[error("batch not decoded within {limit_ms} ms")]
    Deadline { limit_ms: u64 },

    #[error("{payload} payload does not belong on a {signal} stream")]
    Misplaced {
        payload: &'static str,
        signal: &'static str,
    },
}

impl OtapError {
    /// Whether a configured limit, rather than malformed data, refused the
    /// batch.
    #[must_use]
    pub fn is_limit(&self) -> bool {
        matches!(
            self,
            Self::Limit { .. } | Self::StreamState { .. } | Self::Deadline { .. }
        )
    }
}
