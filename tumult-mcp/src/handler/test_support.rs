//! Shared test doubles for handler round-trip tests.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rust_mcp_sdk::auth::AuthInfo;
use rust_mcp_sdk::error::SdkResult;
use rust_mcp_sdk::schema::{
    ClientMessage, Implementation, InitializeRequestParams, InitializeResult, MessageFromServer,
    ProtocolVersion, RequestId, ServerCapabilities, ServerMessage,
};
use rust_mcp_sdk::task_store::{ClientTaskStore, ServerTaskStore};
use rust_mcp_sdk::{McpServer, SessionId};

/// Minimal `McpServer` runtime stub. The handlers under test never touch
/// the runtime, so every method is inert; `server_info` returns a static
/// placeholder.
pub(crate) struct StubMcpServer {
    details: InitializeResult,
    auth_info: tokio::sync::RwLock<Option<AuthInfo>>,
}

impl StubMcpServer {
    fn with_auth_info(auth_info: Option<AuthInfo>) -> Self {
        Self {
            details: InitializeResult {
                capabilities: ServerCapabilities::default(),
                instructions: None,
                meta: None,
                protocol_version: ProtocolVersion::V2025_11_25.into(),
                server_info: Implementation {
                    name: "stub".into(),
                    version: "0.0.0".into(),
                    title: None,
                    description: None,
                    icons: vec![],
                    website_url: None,
                },
            },
            auth_info: tokio::sync::RwLock::new(auth_info),
        }
    }

    /// An inert stub with no captured auth info (the stdio case).
    pub(crate) fn new() -> Self {
        Self::with_auth_info(None)
    }

    /// A stub carrying transport-captured auth info, simulating an HTTP
    /// session whose `Authorization: Bearer` header the middleware stashed.
    pub(crate) fn with_bearer_token(token: &str) -> Self {
        Self::with_auth_info(Some(AuthInfo {
            token_unique_id: token.to_string(),
            client_id: None,
            user_id: None,
            scopes: None,
            expires_at: None,
            audience: None,
            extra: None,
        }))
    }
}

#[async_trait]
impl McpServer for StubMcpServer {
    async fn start(self: Arc<Self>) -> SdkResult<()> {
        Ok(())
    }

    async fn set_client_details(&self, _client_details: InitializeRequestParams) -> SdkResult<()> {
        Ok(())
    }

    fn server_info(&self) -> &InitializeResult {
        &self.details
    }

    fn client_info(&self) -> Option<InitializeRequestParams> {
        None
    }

    async fn auth_info(&self) -> tokio::sync::RwLockReadGuard<'_, Option<AuthInfo>> {
        self.auth_info.read().await
    }

    async fn auth_info_cloned(&self) -> Option<AuthInfo> {
        self.auth_info.read().await.clone()
    }

    async fn update_auth_info(&self, auth_info: Option<AuthInfo>) {
        *self.auth_info.write().await = auth_info;
    }

    async fn wait_for_initialization(&self) {}

    fn task_store(&self) -> Option<Arc<ServerTaskStore>> {
        None
    }

    fn client_task_store(&self) -> Option<Arc<ClientTaskStore>> {
        None
    }

    async fn stderr_message(&self, _message: String) -> SdkResult<()> {
        Ok(())
    }

    fn session_id(&self) -> Option<SessionId> {
        None
    }

    async fn send(
        &self,
        _message: MessageFromServer,
        _request_id: Option<RequestId>,
        _request_timeout: Option<Duration>,
    ) -> SdkResult<Option<ClientMessage>> {
        Ok(None)
    }

    async fn send_batch(
        &self,
        _messages: Vec<ServerMessage>,
        _request_timeout: Option<Duration>,
    ) -> SdkResult<Option<Vec<ClientMessage>>> {
        Ok(None)
    }
}

/// Fresh inert runtime for driving handler entry points in tests.
pub(crate) fn stub_runtime() -> Arc<dyn McpServer> {
    Arc::new(StubMcpServer::new())
}

/// Runtime stub whose session carries the given bearer token as captured
/// `AuthInfo` (the HTTP header channel).
pub(crate) fn stub_runtime_with_bearer(token: &str) -> Arc<dyn McpServer> {
    Arc::new(StubMcpServer::with_bearer_token(token))
}

/// Serializes tests that mutate the process-wide MCP auth environment
/// (`TUMULT_MCP_AUTH_CONFIG` / `TUMULT_MCP_TOKEN`): `McpAuth::load` and the
/// `Cli → ServeOptions` conversion read those variables, so concurrent
/// mutation by parallel tests would be racy.
pub(crate) static AUTH_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
