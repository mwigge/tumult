//! `OpenAI` Codex adapter (`codex exec`, non-interactive).
//!
//! # Invocation shape
//!
//! ```text
//! codex exec --ephemeral -s read-only --color never \
//!     -C <workspace> --skip-git-repo-check [-m <model>] -    # prompt via stdin
//! ```
//!
//! Non-interactive shape: `codex exec` with an ephemeral session,
//! a read-only sandbox, colors disabled, and the prompt read from stdin
//! (the trailing `-`). Codex prints the agent's final message to stdout, so
//! [`CodexAdapter::parse_output`] returns trimmed stdout and treats empty
//! output as a parse error. One deviation: `--skip-git-repo-check` is always
//! passed unconditionally because an explicit
//! workspace is always supplied here and the flag is harmless inside a repo.
//!
//! # Environment
//!
//! - `CODEX_BIN` — explicit binary path override (ignored when not
//!   executable).
//! - `OPENAI_API_KEY` — inherited by the child; when set, `detect()` cheaply
//!   reports `logged_in = Some(true)`.

use std::time::Duration;

use crate::adapter::{AgentCliAdapter, CliInvocation, CliProbe, PromptRequest, RawOutput};
use crate::error::AgentCliError;
use crate::{resolver, runner};

const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

const NAME: &str = "codex";
const BINARY: &str = "codex";
const BINARY_ENV_KEY: &str = "CODEX_BIN";
const INSTALL_HINT: &str = "npm i -g @openai/codex";
const AUTH_HINT: &str = "Run: codex login or set OPENAI_API_KEY";

/// Non-interactive Codex CLI adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct CodexAdapter;

impl CodexAdapter {
    /// Create the adapter.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    fn resolve() -> Option<std::path::PathBuf> {
        resolver::resolve_binary(BINARY_ENV_KEY, BINARY)
    }
}

impl AgentCliAdapter for CodexAdapter {
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
                "Codex CLI not found on PATH. Install with: {INSTALL_HINT} \
                 or set {BINARY_ENV_KEY}."
            ));
        };
        match runner::probe_version(NAME, &bin, PROBE_TIMEOUT) {
            Err(detail) => CliProbe::not_installed(detail),
            Ok(output) => {
                // `codex login status` is not probed here — a subprocess
                // probe is too slow for detect(); only the API-key env
                // fallback is cheap enough to report.
                let (logged_in, detail) = if resolver::env_nonempty("OPENAI_API_KEY") {
                    (Some(true), "Authenticated via OPENAI_API_KEY.".to_string())
                } else {
                    (
                        None,
                        "Codex CLI installed; login state not probed — verified on \
                         first invocation."
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

        let workspace = req.workspace_or_current();
        let mut argv = vec![
            bin.display().to_string(),
            "exec".to_string(),
            "--ephemeral".to_string(),
            "-s".to_string(),
            "read-only".to_string(),
            "--color".to_string(),
            "never".to_string(),
            "-C".to_string(),
            workspace.display().to_string(),
            "--skip-git-repo-check".to_string(),
        ];
        if let Some(model) = req
            .model
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty())
        {
            argv.push("-m".to_string());
            argv.push(model.to_string());
        }
        // Trailing `-`: read the prompt from stdin.
        argv.push("-".to_string());

        Ok(CliInvocation {
            argv,
            stdin: Some(req.prompt.clone()),
            cwd: workspace,
            env: Vec::new(),
            timeout: req.timeout,
        })
    }

    fn parse_output(&self, raw: &RawOutput) -> Result<String, AgentCliError> {
        let result = raw.stdout.trim();
        if result.is_empty() {
            return Err(AgentCliError::OutputParse {
                name: NAME.to_string(),
                detail: "`codex exec` produced empty stdout (expected the agent's final \
                         message)"
                    .to_string(),
            });
        }
        Ok(result.to_string())
    }

    fn explain_failure(&self, raw: &RawOutput) -> String {
        runner::explain_cli_failure("codex exec", raw)
    }
}
