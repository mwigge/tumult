//! Error type for the ingest layer.

use kronika_store::StoreError;

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
