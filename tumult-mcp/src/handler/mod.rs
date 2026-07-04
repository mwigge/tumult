//! MCP handler — routes tool calls to implementations.

mod auth;
mod dispatch;
mod executor;
mod output_schema;
mod resources;
mod schema;
#[cfg(test)]
pub(crate) mod test_support;

pub use auth::{mcp_bind_address, McpAuth};
pub use executor::ProcessExecutor;
pub use schema::*;

use rust_mcp_sdk::schema::{CallToolError, CallToolRequestParams};

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
}

impl Default for TumultHandler {
    fn default() -> Self {
        Self {
            semaphore: tokio::sync::Semaphore::new(MAX_CONCURRENT_TOOL_CALLS),
            workspace_root: std::env::current_dir().unwrap_or_else(|_| "/".into()),
            auth: McpAuth::from_env(),
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
        }
    }

    /// Create a handler with a specific workspace root and authentication config.
    #[must_use]
    pub fn with_auth(workspace_root: std::path::PathBuf, auth: McpAuth) -> Self {
        Self {
            semaphore: tokio::sync::Semaphore::new(MAX_CONCURRENT_TOOL_CALLS),
            workspace_root,
            auth,
        }
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
}
