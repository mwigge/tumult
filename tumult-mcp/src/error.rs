//! Error types for Tumult MCP tool implementations.

/// Error returned by tool functions.
///
/// Each variant represents a distinct failure mode, enabling callers to
/// distinguish I/O failures from validation errors or execution failures.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    /// An I/O operation failed (file read, write, or directory access).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Parsing or decoding a file or format failed.
    #[error("parse error: {0}")]
    Parse(String),

    /// Experiment or query validation failed.
    #[error("validation error: {0}")]
    Validation(String),

    /// Experiment execution or encoding failed.
    #[error("execution failed: {0}")]
    Execution(String),

    /// An analytics store operation failed.
    #[error("store error: {0}")]
    Store(String),

    /// A required resource (file, directory, or store) was not found.
    #[error("not found: {0}")]
    NotFound(String),

    /// A path resolution error or directory traversal attempt was detected.
    #[error("path error: {0}")]
    Path(String),

    /// The provided input is invalid (query type, action name, etc.).
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// A resource already exists and cannot be overwritten.
    #[error("already exists: {0}")]
    AlreadyExists(String),
}
