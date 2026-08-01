use super::*;
use crate::handler::{McpAuth, Role, TumultHandler, MAX_CONCURRENT_TOOL_CALLS};

mod dispatch_roundtrip;
mod meta;
mod resource_links;

use rust_mcp_sdk::schema::CallToolMeta;

use crate::handler::test_support::{stub_runtime, stub_runtime_with_bearer};

/// Build `CallToolRequestParams` from a tool name, JSON arguments, and an
/// optional `_meta.authorization` value (how stdio clients pass the bearer).
fn call_params(
    name: &str,
    arguments: serde_json::Value,
    authorization: Option<&str>,
) -> CallToolRequestParams {
    let arguments = match arguments {
        serde_json::Value::Object(map) => Some(map),
        _ => None,
    };
    let meta = authorization.map(|auth| {
        let mut extra = serde_json::Map::new();
        extra.insert(
            "authorization".into(),
            serde_json::Value::String(auth.into()),
        );
        CallToolMeta {
            progress_token: None,
            extra: Some(extra),
        }
    });
    CallToolRequestParams {
        name: name.into(),
        arguments,
        meta,
        task: None,
    }
}

/// Concatenate all text content blocks of a `CallToolResult`
/// (`resource_link` blocks may follow the text).
fn result_text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::TextContent(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Collect the `resource_link` content blocks of a `CallToolResult`.
fn result_links(result: &CallToolResult) -> Vec<&ResourceLink> {
    result
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ResourceLink(link) => Some(link),
            _ => None,
        })
        .collect()
}

/// Handler with no auth token, rooted at the given directory.
fn open_handler(root: &std::path::Path) -> TumultHandler {
    TumultHandler::with_auth(root.to_path_buf(), McpAuth::none())
}

/// Assert `structured` conforms to the schema advertised for `tool_name`:
/// every required property is present and no undeclared keys appear.
fn assert_conforms(tool_name: &str, structured: &serde_json::Map<String, serde_json::Value>) {
    let schema = output_schema_for(tool_name)
        .unwrap_or_else(|| panic!("'{tool_name}' must advertise an output schema"));
    let properties = schema.properties.clone().unwrap_or_default();
    for required in &schema.required {
        assert!(
            structured.contains_key(required),
            "'{tool_name}' structured content missing required property '{required}'"
        );
    }
    for key in structured.keys() {
        assert!(
            properties.contains_key(key),
            "'{tool_name}' structured content has undeclared property '{key}'"
        );
    }
}
