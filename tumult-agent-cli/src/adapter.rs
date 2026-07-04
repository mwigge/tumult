//! The adapter contract shared by all agent CLI integrations.

use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;

use crate::error::AgentCliError;

/// Default invocation timeout when the caller does not override it.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// A single prompt to run through an agent CLI in one-shot batch mode.
#[derive(Debug, Clone)]
pub struct PromptRequest {
    /// The prompt text delivered to the CLI (via stdin).
    pub prompt: String,
    /// Optional model override; `None` uses the CLI's configured default.
    pub model: Option<String>,
    /// Working directory for the CLI subprocess. An empty path means the
    /// current directory.
    pub workspace: PathBuf,
    /// Deadline for the subprocess; it is killed and reaped on expiry.
    pub timeout: Duration,
}

impl PromptRequest {
    /// Create a request with no model override and the
    /// [default timeout](DEFAULT_TIMEOUT).
    #[must_use]
    pub fn new(prompt: impl Into<String>, workspace: impl Into<PathBuf>) -> Self {
        Self {
            prompt: prompt.into(),
            model: None,
            workspace: workspace.into(),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// The request workspace, or `.` (current directory) when it is empty.
    pub(crate) fn workspace_or_current(&self) -> PathBuf {
        if self.workspace.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            self.workspace.clone()
        }
    }
}

/// Result of probing whether a CLI binary is usable (install + version + auth).
#[derive(Debug, Clone, Serialize)]
pub struct CliProbe {
    /// Whether a runnable binary was found (and answered a version probe).
    pub installed: bool,
    /// Parsed `x.y.z` version, when the version probe output contained one.
    pub version: Option<String>,
    /// `Some(true)` / `Some(false)` only when auth state is cheaply
    /// determinable; `None` means "unclear until the binary runs".
    pub logged_in: Option<bool>,
    /// Resolved binary path, when installed.
    pub bin_path: Option<PathBuf>,
    /// Human-readable detail (auth state, install hint, probe error, ...).
    pub detail: String,
}

impl CliProbe {
    /// A probe describing a binary that could not be found.
    #[must_use]
    pub fn not_installed(detail: impl Into<String>) -> Self {
        Self {
            installed: false,
            version: None,
            logged_in: None,
            bin_path: None,
            detail: detail.into(),
        }
    }
}

/// A single non-interactive subprocess call (no TTY, no approval prompts).
#[derive(Debug, Clone)]
pub struct CliInvocation {
    /// Full argument vector; `argv[0]` is the binary path.
    pub argv: Vec<String>,
    /// Text piped to the child's stdin, if any (stdin is closed otherwise).
    pub stdin: Option<String>,
    /// Working directory for the child process.
    pub cwd: PathBuf,
    /// Extra environment variables overlaid on the inherited environment.
    pub env: Vec<(String, String)>,
    /// Deadline after which the child is killed and reaped.
    pub timeout: Duration,
}

/// Captured output of a finished (or killed) subprocess.
#[derive(Debug, Clone, Default)]
pub struct RawOutput {
    /// Captured stdout, lossily decoded as UTF-8.
    pub stdout: String,
    /// Captured stderr, lossily decoded as UTF-8.
    pub stderr: String,
    /// Process exit code; `None` when terminated by a signal.
    pub exit_code: Option<i32>,
}

/// Contract for one-shot, non-interactive agent CLI execution.
///
/// Implementations describe *how* to drive one CLI (Claude Code, Codex, ...)
/// in batch mode; the shared [`runner`](crate::runner) executes the
/// subprocess.
pub trait AgentCliAdapter {
    /// Stable adapter name used for registry lookups (e.g. `claude-code`).
    fn name(&self) -> &'static str;

    /// Env var that overrides binary resolution with an explicit path.
    fn binary_env_key(&self) -> &'static str;

    /// How to install the CLI (shown when the binary is missing).
    fn install_hint(&self) -> &'static str;

    /// How to authenticate the CLI (shown on auth failures).
    fn auth_hint(&self) -> &'static str;

    /// Resolve the binary and probe version / auth state.
    ///
    /// Never fails; problems are reported structurally via
    /// [`CliProbe::installed`], [`CliProbe::logged_in`], and
    /// [`CliProbe::detail`].
    fn detect(&self) -> CliProbe;

    /// Build the argv / stdin / env for a non-interactive run.
    ///
    /// # Errors
    ///
    /// Returns [`AgentCliError::BinaryNotFound`] when the CLI binary cannot
    /// be resolved.
    fn build_invocation(&self, req: &PromptRequest) -> Result<CliInvocation, AgentCliError>;

    /// Extract the model's answer from the raw output of a successful run.
    ///
    /// # Errors
    ///
    /// Returns [`AgentCliError::OutputParse`] when the output does not match
    /// the CLI's documented non-interactive shape, or
    /// [`AgentCliError::InvocationFailed`] when the CLI reported an error
    /// in-band despite a zero exit code.
    fn parse_output(&self, raw: &RawOutput) -> Result<String, AgentCliError>;

    /// Human-readable explanation for a failed run (non-zero exit or
    /// unusable output). Never fails; always produces a message.
    fn explain_failure(&self, raw: &RawOutput) -> String;
}

/// Truncate `text` to at most `max` characters for error snippets.
pub(crate) fn snippet(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let head: String = text.chars().take(max).collect();
        format!("{head}…")
    }
}

#[cfg(test)]
mod tests {
    use super::snippet;

    #[test]
    fn snippet_truncates_long_text() {
        let long = "x".repeat(300);
        let s = snippet(&long, 200);
        assert_eq!(s.chars().count(), 201);
        assert!(s.ends_with('…'));
    }

    #[test]
    fn snippet_keeps_short_text() {
        assert_eq!(snippet("hello", 200), "hello");
    }
}
