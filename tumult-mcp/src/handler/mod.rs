//! MCP handler — routes tool calls to implementations.

mod auth;
mod dispatch;
mod enact;
mod executor;
mod output_schema;
mod rate_limit;
mod resources;
mod schema;
#[cfg(test)]
pub(crate) mod test_support;

pub use auth::{host_is_loopback, McpAuth, Role};
pub use executor::ProcessExecutor;
pub use schema::*;

use std::sync::Arc;

use rust_mcp_sdk::schema::{CallToolError, CallToolRequestParams};
use rust_mcp_sdk::McpServer;

use crate::tools;

// ── MCP Handler ───────────────────────────────────────────────

/// Maximum concurrent tool calls allowed.
pub(crate) const MAX_CONCURRENT_TOOL_CALLS: usize = 10;

pub struct TumultHandler {
    /// Semaphore limiting concurrent tool execution.
    pub(crate) semaphore: tokio::sync::Semaphore,
    /// Base directory for file operations (path traversal prevention).
    pub(crate) workspace_root: std::path::PathBuf,
    /// Bearer token authentication configuration.
    pub(crate) auth: McpAuth,
    /// Server-wide enactment ledger: at most one fault-injection enactment
    /// runs at a time, and the autopilot gate sees the in-flight count.
    pub(crate) enact_lock: enact::EnactLock,
    /// Per-client token-bucket rate limiter.
    pub(crate) rate_limiter: rate_limit::RateLimiter,
}

impl Default for TumultHandler {
    fn default() -> Self {
        Self {
            semaphore: tokio::sync::Semaphore::new(MAX_CONCURRENT_TOOL_CALLS),
            workspace_root: std::env::current_dir().unwrap_or_else(|_| "/".into()),
            auth: McpAuth::from_env(),
            enact_lock: enact::EnactLock::new(),
            rate_limiter: rate_limit::RateLimiter::from_env(),
        }
    }
}

impl TumultHandler {
    /// Create a handler with a specific workspace root for path validation.
    #[must_use]
    pub fn with_workspace_root(workspace_root: std::path::PathBuf) -> Self {
        Self {
            semaphore: tokio::sync::Semaphore::new(MAX_CONCURRENT_TOOL_CALLS),
            workspace_root,
            auth: McpAuth::from_env(),
            enact_lock: enact::EnactLock::new(),
            rate_limiter: rate_limit::RateLimiter::from_env(),
        }
    }

    /// Create a handler with a specific workspace root and authentication config.
    #[must_use]
    pub fn with_auth(workspace_root: std::path::PathBuf, auth: McpAuth) -> Self {
        Self {
            semaphore: tokio::sync::Semaphore::new(MAX_CONCURRENT_TOOL_CALLS),
            workspace_root,
            auth,
            enact_lock: enact::EnactLock::new(),
            rate_limiter: rate_limit::RateLimiter::from_env(),
        }
    }

    /// Replace the rate limiter (tests need deterministic buckets, not the
    /// environment's).
    #[cfg(test)]
    pub(crate) fn set_rate_limiter(&mut self, limiter: rate_limit::RateLimiter) {
        self.rate_limiter = limiter;
    }

    /// Validate and resolve a user-supplied file path against the workspace root.
    ///
    /// # Errors
    ///
    /// Returns `CallToolError` if the path escapes the workspace root or
    /// the resolved path contains non-UTF-8 characters.
    fn resolve_path(&self, user_path: &str) -> std::result::Result<String, CallToolError> {
        let resolved = tools::safe_resolve_path(&self.workspace_root, user_path)
            .map_err(|e| CallToolError::invalid_arguments("path", Some(e.to_string())))?;
        resolved
            .to_str()
            .map(std::string::ToString::to_string)
            .ok_or_else(|| {
                CallToolError::invalid_arguments(
                    "path",
                    Some(format!(
                        "path contains non-UTF-8 characters: {}",
                        resolved.display()
                    )),
                )
            })
    }

    /// Return the workspace root as a UTF-8 string.
    ///
    /// # Errors
    ///
    /// Returns `CallToolError` when the workspace root path contains non-UTF-8 characters.
    fn workspace_root_str(&self) -> std::result::Result<String, CallToolError> {
        self.workspace_root
            .to_str()
            .map(std::string::ToString::to_string)
            .ok_or_else(|| {
                CallToolError::invalid_arguments(
                    "workspace_root",
                    Some(format!(
                        "workspace root path contains non-UTF-8 characters: {}",
                        self.workspace_root.display()
                    )),
                )
            })
    }

    /// Extract authorization token from `_meta.authorization` in the call params.
    ///
    /// MCP clients using stdio transport pass authentication via the `_meta`
    /// field since HTTP headers are not available at the handler level.
    fn extract_authorization(params: &CallToolRequestParams) -> Option<String> {
        params
            .meta
            .as_ref()
            .and_then(|m| m.extra.as_ref())
            .and_then(|extra| extra.get("authorization"))
            .and_then(|v| v.as_str())
            .map(std::string::ToString::to_string)
    }

    /// Resolve the caller's Authorization value from the two supported
    /// channels: an explicit `_meta.authorization` always wins; otherwise
    /// fall back to the HTTP `Authorization: Bearer` header the transport
    /// captured onto the session runtime (see `server::HeaderCaptureProvider`).
    /// Stdio runtimes carry no header and yield `None`.
    async fn resolve_authorization(
        meta_authorization: Option<String>,
        runtime: &Arc<dyn McpServer>,
    ) -> Option<String> {
        if meta_authorization.is_some() {
            return meta_authorization;
        }
        runtime
            .auth_info_cloned()
            .await
            .map(|info| format!("Bearer {}", info.token_unique_id))
    }
}
