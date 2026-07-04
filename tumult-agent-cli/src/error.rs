//! Error types for agent CLI adapters.

use thiserror::Error;

/// Errors produced while detecting, invoking, or parsing an agent CLI.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AgentCliError {
    /// The CLI binary could not be resolved via env override or `PATH`.
    #[error("{name} CLI not found. Install with: {install_hint}")]
    BinaryNotFound {
        /// Adapter name (e.g. `claude-code`).
        name: String,
        /// How to install the CLI.
        install_hint: String,
    },

    /// The CLI is installed but definitively not authenticated.
    #[error("{name} CLI is not authenticated. {auth_hint}")]
    NotAuthenticated {
        /// Adapter name.
        name: String,
        /// How to authenticate.
        auth_hint: String,
    },

    /// The subprocess ran but failed (non-zero exit, spawn error, or an
    /// error result reported in-band by the CLI).
    #[error("{name} CLI invocation failed: {explain}")]
    InvocationFailed {
        /// Adapter name.
        name: String,
        /// Human-readable failure explanation (from `explain_failure`).
        explain: String,
    },

    /// The subprocess exceeded its deadline and was killed.
    #[error("{name} CLI timed out after {seconds}s")]
    Timeout {
        /// Adapter name.
        name: String,
        /// Configured timeout in seconds.
        seconds: f64,
    },

    /// The subprocess succeeded but its output did not match the expected
    /// shape (e.g. malformed JSON envelope, empty stdout).
    #[error("failed to parse {name} CLI output: {detail}")]
    OutputParse {
        /// Adapter name.
        name: String,
        /// What was expected and a snippet of what was received.
        detail: String,
    },

    /// A registry lookup used a name no adapter is registered under.
    #[error("unknown adapter '{name}'; available: {available}")]
    UnknownAdapter {
        /// The requested adapter name.
        name: String,
        /// Comma-separated list of registered adapter names.
        available: String,
    },
}
