//! Error type for the ingest layer.

use tumult_lake::StoreError;

#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    #[error("failed to decode OTLP protobuf payload: {0}")]
    Decode(#[from] prost::DecodeError),

    #[error(transparent)]
    Store(#[from] StoreError),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Csv(#[from] csv::Error),

    #[error("ingest writer channel closed or write failed: {0}")]
    Channel(String),

    #[error("unrecognized import format for {0}: expected a tumult journal JSON object or CSV with a header row")]
    UnknownFormat(String),
}

/// What a sender is told when its batch was not stored: the writer task
/// is gone or the store refused the write. Nothing of the batch was stored
/// (a batch is one transaction), so every carrier tells the sender to send
/// it again: OTLP/gRPC and OTel-Arrow with [`WriteFailed::CODE`], OTLP/HTTP
/// with [`WriteFailed::HTTP`]. The daemon itself does not retry.
///
/// Every failure gets this answer: the writer hands back an untyped error,
/// so a batch the store refuses for its own content is sent again too,
/// until the retry budget of the sender ends.
pub(crate) struct WriteFailed;

impl WriteFailed {
    /// The words the sender gets. The cause, which may name a store path
    /// or SQL, goes only to the log.
    pub(crate) const MESSAGE: &'static str = "telemetry write failed";

    /// `UNAVAILABLE`: the gRPC code an OTLP or OTel-Arrow exporter retries.
    pub(crate) const CODE: tonic::Code = tonic::Code::Unavailable;

    /// `503`: an HTTP status an OTLP/HTTP exporter retries.
    pub(crate) const HTTP: axum::http::StatusCode = axum::http::StatusCode::SERVICE_UNAVAILABLE;

    /// The answer as a gRPC status.
    pub(crate) fn status() -> tonic::Status {
        tonic::Status::new(Self::CODE, Self::MESSAGE)
    }
}
