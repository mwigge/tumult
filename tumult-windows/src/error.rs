//! Windows fault-injection error types.

use thiserror::Error;

/// Errors raised while constructing or executing a Windows-native fault.
///
/// [`Self::InvalidArgument`] is a caller-input error (raised on Linux and
/// Windows alike, since argument validation is host-independent), while
/// [`Self::Spawn`] and [`Self::CommandFailed`] only occur where the Windows
/// tools (`taskkill`, `netsh`) actually run.
#[derive(Error, Debug)]
pub enum WindowsError {
    /// An argument was missing, mistyped, or mutually exclusive with another.
    #[error("invalid argument `{argument}`: {reason}")]
    InvalidArgument {
        /// The argument key that was invalid.
        argument: String,
        /// Human-readable explanation of why the value is invalid.
        reason: String,
    },

    /// The Windows tool could not be spawned at all (e.g. `taskkill` is not on
    /// `PATH` — the expected outcome when the crate is exercised on Linux).
    #[error("failed to spawn `{program}`: {source}")]
    Spawn {
        /// The program that could not be launched.
        program: String,
        /// The underlying OS spawn error.
        #[source]
        source: std::io::Error,
    },

    /// The tool ran to completion but exited non-zero.
    #[error("`{program}` exited with status {code}: {stderr}")]
    CommandFailed {
        /// The program that reported failure.
        program: String,
        /// The process exit code (`-1` when the process was signalled).
        code: i32,
        /// Captured standard error, trimmed.
        stderr: String,
    },
}

impl WindowsError {
    /// Build a [`WindowsError::InvalidArgument`] from a key and a reason.
    #[must_use]
    pub fn invalid_argument(argument: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::InvalidArgument {
            argument: argument.into(),
            reason: reason.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::WindowsError;

    #[test]
    fn invalid_argument_message_contains_key_and_reason() {
        let err = WindowsError::invalid_argument("image", "must be set");
        assert!(err.to_string().contains("image"));
        assert!(err.to_string().contains("must be set"));
    }

    #[test]
    fn command_failed_message_contains_program_and_code() {
        let err = WindowsError::CommandFailed {
            program: "taskkill".into(),
            code: 128,
            stderr: "not found".into(),
        };
        let message = err.to_string();
        assert!(message.contains("taskkill"));
        assert!(message.contains("128"));
        assert!(message.contains("not found"));
    }
}
