//! Error type for the recommendation pipeline ([`crate::recommend`],
//! [`crate::write`], and the test-only model-response parsing).
//!
//! Follows the same `thiserror` taxonomy as [`crate::llm::AiError`] and
//! [`crate::sql_guard::SqlGuardError`]; binaries map it into `anyhow` at
//! their boundary.

use std::path::PathBuf;

/// Errors from building, rendering, or persisting recommendations.
#[derive(Debug, thiserror::Error)]
pub enum RecommendError {
    /// JSON encoding of the recommendation output failed.
    #[error("encode JSON: {0}")]
    Json(#[from] serde_json::Error),

    /// The experiment output directory could not be created.
    #[error("create experiment output dir {}: {source}", path.display())]
    CreateOutputDir {
        path: PathBuf,
        source: std::io::Error,
    },

    /// A validated experiment file could not be written.
    #[error("write experiment {}: {source}", path.display())]
    WriteExperiment {
        path: PathBuf,
        source: std::io::Error,
    },

    /// The model answer contained no parseable recommendation JSON.
    #[error("model response did not contain valid recommendation JSON")]
    InvalidModelResponse,

    /// The parsed model response carried an empty recommendations list.
    #[error("model response did not include recommendations")]
    EmptyModelResponse,
}
