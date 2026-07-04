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

use std::path::Path;

use rust_mcp_sdk::schema::{
    ListResourcesResult, ReadResourceResult, Resource, ResourceLink, RpcError, TextResourceContents,
};

use crate::tools;

use super::TumultHandler;

/// Page size for `resources/list`.
pub(crate) const RESOURCES_PAGE_SIZE: usize = 100;

/// Maximum number of `resource_link` content items attached to a single
/// `tumult_list_journals` result.
pub(crate) const RESOURCE_LINKS_MAX: usize = 50;

/// URI prefix shared by every Tumult resource.
const URI_PREFIX: &str = "tumult://";

/// Kind of a workspace resource, deciding its URI scheme tail and MIME type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResourceKind {
    /// Experiment journal — served as JSON (`tumult://journal/…`).
    Journal,
    /// Experiment definition — served as raw TOON (`tumult://experiment/…`).
    Experiment,
    /// `GameDay` campaign — served as raw TOON (`tumult://gameday/…`).
    Gameday,
}

impl ResourceKind {
    /// URI path segment between `tumult://` and the file name.
    fn uri_kind(self) -> &'static str {
        match self {
            Self::Journal => "journal",
            Self::Experiment => "experiment",
            Self::Gameday => "gameday",
        }
    }

    /// MIME type of the content `resources/read` returns for this kind:
    /// journals are rendered as JSON, everything else is raw TOON text.
    fn mime_type(self) -> &'static str {
        match self {
            Self::Journal => "application/json",
            Self::Experiment | Self::Gameday => "application/toon",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Journal => "Tumult experiment journal (read as JSON: {summary, journal}).",
            Self::Experiment => "Tumult chaos experiment definition (raw TOON).",
            Self::Gameday => "Tumult GameDay campaign definition (raw TOON).",
        }
    }
}

/// Classify a workspace `.toon` file. `.gameday.toon` wins on the file
/// name; otherwise journals are recognized by their `experiment_title:`
/// field before experiments are recognized by `title:` (a journal may embed
/// nested hypothesis `title:` lines), and unreadable/ambiguous files fall
/// back to journal — mirroring `tumult_list_journals`, which lists every
/// `.toon` file.
pub(crate) fn classify(path: &Path) -> ResourceKind {
    if path
        .file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.ends_with(".gameday"))
    {
        return ResourceKind::Gameday;
    }
    let Ok(content) = std::fs::read_to_string(path) else {
        return ResourceKind::Journal;
    };
    if content
        .lines()
        .any(|line| line.trim_start().starts_with("experiment_title:"))
    {
        return ResourceKind::Journal;
    }
    if tools::extract_title(&content).is_some() {
        return ResourceKind::Experiment;
    }
    ResourceKind::Journal
}

/// Parse a `tumult://{kind}/{filename}` URI. The filename tail must be a
/// plain `.toon` file name — separators, traversal components, and unknown
/// kinds/schemes are protocol errors.
fn parse_resource_uri(uri: &str) -> Result<(ResourceKind, &str), RpcError> {
    let invalid =
        |message: String| RpcError::invalid_params().with_message(format!("{message}: {uri}"));
    let rest = uri
        .strip_prefix(URI_PREFIX)
        .ok_or_else(|| invalid("unsupported resource URI scheme".into()))?;
    let (kind, name) = rest
        .split_once('/')
        .ok_or_else(|| invalid("malformed resource URI".into()))?;
    let kind = match kind {
        "journal" => ResourceKind::Journal,
        "experiment" => ResourceKind::Experiment,
        "gameday" => ResourceKind::Gameday,
        other => return Err(invalid(format!("unknown resource kind '{other}'"))),
    };
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
        return Err(invalid(
            "resource name must be a plain file name (no path separators or traversal)".into(),
        ));
    }
    match kind {
        ResourceKind::Gameday if !name.ends_with(".gameday.toon") => Err(invalid(
            "gameday resource names must end with .gameday.toon".into(),
        )),
        ResourceKind::Journal | ResourceKind::Experiment if !name.ends_with(".toon") => {
            Err(invalid("resource names must end with .toon".into()))
        }
        _ => Ok((kind, name)),
    }
}

// ── Pagination cursor (opaque base64 of the offset) ───────────

const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode a `resources/list` offset as an opaque base64 cursor.
fn encode_cursor(offset: usize) -> String {
    let digits = offset.to_string().into_bytes();
    let mut out = String::with_capacity(digits.len().div_ceil(3) * 4);
    for chunk in digits.chunks(3) {
        let b1 = chunk.get(1).copied().map(usize::from);
        let b2 = chunk.get(2).copied().map(usize::from);
        let group = (usize::from(chunk[0]) << 16) | (b1.unwrap_or(0) << 8) | b2.unwrap_or(0);
        out.push(char::from(BASE64_ALPHABET[(group >> 18) & 63]));
        out.push(char::from(BASE64_ALPHABET[(group >> 12) & 63]));
        out.push(if b1.is_some() {
            char::from(BASE64_ALPHABET[(group >> 6) & 63])
        } else {
            '='
        });
        out.push(if b2.is_some() {
            char::from(BASE64_ALPHABET[group & 63])
        } else {
            '='
        });
    }
    out
}

/// Decode an opaque cursor back to an offset.
///
/// # Errors
///
/// Returns an invalid-params [`RpcError`] for anything that is not the
/// base64 encoding of a decimal offset (per the MCP spec, invalid cursors
/// are protocol errors).
fn decode_cursor(cursor: &str) -> Result<usize, RpcError> {
    let invalid =
        || RpcError::invalid_params().with_message(format!("invalid pagination cursor: {cursor}"));
    if cursor.is_empty() || !cursor.len().is_multiple_of(4) {
        return Err(invalid());
    }
    let trimmed = cursor.trim_end_matches('=');
    if cursor.len() - trimmed.len() > 2 {
        return Err(invalid());
    }
    let mut bits: usize = 0;
    let mut bit_count: u32 = 0;
    let mut digits: Vec<u8> = Vec::with_capacity(cursor.len() / 4 * 3);
    for byte in trimmed.bytes() {
        let value = BASE64_ALPHABET
            .iter()
            .position(|&b| b == byte)
            .ok_or_else(invalid)?;
        bits = (bits << 6) | value;
        bit_count += 6;
        if bit_count >= 8 {
            bit_count -= 8;
            let out = u8::try_from((bits >> bit_count) & 0xFF).map_err(|_| invalid())?;
            digits.push(out);
            bits &= (1 << bit_count) - 1;
        }
    }
    if bits != 0 {
        // Non-canonical padding bits.
        return Err(invalid());
    }
    let text = String::from_utf8(digits).map_err(|_| invalid())?;
    text.parse::<usize>().map_err(|_| invalid())
}

// ── Resource link builders (tool results) ─────────────────────

/// Build a `resource_link` for a workspace file. Files directly in the
/// workspace root get a readable `tumult://` URI; anything else (e.g. a
/// journal written into a subdirectory) falls back to a `file://` link.
pub(crate) fn workspace_resource_link(
    workspace_root: &Path,
    kind: ResourceKind,
    path: &Path,
) -> ResourceLink {
    let in_root = path
        .parent()
        .and_then(|parent| parent.canonicalize().ok())
        .zip(workspace_root.canonicalize().ok())
        .is_some_and(|(parent, root)| parent == root);
    let name = path.file_name().and_then(|n| n.to_str());
    match (in_root, name) {
        (true, Some(name)) => ResourceLink::new(
            vec![],
            name.to_string(),
            format!("{URI_PREFIX}{}/{name}", kind.uri_kind()),
            None,
            Some(kind.description().to_string()),
            None,
            Some(kind.mime_type().to_string()),
            None,
            None,
        ),
        _ => file_resource_link(path),
    }
}

/// Build a `file://` `resource_link` for a written file that has no
/// `tumult://` scheme (e.g. reports) or lives outside the workspace root.
pub(crate) fn file_resource_link(path: &Path) -> ResourceLink {
    let mime = match path.extension().and_then(|e| e.to_str()) {
        Some("json") => "application/json",
        Some("xml") => "application/xml",
        Some("toon") => "application/toon",
        _ => "text/plain",
    };
    ResourceLink::new(
        vec![],
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("resource")
            .to_string(),
        format!("file://{}", path.display()),
        None,
        None,
        None,
        Some(mime.to_string()),
        None,
        None,
    )
}

// ── Handler logic (called from the ServerHandler impl) ────────

impl TumultHandler {
    /// Enforce the same bearer-token gate as tool calls for resource
    /// requests; the token travels in the request `_meta` extra fields.
    pub(crate) fn check_resource_auth(
        &self,
        extra: Option<&serde_json::Map<String, serde_json::Value>>,
    ) -> Result<(), RpcError> {
        let authorization = extra
            .and_then(|extra| extra.get("authorization"))
            .and_then(serde_json::Value::as_str);
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
mod tests {
    use super::*;
    use crate::handler::test_support::stub_runtime;
    use crate::handler::{McpAuth, TumultHandler};
    use rust_mcp_sdk::mcp_server::ServerHandler;
    use rust_mcp_sdk::schema::{
        PaginatedMeta, PaginatedRequestParams, ReadResourceContent, ReadResourceMeta,
        ReadResourceRequestParams,
    };

    fn open_handler(root: &std::path::Path) -> TumultHandler {
        TumultHandler::with_auth(root.to_path_buf(), McpAuth { token: None })
    }

    fn read_params(uri: &str) -> ReadResourceRequestParams {
        ReadResourceRequestParams {
            meta: None,
            uri: uri.into(),
        }
    }

    fn contents_text(result: &rust_mcp_sdk::schema::ReadResourceResult) -> (&str, Option<&str>) {
        assert_eq!(result.contents.len(), 1, "exactly one contents item");
        match &result.contents[0] {
            ReadResourceContent::TextResourceContents(text) => {
                (text.text.as_str(), text.mime_type.as_deref())
            }
            ReadResourceContent::BlobResourceContents(_) => panic!("expected text contents"),
        }
    }

    // ── cursor codec ──────────────────────────────────────────

    #[test]
    fn cursor_round_trips_offsets() {
        for offset in [0usize, 1, 99, 100, 12345, usize::MAX / 2] {
            let cursor = encode_cursor(offset);
            assert_eq!(decode_cursor(&cursor).unwrap(), offset, "cursor {cursor}");
        }
    }

    #[test]
    fn cursor_rejects_invalid_input() {
        for bad in ["", "!!!!", "AAAA", "abc", "MTAw=", "====", "not base64"] {
            assert!(
                decode_cursor(bad).is_err(),
                "cursor {bad:?} must be rejected"
            );
        }
    }

    // ── URI parsing ───────────────────────────────────────────

    #[test]
    fn parse_uri_accepts_plain_names_and_rejects_everything_else() {
        assert_eq!(
            parse_resource_uri("tumult://journal/a.toon").unwrap(),
            (ResourceKind::Journal, "a.toon")
        );
        assert_eq!(
            parse_resource_uri("tumult://gameday/x.gameday.toon").unwrap(),
            (ResourceKind::Gameday, "x.gameday.toon")
        );
        for bad in [
            "file:///etc/passwd",
            "tumult://journal",
            "tumult://journal/",
            "tumult://journal/..",
            "tumult://journal/../escape",
            "tumult://journal/sub/a.toon",
            "tumult://journal/a\\b.toon",
            "tumult://journal/passwd",
            "tumult://gameday/plain.toon",
            "tumult://prompt/a.toon",
        ] {
            assert!(parse_resource_uri(bad).is_err(), "{bad} must be rejected");
        }
    }

    // ── resources/list ────────────────────────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn list_resources_empty_workspace_returns_no_resources() {
        let tmp = tempfile::tempdir().unwrap();
        let handler = open_handler(tmp.path());
        let result = handler
            .handle_list_resources_request(None, stub_runtime())
            .await
            .expect("list_resources must succeed");
        assert!(result.resources.is_empty());
        assert!(result.next_cursor.is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn list_resources_classifies_kinds_with_consistent_mime_types() {
        let tmp = tempfile::tempdir().unwrap();
        // journal.toon (journal) + test.toon (experiment definition).
        crate::tools::test_support::write_run_journal(tmp.path());
        std::fs::write(
            tmp.path().join("drill.gameday.toon"),
            "title: Drill\nexperiments[1]:\n  - path: test.toon\n",
        )
        .unwrap();
        let handler = open_handler(tmp.path());

        let result = handler
            .handle_list_resources_request(None, stub_runtime())
            .await
            .unwrap();
        let by_uri: std::collections::HashMap<&str, &rust_mcp_sdk::schema::Resource> = result
            .resources
            .iter()
            .map(|r| (r.uri.as_str(), r))
            .collect();
        assert_eq!(result.resources.len(), 3, "uris: {:?}", by_uri.keys());

        let journal = by_uri["tumult://journal/journal.toon"];
        assert_eq!(journal.mime_type.as_deref(), Some("application/json"));
        assert_eq!(journal.name, "journal.toon");
        assert!(journal.size.is_some_and(|s| s > 0));

        let experiment = by_uri["tumult://experiment/test.toon"];
        assert_eq!(experiment.mime_type.as_deref(), Some("application/toon"));

        let gameday = by_uri["tumult://gameday/drill.gameday.toon"];
        assert_eq!(gameday.mime_type.as_deref(), Some("application/toon"));

        // mimeType consistency: reading each listed resource returns the
        // same MIME type the listing advertised.
        for resource in &result.resources {
            let read = handler
                .handle_read_resource_request(read_params(&resource.uri), stub_runtime())
                .await
                .unwrap_or_else(|e| panic!("read {} must succeed: {e}", resource.uri));
            let (_, mime) = contents_text(&read);
            assert_eq!(mime, resource.mime_type.as_deref(), "uri {}", resource.uri);
        }
    }

    /// Write `count` minimal experiment files named `exp-XXXX.toon`.
    fn write_many_experiments(dir: &std::path::Path, count: usize) {
        for i in 0..count {
            std::fs::write(
                dir.join(format!("exp-{i:04}.toon")),
                format!("title: Experiment {i}\n"),
            )
            .unwrap();
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn list_resources_exactly_one_page_has_no_cursor() {
        let tmp = tempfile::tempdir().unwrap();
        write_many_experiments(tmp.path(), RESOURCES_PAGE_SIZE);
        let handler = open_handler(tmp.path());

        let result = handler
            .handle_list_resources_request(None, stub_runtime())
            .await
            .unwrap();
        assert_eq!(result.resources.len(), RESOURCES_PAGE_SIZE);
        assert!(
            result.next_cursor.is_none(),
            "an exactly-full page must not advertise another page"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn list_resources_paginates_beyond_page_size() {
        let tmp = tempfile::tempdir().unwrap();
        write_many_experiments(tmp.path(), RESOURCES_PAGE_SIZE + 5);
        let handler = open_handler(tmp.path());

        let first = handler
            .handle_list_resources_request(None, stub_runtime())
            .await
            .unwrap();
        assert_eq!(first.resources.len(), RESOURCES_PAGE_SIZE);
        let cursor = first.next_cursor.clone().expect("cursor for second page");

        let second = handler
            .handle_list_resources_request(
                Some(PaginatedRequestParams {
                    cursor: Some(cursor),
                    meta: None,
                }),
                stub_runtime(),
            )
            .await
            .unwrap();
        assert_eq!(second.resources.len(), 5);
        assert!(second.next_cursor.is_none());

        // The two pages partition the sorted set with no overlap.
        let first_uris: std::collections::HashSet<&str> =
            first.resources.iter().map(|r| r.uri.as_str()).collect();
        for resource in &second.resources {
            assert!(!first_uris.contains(resource.uri.as_str()));
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn list_resources_rejects_invalid_cursor() {
        let tmp = tempfile::tempdir().unwrap();
        write_many_experiments(tmp.path(), 3);
        let handler = open_handler(tmp.path());

        let err = handler
            .handle_list_resources_request(
                Some(PaginatedRequestParams {
                    cursor: Some("not-a-cursor!!".into()),
                    meta: None,
                }),
                stub_runtime(),
            )
            .await
            .expect_err("invalid cursor must be a protocol error");
        assert!(
            err.message.contains("invalid pagination cursor"),
            "got: {err}"
        );
    }

    // ── resources/read ────────────────────────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn read_resource_journal_matches_read_journal_tool_output() {
        let tmp = tempfile::tempdir().unwrap();
        let journal_path = crate::tools::test_support::write_run_journal(tmp.path());
        let handler = open_handler(tmp.path());

        let result = handler
            .handle_read_resource_request(
                read_params("tumult://journal/journal.toon"),
                stub_runtime(),
            )
            .await
            .expect("journal read must succeed");
        let (text, mime) = contents_text(&result);
        assert_eq!(mime, Some("application/json"));

        let tool =
            crate::tools::read_journal(journal_path.to_str().unwrap(), "json", false).unwrap();
        assert_eq!(
            text, tool.text,
            "resource JSON must equal tumult_read_journal format=json output"
        );
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["summary"]["experiment_title"], "MCP test experiment");
        assert_eq!(parsed["journal"]["experiment_title"], "MCP test experiment");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn read_resource_experiment_and_gameday_return_raw_text() {
        let tmp = tempfile::tempdir().unwrap();
        let exp = crate::tools::test_support::write_valid_experiment(tmp.path());
        let raw_exp = std::fs::read_to_string(&exp).unwrap();
        let raw_gd = "title: Drill\nexperiments[1]:\n  - path: test.toon\n";
        std::fs::write(tmp.path().join("drill.gameday.toon"), raw_gd).unwrap();
        let handler = open_handler(tmp.path());

        let result = handler
            .handle_read_resource_request(
                read_params("tumult://experiment/test.toon"),
                stub_runtime(),
            )
            .await
            .unwrap();
        let (text, mime) = contents_text(&result);
        assert_eq!(text, raw_exp);
        assert_eq!(mime, Some("application/toon"));

        let result = handler
            .handle_read_resource_request(
                read_params("tumult://gameday/drill.gameday.toon"),
                stub_runtime(),
            )
            .await
            .unwrap();
        let (text, mime) = contents_text(&result);
        assert_eq!(text, raw_gd);
        assert_eq!(mime, Some("application/toon"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn read_resource_over_cap_journal_returns_summary_with_note() {
        let tmp = tempfile::tempdir().unwrap();
        let journal_path = crate::tools::test_support::write_run_journal(tmp.path());
        // Inflate the journal past the 512 KiB JSON cap.
        let mut journal = tumult_core::journal::read_journal(&journal_path).unwrap();
        journal.method_results[0].output = Some("x".repeat(2 * crate::tools::MAX_TEXT_BYTES));
        tumult_core::journal::write_journal(&journal, &journal_path).unwrap();
        let handler = open_handler(tmp.path());

        let result = handler
            .handle_read_resource_request(
                read_params("tumult://journal/journal.toon"),
                stub_runtime(),
            )
            .await
            .expect("oversized journal must degrade to a summary, not fail");
        let (text, _) = contents_text(&result);
        assert!(text.len() <= crate::tools::MAX_TEXT_BYTES);
        let parsed: serde_json::Value = serde_json::from_str(text).expect("summary must be JSON");
        assert_eq!(parsed["summary"]["experiment_title"], "MCP test experiment");
        assert!(
            parsed.get("journal").is_none(),
            "full journal must be dropped"
        );
        assert!(
            parsed["note"].as_str().unwrap().contains("512 KiB"),
            "note must explain the cap: {parsed}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn read_resource_rejects_traversal_and_unknown_uris() {
        let tmp = tempfile::tempdir().unwrap();
        crate::tools::test_support::write_run_journal(tmp.path());
        let handler = open_handler(tmp.path());

        for uri in [
            "tumult://journal/../escape",
            "tumult://journal/..",
            "tumult://experiment/../../etc/passwd",
            "file:///etc/passwd",
            "tumult://secrets/x.toon",
            "tumult://journal/etc-passwd", // no .toon extension
        ] {
            assert!(
                handler
                    .handle_read_resource_request(read_params(uri), stub_runtime())
                    .await
                    .is_err(),
                "{uri} must be rejected"
            );
        }

        // A well-formed URI for a missing file is also a protocol error.
        let err = handler
            .handle_read_resource_request(
                read_params("tumult://journal/absent.toon"),
                stub_runtime(),
            )
            .await
            .expect_err("missing resource must be a protocol error");
        assert!(err.message.contains("resource not found"), "got: {err}");
    }

    // ── auth ──────────────────────────────────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resources_enforce_bearer_auth_like_tools() {
        let tmp = tempfile::tempdir().unwrap();
        crate::tools::test_support::write_run_journal(tmp.path());
        let handler = TumultHandler::with_auth(
            tmp.path().to_path_buf(),
            McpAuth {
                token: Some("resource-secret".into()),
            },
        );

        // No token: both entry points must reject.
        let err = handler
            .handle_list_resources_request(None, stub_runtime())
            .await
            .expect_err("list without token must be rejected");
        assert!(err.message.contains("Unauthorized"), "got: {err}");
        let err = handler
            .handle_read_resource_request(
                read_params("tumult://journal/journal.toon"),
                stub_runtime(),
            )
            .await
            .expect_err("read without token must be rejected");
        assert!(err.message.contains("Unauthorized"), "got: {err}");

        // Correct bearer via _meta.authorization: both succeed.
        let mut extra = serde_json::Map::new();
        extra.insert(
            "authorization".into(),
            serde_json::Value::String("Bearer resource-secret".into()),
        );
        let list = handler
            .handle_list_resources_request(
                Some(PaginatedRequestParams {
                    cursor: None,
                    meta: Some(PaginatedMeta {
                        progress_token: None,
                        extra: Some(extra.clone()),
                    }),
                }),
                stub_runtime(),
            )
            .await
            .expect("authorized list must succeed");
        assert!(!list.resources.is_empty());
        handler
            .handle_read_resource_request(
                ReadResourceRequestParams {
                    meta: Some(ReadResourceMeta {
                        progress_token: None,
                        extra: Some(extra),
                    }),
                    uri: "tumult://journal/journal.toon".into(),
                },
                stub_runtime(),
            )
            .await
            .expect("authorized read must succeed");
    }

    // ── resource links ────────────────────────────────────────

    #[test]
    fn workspace_resource_link_uses_tumult_uri_in_root_and_file_uri_elsewhere() {
        let tmp = tempfile::tempdir().unwrap();
        let in_root = tmp.path().join("j.toon");
        std::fs::write(&in_root, "x").unwrap();
        let link = workspace_resource_link(tmp.path(), ResourceKind::Journal, &in_root);
        assert_eq!(link.uri, "tumult://journal/j.toon");
        assert_eq!(link.mime_type.as_deref(), Some("application/json"));

        let sub = tmp.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let nested = sub.join("j.toon");
        std::fs::write(&nested, "x").unwrap();
        let link = workspace_resource_link(tmp.path(), ResourceKind::Journal, &nested);
        assert!(
            link.uri.starts_with("file://"),
            "nested files fall back to file://: {}",
            link.uri
        );
    }

    #[test]
    fn file_resource_link_maps_extension_to_mime() {
        let link = file_resource_link(Path::new("/tmp/report.xml"));
        assert_eq!(link.uri, "file:///tmp/report.xml");
        assert_eq!(link.mime_type.as_deref(), Some("application/xml"));
        assert_eq!(link.name, "report.xml");
        let link = file_resource_link(Path::new("/tmp/report.json"));
        assert_eq!(link.mime_type.as_deref(), Some("application/json"));
    }
}
