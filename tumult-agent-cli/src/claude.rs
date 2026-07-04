//! Claude Code adapter (`claude -p`, print mode, non-interactive).
//!
//! # Invocation shape
//!
//! ```text
//! claude -p --output-format json [--model <m>]     # prompt piped via stdin
//! ```
//!
//! Print mode (`-p`) with the prompt on stdin is
//! `claude_code.py`; `--output-format json` makes the CLI emit a single JSON
//! result envelope (`{"type":"result","is_error":false,"result":"...",...}`)
//! from which [`ClaudeCodeAdapter::parse_output`] extracts the `result`
//! field. Parsing is strict: JSON mode means JSON, so malformed output is a
//! typed [`AgentCliError::OutputParse`] rather than a silent raw-stdout
//! fallback.
//!
//! # Environment
//!
//! - `CLAUDE_CODE_BIN` — explicit binary path override (ignored when not
//!   executable).
//! - `ANTHROPIC_API_KEY` — inherited by the child; when set, `detect()`
//!   cheaply reports `logged_in = Some(true)`.

use std::time::Duration;

use crate::adapter::{snippet, AgentCliAdapter, CliInvocation, CliProbe, PromptRequest, RawOutput};
use crate::error::AgentCliError;
use crate::{resolver, runner};

/// `claude --version` does config/cache init that can be slow on cold starts,
/// so its probe budget is larger than Codex's.
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

const NAME: &str = "claude-code";
const BINARY: &str = "claude";
const BINARY_ENV_KEY: &str = "CLAUDE_CODE_BIN";
const INSTALL_HINT: &str = "npm i -g @anthropic-ai/claude-code";
const AUTH_HINT: &str = "Run: claude auth login or set ANTHROPIC_API_KEY";

/// Non-interactive Claude Code CLI adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClaudeCodeAdapter;

impl ClaudeCodeAdapter {
    /// Create the adapter.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    fn resolve() -> Option<std::path::PathBuf> {
        resolver::resolve_binary(BINARY_ENV_KEY, BINARY)
    }
}

impl AgentCliAdapter for ClaudeCodeAdapter {
    fn name(&self) -> &'static str {
        NAME
    }

    fn binary_env_key(&self) -> &'static str {
        BINARY_ENV_KEY
    }

    fn install_hint(&self) -> &'static str {
        INSTALL_HINT
    }

    fn auth_hint(&self) -> &'static str {
        AUTH_HINT
    }

    fn detect(&self) -> CliProbe {
        let Some(bin) = Self::resolve() else {
            return CliProbe::not_installed(format!(
                "Claude Code CLI not found on PATH. Install with: {INSTALL_HINT} \
                 or set {BINARY_ENV_KEY}."
            ));
        };
        match runner::probe_version(NAME, &bin, PROBE_TIMEOUT) {
            Err(detail) => CliProbe::not_installed(detail),
            Ok(output) => {
                // `claude auth status` can be slow (it may touch shared CLI
                // state), so it is deliberately not probed here. Auth is
                // cheaply confirmable only via the API-key env fallback;
                // otherwise it stays unclear until the binary runs.
                let (logged_in, detail) = if resolver::env_nonempty("ANTHROPIC_API_KEY") {
                    (
                        Some(true),
                        "Authenticated via ANTHROPIC_API_KEY.".to_string(),
                    )
                } else {
                    (
                        None,
                        "Auth state not probed (skipping slow `claude auth status`); \
                         verified on first invocation."
                            .to_string(),
                    )
                };
                CliProbe {
                    installed: true,
                    version: runner::extract_semver(&output),
                    logged_in,
                    bin_path: Some(bin),
                    detail,
                }
            }
        }
    }

    fn build_invocation(&self, req: &PromptRequest) -> Result<CliInvocation, AgentCliError> {
        let bin = Self::resolve().ok_or_else(|| AgentCliError::BinaryNotFound {
            name: NAME.to_string(),
            install_hint: format!("{INSTALL_HINT} or set {BINARY_ENV_KEY} to the binary path"),
        })?;

        let mut argv = vec![
            bin.display().to_string(),
            "-p".to_string(),
            "--output-format".to_string(),
            "json".to_string(),
        ];
        if let Some(model) = req
            .model
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty())
        {
            argv.push("--model".to_string());
            argv.push(model.to_string());
        }

        Ok(CliInvocation {
            argv,
            stdin: Some(req.prompt.clone()),
            cwd: req.workspace_or_current(),
            env: Vec::new(),
            timeout: req.timeout,
        })
    }

    fn parse_output(&self, raw: &RawOutput) -> Result<String, AgentCliError> {
        let stdout = raw.stdout.trim();
        let envelope: serde_json::Value =
            serde_json::from_str(stdout).map_err(|e| AgentCliError::OutputParse {
                name: NAME.to_string(),
                detail: format!(
                    "expected a JSON result envelope from `claude -p --output-format json` \
                     but stdout is not valid JSON ({e}); stdout snippet: {:?}",
                    snippet(stdout, 200)
                ),
            })?;

        if envelope
            .get("is_error")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        {
            let message = envelope
                .get("result")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("(no result text in error envelope)");
            return Err(AgentCliError::InvocationFailed {
                name: NAME.to_string(),
                explain: format!(
                    "Claude Code reported an error result: {}",
                    snippet(message, 500)
                ),
            });
        }

        match envelope.get("result").and_then(serde_json::Value::as_str) {
            Some(result) => Ok(result.to_string()),
            None => Err(AgentCliError::OutputParse {
                name: NAME.to_string(),
                detail: format!(
                    "JSON envelope has no string `result` field \
                     (subtype: {}); stdout snippet: {:?}",
                    envelope
                        .get("subtype")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("unknown"),
                    snippet(stdout, 200)
                ),
            }),
        }
    }

    fn explain_failure(&self, raw: &RawOutput) -> String {
        runner::explain_cli_failure("claude -p", raw)
    }
}
