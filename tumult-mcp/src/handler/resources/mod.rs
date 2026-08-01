//! MCP resources — workspace journals, experiments, and `GameDay` files.
//!
//! URI scheme (filenames only; no path separators, traversal rejected):
//!
//! - `tumult://journal/{filename}` — journal `.toon` files in the workspace
//!   root, read as the same JSON object `tumult_read_journal` returns
//!   (`{summary, journal}`; over 512 KiB the summary shape plus a note).
//! - `tumult://experiment/{filename}` — experiment `.toon` definitions,
//!   read as raw TOON text.
//! - `tumult://gameday/{filename}` — `.gameday.toon` campaign files, read
//!   as raw TOON text.
//!
//! `resources/list` enumerates the workspace root (flat, matching the URI
//! scheme's filename-only addressing) and paginates with an opaque base64
//! cursor over the sorted entry offset.

mod cursor;
mod links;
mod uri;

use std::path::Path;

use rust_mcp_sdk::schema::{
    ListResourcesResult, ReadResourceResult, Resource, ResourceLink, RpcError, TextResourceContents,
};

use crate::tools;

use super::TumultHandler;
use cursor::{decode_cursor, encode_cursor};
use uri::{parse_resource_uri, URI_PREFIX};

pub(crate) use links::{file_resource_link, workspace_resource_link};
pub(crate) use uri::{classify, ResourceKind};

/// Page size for `resources/list`.
pub(crate) const RESOURCES_PAGE_SIZE: usize = 100;

/// Maximum number of `resource_link` content items attached to a single
/// `tumult_list_journals` result.
pub(crate) const RESOURCE_LINKS_MAX: usize = 50;

// ── Handler logic (called from the ServerHandler impl) ────────

impl TumultHandler {
    /// Enforce the same bearer-token gate as tool calls for resource
    /// requests; the token travels in the request `_meta` extra fields or,
    /// on the HTTP transport, in the `Authorization` header.
    pub(crate) fn check_resource_auth(&self, authorization: Option<&str>) -> Result<(), RpcError> {
        self.auth
            .check(authorization)
            .map_err(|e| RpcError::invalid_request().with_message(format!("Unauthorized: {e}")))
    }

    /// One page of workspace resources, starting at the offset encoded in
    /// `cursor` (or the beginning when absent).
    pub(crate) fn list_resources_page(
        &self,
        cursor: Option<&str>,
    ) -> Result<ListResourcesResult, RpcError> {
        let offset = cursor.map(decode_cursor).transpose()?.unwrap_or(0);
        let all = self.enumerate_workspace_resources()?;
        let total = all.len();
        let end = total.min(offset.saturating_add(RESOURCES_PAGE_SIZE));
        let resources: Vec<Resource> = if offset >= total {
            Vec::new()
        } else {
            all.into_iter().take(end).skip(offset).collect()
        };
        Ok(ListResourcesResult {
            meta: None,
            next_cursor: (end < total).then(|| encode_cursor(end)),
            resources,
        })
    }

    /// Enumerate every `.toon` file directly in the workspace root (the
    /// same flat listing `tumult_list_journals` produces for the root),
    /// classified into journal/experiment/gameday resources and sorted by
    /// URI for stable pagination.
    fn enumerate_workspace_resources(&self) -> Result<Vec<Resource>, RpcError> {
        let read_dir = std::fs::read_dir(&self.workspace_root).map_err(|e| {
            RpcError::internal_error().with_message(format!("cannot read workspace root: {e}"))
        })?;
        let mut resources = Vec::new();
        for entry in read_dir.flatten() {
            let path = entry.path();
            if !path.is_file() || path.extension().and_then(|e| e.to_str()) != Some("toon") {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let kind = classify(&path);
            resources.push(Resource {
                uri: format!("{URI_PREFIX}{}/{name}", kind.uri_kind()),
                name: name.to_string(),
                title: None,
                description: Some(kind.description().to_string()),
                mime_type: Some(kind.mime_type().to_string()),
                size: entry
                    .metadata()
                    .ok()
                    .and_then(|m| i64::try_from(m.len()).ok()),
                annotations: None,
                icons: vec![],
                meta: None,
            });
        }
        resources.sort_by(|a, b| a.uri.cmp(&b.uri));
        Ok(resources)
    }

    /// Read one resource by `tumult://` URI. Journals come back as the
    /// `tumult_read_journal` JSON object; experiments and gamedays as raw
    /// file text. Everything else is a protocol error.
    pub(crate) fn read_resource_uri(&self, uri: &str) -> Result<ReadResourceResult, RpcError> {
        let (kind, name) = parse_resource_uri(uri)?;
        let resolved = tools::safe_resolve_path(&self.workspace_root, name).map_err(|e| {
            RpcError::invalid_params().with_message(format!("resource not found: {e}"))
        })?;
        let text = match kind {
            ResourceKind::Journal => journal_json_text(&resolved)?,
            ResourceKind::Experiment | ResourceKind::Gameday => std::fs::read_to_string(&resolved)
                .map_err(|e| {
                    RpcError::invalid_params().with_message(format!("resource not readable: {e}"))
                })?,
        };
        Ok(ReadResourceResult {
            meta: None,
            contents: vec![TextResourceContents {
                meta: None,
                mime_type: Some(kind.mime_type().to_string()),
                text,
                uri: uri.to_string(),
            }
            .into()],
        })
    }

    /// `resource_link`s for the journal paths in a `tumult_list_journals`
    /// structured result, capped at [`RESOURCE_LINKS_MAX`].
    pub(crate) fn journal_page_links(
        &self,
        structured: &serde_json::Map<String, serde_json::Value>,
    ) -> Vec<ResourceLink> {
        structured
            .get("items")
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .take(RESOURCE_LINKS_MAX)
                    .map(|item| {
                        let path = Path::new(item);
                        workspace_resource_link(&self.workspace_root, classify(path), path)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Render a journal file the way `tumult_read_journal` (format=json) does:
/// a pretty `{summary, journal}` object. When that JSON exceeds the 512 KiB
/// text cap, return the summary shape plus an explanatory note instead —
/// truncated JSON would not parse.
fn journal_json_text(path: &Path) -> Result<String, RpcError> {
    let path_str = path.to_str().ok_or_else(|| {
        RpcError::internal_error().with_message("resource path contains non-UTF-8 characters")
    })?;
    let report = tools::read_journal(path_str, "json", false).map_err(|e| {
        RpcError::invalid_params().with_message(format!("not a readable journal resource: {e}"))
    })?;
    let internal = |e: serde_json::Error| RpcError::internal_error().with_message(e.to_string());
    let full = serde_json::to_string_pretty(&serde_json::Value::Object(report.structured.clone()))
        .map_err(internal)?;
    if full.len() <= tools::MAX_TEXT_BYTES {
        return Ok(full);
    }
    let mut summary = serde_json::Map::new();
    if let Some(value) = report.structured.get("summary") {
        summary.insert("summary".into(), value.clone());
    }
    summary.insert(
        "note".into(),
        serde_json::Value::String(format!(
            "full journal JSON is {} bytes, over the 512 KiB resource cap; returning the \
             summary only — use the tumult_read_journal tool (summary=true) or read the raw file",
            full.len()
        )),
    );
    serde_json::to_string_pretty(&serde_json::Value::Object(summary)).map_err(internal)
}

#[cfg(test)]
mod tests;
