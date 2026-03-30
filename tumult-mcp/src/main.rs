//! Tumult MCP Server — Exposes Tumult chaos engineering as MCP tools.

use async_trait::async_trait;
use rust_mcp_sdk::{
    error::SdkResult,
    macros,
    mcp_server::{server_runtime, McpServerOptions, ServerHandler},
    schema::*,
    *,
};
use std::sync::Arc;

#[macros::mcp_tool(
    name = "tumult_discover",
    description = "List all discovered Tumult plugins, actions, and probes."
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct DiscoverTool {}

#[derive(Default)]
struct TumultHandler;

#[async_trait]
impl ServerHandler for TumultHandler {
    async fn handle_list_tools_request(
        &self,
        _params: Option<PaginatedRequestParams>,
        _runtime: Arc<dyn McpServer>,
    ) -> std::result::Result<ListToolsResult, RpcError> {
        Ok(ListToolsResult {
            tools: vec![DiscoverTool::tool()],
            meta: None,
            next_cursor: None,
        })
    }

    async fn handle_call_tool_request(
        &self,
        params: CallToolRequestParams,
        _runtime: Arc<dyn McpServer>,
    ) -> std::result::Result<CallToolResult, CallToolError> {
        match params.name.as_str() {
            "tumult_discover" => Ok(CallToolResult::text_content(vec![
                "Tumult plugins: (none discovered)".into(),
            ])),
            _ => Err(CallToolError::unknown_tool(params.name)),
        }
    }
}

#[tokio::main]
async fn main() -> SdkResult<()> {
    let server_details = InitializeResult {
        server_info: Implementation {
            name: "tumult-mcp".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            title: Some("Tumult Chaos Engineering MCP Server".into()),
            description: Some("Run chaos experiments, analyze journals, discover plugins".into()),
            icons: vec![],
            website_url: Some("https://github.com/mwigge/tumult".into()),
        },
        capabilities: ServerCapabilities {
            tools: Some(ServerCapabilitiesTools { list_changed: None }),
            ..Default::default()
        },
        protocol_version: ProtocolVersion::V2025_11_25.into(),
        instructions: Some("Tumult is a Rust-native chaos engineering platform.".into()),
        meta: None,
    };

    let transport = StdioTransport::new(TransportOptions::default())?;
    let handler = TumultHandler.to_mcp_server_handler();
    let server = server_runtime::create_server(McpServerOptions {
        transport,
        handler,
        server_details,
        task_store: None,
        client_task_store: None,
        message_observer: None,
    });
    server.start().await
}
