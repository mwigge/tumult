//! Tumult Agent CLI — an adapter layer for invoking agentic coding CLIs
//! (Claude Code, `OpenAI` Codex) **non-interactively**.
//!
//! Each adapter drives its CLI in one-shot batch mode: a single subprocess
//! call with no TTY, no approval prompts, and no session persistence. The
//! prompt goes in (via stdin), the model's final answer comes out. Because
//! these are blocking, run-to-completion calls, subprocess execution uses
//! [`std::process`] rather than an async runtime.
//!
//! The adapter contract is intentionally small: every adapter can
//! *detect* its binary (install + version + auth probe), *build* a
//! non-interactive invocation, *parse* the model answer out of raw output,
//! and *explain* a failure in human-readable terms.
//!
//! # Quick start
//!
//! ```no_run
//! use std::path::PathBuf;
//! use tumult_agent_cli::{AdapterRegistry, PromptRequest};
//!
//! let registry = AdapterRegistry::builtin();
//! let adapter = registry.get("claude-code")?;
//! let request = PromptRequest::new("Summarize this repo.", PathBuf::from("."));
//! let answer = tumult_agent_cli::run_prompt(adapter, &request)?;
//! println!("{answer}");
//! # Ok::<(), tumult_agent_cli::AgentCliError>(())
//! ```
//!
//! # Binary resolution
//!
//! Each adapter honors an explicit env-var override (`CLAUDE_CODE_BIN`,
//! `CODEX_BIN`) pointing at the binary; a non-executable override is ignored
//! with a warning and resolution falls back to a `PATH` search.
//!
//! # Environment
//!
//! The runner always sets `NO_COLOR=1` on the child process so output stays
//! machine-parseable. Everything else is inherited from the parent process,
//! so API-key auth (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`) flows through.

pub mod adapter;
pub mod claude;
pub mod codex;
pub mod error;
pub mod registry;
pub mod resolver;
pub mod runner;

pub use adapter::{AgentCliAdapter, CliInvocation, CliProbe, PromptRequest, RawOutput};
pub use claude::ClaudeCodeAdapter;
pub use codex::CodexAdapter;
pub use error::AgentCliError;
pub use registry::AdapterRegistry;
pub use runner::{run, run_prompt};
