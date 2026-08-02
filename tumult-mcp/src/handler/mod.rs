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

/// MCP request handler: lists the tool surface and dispatches `tools/call`
/// requests to the per-family dispatch bodies, enforcing rate limiting,
/// bearer-token auth, role gating, and a concurrency cap along the way.
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

#[cfg(test)]
mod tests {
    use super::*;
    use rust_mcp_sdk::schema::CallToolMeta;

    #[test]
    fn default_handler_allows_full_concurrency_from_a_workspace_root() {
        let handler = TumultHandler::default();
        assert_eq!(
            handler.semaphore.available_permits(),
            MAX_CONCURRENT_TOOL_CALLS
        );
        assert!(
            !handler.workspace_root.as_os_str().is_empty(),
            "the default workspace root falls back to the current directory"
        );
    }

    #[test]
    fn resolve_path_accepts_contained_files_and_rejects_escapes() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("inside.toon"), "x").unwrap();
        let handler = TumultHandler::with_auth(tmp.path().to_path_buf(), McpAuth::none());

        let resolved = handler.resolve_path("inside.toon").unwrap();
        assert!(resolved.ends_with("inside.toon"), "{resolved}");
        assert_eq!(
            handler.workspace_root_str().unwrap(),
            tmp.path().to_str().unwrap()
        );

        let err = handler
            .resolve_path("../escape.toon")
            .expect_err("a path outside the workspace must be rejected");
        assert!(err.to_string().contains("path"), "got: {err}");

        let err = handler
            .resolve_path("no-such-file.toon")
            .expect_err("a non-existent file cannot be canonicalized");
        assert!(err.to_string().contains("path"), "got: {err}");
    }

    #[test]
    fn extract_authorization_reads_only_meta_extra() {
        let without_meta = CallToolRequestParams {
            name: "tumult_whoami".into(),
            arguments: None,
            meta: None,
            task: None,
        };
        assert_eq!(TumultHandler::extract_authorization(&without_meta), None);

        let mut extra = serde_json::Map::new();
        extra.insert(
            "authorization".into(),
            serde_json::Value::String("Bearer tok".into()),
        );
        let with_meta = CallToolRequestParams {
            meta: Some(CallToolMeta {
                progress_token: None,
                extra: Some(extra),
            }),
            ..without_meta
        };
        assert_eq!(
            TumultHandler::extract_authorization(&with_meta).as_deref(),
            Some("Bearer tok")
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resolve_authorization_prefers_meta_then_the_captured_header() {
        let runtime = crate::handler::test_support::stub_runtime_with_bearer("http-tok");
        // An explicit `_meta.authorization` always wins.
        let resolved =
            TumultHandler::resolve_authorization(Some("Bearer meta-tok".into()), &runtime).await;
        assert_eq!(resolved.as_deref(), Some("Bearer meta-tok"));
        // Otherwise fall back to the HTTP header captured on the session.
        let resolved = TumultHandler::resolve_authorization(None, &runtime).await;
        assert_eq!(resolved.as_deref(), Some("Bearer http-tok"));
        // A stdio session carries no header at all.
        let runtime = crate::handler::test_support::stub_runtime();
        assert_eq!(
            TumultHandler::resolve_authorization(None, &runtime).await,
            None
        );
    }
}
