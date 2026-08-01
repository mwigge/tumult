use super::*;
use crate::handler::test_support::stub_runtime;
use crate::handler::{McpAuth, TumultHandler};
use rust_mcp_sdk::mcp_server::ServerHandler;
use rust_mcp_sdk::schema::{
    PaginatedMeta, PaginatedRequestParams, ReadResourceContent, ReadResourceMeta,
    ReadResourceRequestParams,
};

fn open_handler(root: &std::path::Path) -> TumultHandler {
    TumultHandler::with_auth(root.to_path_buf(), McpAuth::none())
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
        .handle_read_resource_request(read_params("tumult://journal/journal.toon"), stub_runtime())
        .await
        .expect("journal read must succeed");
    let (text, mime) = contents_text(&result);
    assert_eq!(mime, Some("application/json"));

    let tool = crate::tools::read_journal(journal_path.to_str().unwrap(), "json", false).unwrap();
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
        .handle_read_resource_request(read_params("tumult://experiment/test.toon"), stub_runtime())
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
        .handle_read_resource_request(read_params("tumult://journal/journal.toon"), stub_runtime())
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
        .handle_read_resource_request(read_params("tumult://journal/absent.toon"), stub_runtime())
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
        McpAuth::single_operator("resource-secret".into()),
    );

    // No token: both entry points must reject.
    let err = handler
        .handle_list_resources_request(None, stub_runtime())
        .await
        .expect_err("list without token must be rejected");
    assert!(err.message.contains("Unauthorized"), "got: {err}");
    let err = handler
        .handle_read_resource_request(read_params("tumult://journal/journal.toon"), stub_runtime())
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
