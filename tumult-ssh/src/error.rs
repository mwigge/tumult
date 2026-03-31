//! SSH error types.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum SshError {
    #[error("russh error: {0}")]
    Russh(#[from] russh::Error),

    #[error("connection failed to {host}:{port}: {reason}")]
    ConnectionFailed {
        host: String,
        port: u16,
        reason: String,
    },

    #[error("authentication failed for {user}@{host}: {reason}")]
    AuthenticationFailed {
        host: String,
        user: String,
        reason: String,
    },

    #[error("key file not found: {path}")]
    KeyNotFound { path: String },

    #[error("key file permissions too open on {path}: mode {mode:#o}, expected 0600 or stricter")]
    KeyPermissionsTooOpen { path: String, mode: u32 },

    #[error("key parse error: {0}")]
    KeyParseError(String),

    #[error("command execution failed: {0}")]
    ExecutionFailed(String),

    #[error("channel error: {0}")]
    ChannelError(String),

    #[error("upload failed: {0}")]
    UploadFailed(String),

    #[error("session closed")]
    SessionClosed,

    #[error("timeout after {seconds}s")]
    Timeout { seconds: f64 },
}
