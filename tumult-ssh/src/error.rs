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

    #[error("host key verification failed: server key not recognized")]
    HostKeyVerificationFailed,

    /// The server's public key was not found in the `known_hosts` file.
    #[error("host key for {host} not found in known_hosts (fingerprint: {fingerprint})")]
    HostKeyNotFound { host: String, fingerprint: String },

    /// The server's public key does not match the entry in `known_hosts`.
    #[error(
        "host key mismatch for {host}: expected {expected_fingerprint}, got {actual_fingerprint}"
    )]
    HostKeyMismatch {
        host: String,
        expected_fingerprint: String,
        actual_fingerprint: String,
    },

    /// Failed to read or write the `known_hosts` file.
    #[error("known_hosts file error for {path}: {reason}")]
    KnownHostsIo { path: String, reason: String },

    #[error("session closed")]
    SessionClosed,

    #[error("timeout after {seconds}s")]
    Timeout { seconds: f64 },
}
