use super::*;
use crate::handler::{McpAuth, Role, TumultHandler, MAX_CONCURRENT_TOOL_CALLS};

#[test]
fn all_tools_listed() {
    let tools = vec![
        RunExperimentTool::tool(),
        ValidateTool::tool(),
        AnalyzeTool::tool(),
        ReadJournalTool::tool(),
        ListJournalsTool::tool(),
        DiscoverTool::tool(),
        CreateExperimentTool::tool(),
        QueryTracesTool::tool(),
        StoreStatsTool::tool(),
        AnalyzeStoreTool::tool(),
        ListExperimentsTool::tool(),
        ReportTool::tool(),
        ComplianceTool::tool(),
        TrendTool::tool(),
        AgentsTool::tool(),
        GameDayCreateTool::tool(),
        GameDayRunTool::tool(),
        GameDayAnalyzeTool::tool(),
        GameDayListTool::tool(),
        RecommendTool::tool(),
        CoverageTool::tool(),
        AgenticListScenariosTool::tool(),
        AgenticSmokeTool::tool(),
        AgenticRunExperimentTool::tool(),
        ChaosGraphQueryTool::tool(),
        ChaosGraphNeighborsTool::tool(),
        ChaosGraphCoverageGapsTool::tool(),
        ChaosGraphCypherTool::tool(),
        FaultCatalogTool::tool(),
        ScaffoldExperimentTool::tool(),
        TopologyImportTool::tool(),
        TopologyMapTool::tool(),
        ComplianceLineageTool::tool(),
        RecommendInjectionTool::tool(),
        AutopilotRunTool::tool(),
        AutopilotStatusTool::tool(),
        AutopilotRespondTool::tool(),
        AutopilotExportTool::tool(),
        AutopilotNotifyTool::tool(),
        WhoamiTool::tool(),
    ];
    assert_eq!(tools.len(), 40);
}

#[test]
fn handler_has_semaphore_with_correct_limit() {
    let handler = TumultHandler::with_auth(
        std::env::current_dir().unwrap_or_else(|_| "/".into()),
        McpAuth::none(),
    );
    assert_eq!(
        handler.semaphore.available_permits(),
        MAX_CONCURRENT_TOOL_CALLS
    );
}

#[test]
fn tool_names_follow_convention() {
    let tools = vec![
        RunExperimentTool::tool(),
        ValidateTool::tool(),
        AnalyzeTool::tool(),
        ReadJournalTool::tool(),
        ListJournalsTool::tool(),
        DiscoverTool::tool(),
        CreateExperimentTool::tool(),
        QueryTracesTool::tool(),
        StoreStatsTool::tool(),
        AnalyzeStoreTool::tool(),
        ListExperimentsTool::tool(),
        ReportTool::tool(),
        ComplianceTool::tool(),
        TrendTool::tool(),
        AgentsTool::tool(),
        GameDayCreateTool::tool(),
        GameDayRunTool::tool(),
        GameDayAnalyzeTool::tool(),
        GameDayListTool::tool(),
        RecommendTool::tool(),
        CoverageTool::tool(),
        AgenticListScenariosTool::tool(),
        AgenticSmokeTool::tool(),
        AgenticRunExperimentTool::tool(),
        ChaosGraphQueryTool::tool(),
        ChaosGraphNeighborsTool::tool(),
        ChaosGraphCoverageGapsTool::tool(),
        ChaosGraphCypherTool::tool(),
        FaultCatalogTool::tool(),
        ScaffoldExperimentTool::tool(),
        TopologyImportTool::tool(),
        TopologyMapTool::tool(),
        ComplianceLineageTool::tool(),
        RecommendInjectionTool::tool(),
        AutopilotRunTool::tool(),
        AutopilotStatusTool::tool(),
        AutopilotRespondTool::tool(),
        AutopilotExportTool::tool(),
        AutopilotNotifyTool::tool(),
        WhoamiTool::tool(),
    ];
    for tool in &tools {
        assert!(
            tool.name.starts_with("tumult_"),
            "tool name '{}' must start with tumult_",
            tool.name
        );
    }
}

/// Verify that the handler struct carries an `auth` field.
/// This ensures authentication is structurally wired in, not just declared.
#[test]
fn auth_wired_into_handler() {
    // A handler with a configured token must carry it in the auth field.
    let handler = TumultHandler::with_auth(
        "/tmp".into(),
        McpAuth::single_operator("handler-secret".into()),
    );
    // Auth check without token should fail (token is set on handler).
    assert!(handler.auth.check(None).is_err());
    // Auth check with correct bearer should pass.
    assert!(handler.auth.check(Some("Bearer handler-secret")).is_ok());
}

/// Verify the handler accepts requests when the correct bearer token is supplied.
#[test]
fn auth_wired_accepts_valid_token() {
    let handler = TumultHandler::with_auth(
        "/tmp".into(),
        McpAuth::single_operator("valid-token-xyz".into()),
    );
    assert!(handler.auth.check(Some("Bearer valid-token-xyz")).is_ok());
    assert!(handler.auth.check(Some("Bearer wrong")).is_err());
}

#[test]
fn handler_with_workspace_root_sets_path() {
    let handler = TumultHandler::with_workspace_root("/tmp".into());
    assert_eq!(handler.workspace_root, std::path::PathBuf::from("/tmp"));
}

#[test]
fn workspace_root_str_returns_valid_path_for_utf8_root() {
    let handler = TumultHandler::with_workspace_root("/tmp".into());
    let result = handler.workspace_root_str();
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "/tmp");
}

#[test]
fn resolve_path_returns_error_for_traversal() {
    let tmp = tempfile::tempdir().unwrap();
    let handler = TumultHandler::with_workspace_root(tmp.path().to_path_buf());
    let result = handler.resolve_path("../../etc/passwd");
    assert!(result.is_err(), "path traversal must be rejected");
}

// ── ServerHandler round trips (dispatch) ─────────────────────
//
// These tests invoke `handle_list_tools_request` / `handle_call_tool_request`
// directly against a stub `McpServer` runtime — no transport, no network.

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_tools_round_trip_returns_all_registered_tools() {
    let tmp = tempfile::tempdir().unwrap();
    let handler = open_handler(tmp.path());
    let result = handler
        .handle_list_tools_request(None, stub_runtime())
        .await
        .expect("list_tools must succeed");

    // Every name dispatched in handle_call_tool_request must be listed.
    let expected = [
        "tumult_run_experiment",
        "tumult_validate",
        "tumult_analyze",
        "tumult_read_journal",
        "tumult_list_journals",
        "tumult_discover",
        "tumult_create_experiment",
        "tumult_query_traces",
        "tumult_store_stats",
        "tumult_analyze_store",
        "tumult_list_experiments",
        "tumult_report",
        "tumult_compliance",
        "tumult_trend",
        "tumult_agents",
        "tumult_gameday_create",
        "tumult_gameday_run",
        "tumult_gameday_analyze",
        "tumult_gameday_list",
        "tumult_recommend",
        "tumult_coverage",
        "tumult_agentic_list_scenarios",
        "tumult_agentic_smoke",
        "tumult_agentic_run_experiment",
        "tumult_chaosgraph_query",
        "tumult_chaosgraph_neighbors",
        "tumult_chaosgraph_coverage_gaps",
        "tumult_fault_catalog",
        "tumult_scaffold_experiment",
        "tumult_chaosgraph_cypher",
        "tumult_topology_import",
        "tumult_topology_map",
        "tumult_compliance_lineage",
        "tumult_recommend_injection",
        "tumult_autopilot_run",
        "tumult_autopilot_status",
        "tumult_autopilot_respond",
        "tumult_autopilot_export",
        "tumult_autopilot_notify",
        "tumult_whoami",
    ];
    let names: Vec<&str> = result.tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names.len(),
        expected.len(),
        "listed tool count must match dispatch arms: {names:?}"
    );
    for name in expected {
        assert!(names.contains(&name), "tool '{name}' missing from list");
    }
    let unique: std::collections::HashSet<&str> = names.iter().copied().collect();
    assert_eq!(unique.len(), names.len(), "tool names must be unique");
    assert!(result.next_cursor.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_tools_every_tool_has_description_and_object_schema() {
    let tmp = tempfile::tempdir().unwrap();
    let handler = open_handler(tmp.path());
    let result = handler
        .handle_list_tools_request(None, stub_runtime())
        .await
        .expect("list_tools must succeed");

    for tool in &result.tools {
        assert!(
            tool.description.as_deref().is_some_and(|d| !d.is_empty()),
            "tool '{}' must have a non-empty description",
            tool.name
        );
        let schema = serde_json::to_value(&tool.input_schema)
            .unwrap_or_else(|e| panic!("schema for '{}' must serialize: {e}", tool.name));
        assert_eq!(
            schema.get("type").and_then(serde_json::Value::as_str),
            Some("object"),
            "input schema for '{}' must be a JSON object schema: {schema}",
            tool.name
        );
    }
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

#[test]
fn tool_annotations_reflect_tool_behavior() {
    // Read-only, idempotent, closed-world tools. The agentic tools are
    // deterministic in-memory simulations (fake adapters only), so they
    // are classified read-only despite their "run" names.
    let read_only = [
        ValidateTool::tool(),
        AnalyzeTool::tool(),
        ReadJournalTool::tool(),
        ListJournalsTool::tool(),
        DiscoverTool::tool(),
        QueryTracesTool::tool(),
        StoreStatsTool::tool(),
        AnalyzeStoreTool::tool(),
        ListExperimentsTool::tool(),
        ComplianceTool::tool(),
        TrendTool::tool(),
        AgentsTool::tool(),
        GameDayAnalyzeTool::tool(),
        GameDayListTool::tool(),
        CoverageTool::tool(),
        AgenticListScenariosTool::tool(),
        AgenticSmokeTool::tool(),
        AgenticRunExperimentTool::tool(),
        ChaosGraphQueryTool::tool(),
        ChaosGraphNeighborsTool::tool(),
        ChaosGraphCoverageGapsTool::tool(),
        ChaosGraphCypherTool::tool(),
        FaultCatalogTool::tool(),
        ScaffoldExperimentTool::tool(),
        TopologyMapTool::tool(),
        ComplianceLineageTool::tool(),
        RecommendInjectionTool::tool(),
        AutopilotStatusTool::tool(),
        WhoamiTool::tool(),
    ];
    for tool in &read_only {
        let a = tool
            .annotations
            .as_ref()
            .unwrap_or_else(|| panic!("'{}' must carry annotations", tool.name));
        assert_eq!(a.read_only_hint, Some(true), "read_only: {}", tool.name);
        assert_eq!(a.idempotent_hint, Some(true), "idempotent: {}", tool.name);
        assert_eq!(a.open_world_hint, Some(false), "open_world: {}", tool.name);
    }

    // Chaos executors: destructive and open-world.
    for tool in &[RunExperimentTool::tool(), GameDayRunTool::tool()] {
        let a = tool
            .annotations
            .as_ref()
            .unwrap_or_else(|| panic!("'{}' must carry annotations", tool.name));
        assert_eq!(a.destructive_hint, Some(true), "destructive: {}", tool.name);
        assert_eq!(a.read_only_hint, Some(false), "read_only: {}", tool.name);
        assert_eq!(a.idempotent_hint, Some(false), "idempotent: {}", tool.name);
        assert_eq!(a.open_world_hint, Some(true), "open_world: {}", tool.name);
    }

    // create_experiment and gameday_create write a new local file:
    // additive, not idempotent (second call errors), closed-world.
    for tool in &[CreateExperimentTool::tool(), GameDayCreateTool::tool()] {
        let a = tool
            .annotations
            .as_ref()
            .unwrap_or_else(|| panic!("'{}' must carry annotations", tool.name));
        assert_eq!(
            a.destructive_hint,
            Some(false),
            "destructive: {}",
            tool.name
        );
        assert_eq!(a.read_only_hint, Some(false), "read_only: {}", tool.name);
        assert_eq!(a.idempotent_hint, Some(false), "idempotent: {}", tool.name);
        assert_eq!(a.open_world_hint, Some(false), "open_world: {}", tool.name);
    }

    // topology_import writes the declared-topology sub-graph into the
    // store: not read-only, but idempotent (re-import converges) and
    // closed-world.
    let import = TopologyImportTool::tool();
    let a = import.annotations.as_ref().expect("import annotations");
    assert_eq!(a.destructive_hint, Some(false));
    assert_eq!(a.read_only_hint, Some(false));
    assert_eq!(a.idempotent_hint, Some(true));
    assert_eq!(a.open_world_hint, Some(false));

    // report may write a file (output_path) but re-rendering the same
    // journal is idempotent and closed-world. Annotations are static, so
    // the write-capable classification wins over the inline-only path.
    let report = ReportTool::tool();
    let a = report.annotations.as_ref().expect("report annotations");
    assert_eq!(a.destructive_hint, Some(false));
    assert_eq!(a.read_only_hint, Some(false));
    assert_eq!(a.idempotent_hint, Some(true));
    assert_eq!(a.open_world_hint, Some(false));

    // recommend can spawn a local agent CLI (which may reach its model
    // API over the network) and can write validated experiment files.
    let recommend = RecommendTool::tool();
    let a = recommend
        .annotations
        .as_ref()
        .expect("recommend annotations");
    assert_eq!(a.destructive_hint, Some(false));
    assert_eq!(a.read_only_hint, Some(false));
    assert_eq!(a.idempotent_hint, Some(false));
    assert_eq!(a.open_world_hint, Some(true));

    // autopilot_run persists new decision records on every pass (not
    // idempotent) and, with execute=true, runs playbook experiments
    // against real targets (open-world) — real fault injection, so the
    // annotation is honest about the destructive potential even though
    // the default (execute=false) only decides and records.
    let run = AutopilotRunTool::tool();
    let a = run.annotations.as_ref().expect("autopilot run annotations");
    assert_eq!(a.destructive_hint, Some(true));
    assert_eq!(a.read_only_hint, Some(false));
    assert_eq!(a.idempotent_hint, Some(false));
    assert_eq!(a.open_world_hint, Some(true));

    // autopilot_respond appends exactly one human event per decision (a
    // second response errors — not idempotent); approval runs the
    // playbook experiment (open-world) after the gate re-evaluation, so
    // it is destructive exactly like autopilot_run with execute=true.
    let respond = AutopilotRespondTool::tool();
    let a = respond
        .annotations
        .as_ref()
        .expect("autopilot respond annotations");
    assert_eq!(a.destructive_hint, Some(true));
    assert_eq!(a.read_only_hint, Some(false));
    assert_eq!(a.idempotent_hint, Some(false));
    assert_eq!(a.open_world_hint, Some(true));

    // autopilot_export re-writes the same Parquet files into the target
    // directory: a converging local file write.
    let export = AutopilotExportTool::tool();
    let a = export
        .annotations
        .as_ref()
        .expect("autopilot export annotations");
    assert_eq!(a.destructive_hint, Some(false));
    assert_eq!(a.read_only_hint, Some(false));
    assert_eq!(a.idempotent_hint, Some(true));
    assert_eq!(a.open_world_hint, Some(false));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_tools_advertises_output_schema_for_exactly_the_structured_tools() {
    let tmp = tempfile::tempdir().unwrap();
    let handler = open_handler(tmp.path());
    let result = handler
        .handle_list_tools_request(None, stub_runtime())
        .await
        .expect("list_tools must succeed");

    for tool in &result.tools {
        let expected = super::super::output_schema::STRUCTURED_TOOLS.contains(&tool.name.as_str());
        assert_eq!(
            tool.output_schema.is_some(),
            expected,
            "output schema advertisement mismatch for '{}'",
            tool.name
        );
        assert!(
            tool.annotations.is_some(),
            "tool '{}' must carry annotations",
            tool.name
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_run_experiment_persists_journal_and_ingests() {
    let tmp = tempfile::tempdir().unwrap();
    crate::tools::test_support::write_valid_experiment(tmp.path());
    let handler = open_handler(tmp.path());
    let store_path = tmp.path().join("analytics.duckdb");

    let params = call_params(
        "tumult_run_experiment",
        serde_json::json!({
            "experiment_path": "test.toon",
            "store_path": store_path.to_str().unwrap(),
        }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect("run must produce a result");
    assert!(result.is_error.is_none(), "{}", result_text(&result));

    // Journal persisted under the CLI-default name in the workspace root.
    let journal_file = tmp.path().join("journal.toon");
    assert!(journal_file.exists(), "journal.toon must be written");
    let journal = tumult_core::journal::read_journal(&journal_file).unwrap();
    assert_eq!(journal.experiment_title, "MCP test experiment");

    // Auto-ingested into the analytics store.
    let store = tumult_analytics::AnalyticsStore::open(&store_path).unwrap();
    assert_eq!(store.stats().unwrap().experiment_count, 1);

    // structuredContent present, conforming, and mirrored in the text.
    let structured = result
        .structured_content
        .as_ref()
        .expect("run must set structuredContent");
    assert_conforms("tumult_run_experiment", structured);
    assert_eq!(structured["ingestion"], "ingested");
    assert_eq!(
        structured["journal"]["experiment_title"],
        "MCP test experiment"
    );
    let text_json: serde_json::Value =
        serde_json::from_str(&result_text(&result)).expect("json text content");
    assert_eq!(text_json["journal_path"], structured["journal_path"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_run_experiment_honors_journal_path_and_no_ingest() {
    let tmp = tempfile::tempdir().unwrap();
    crate::tools::test_support::write_valid_experiment(tmp.path());
    let handler = open_handler(tmp.path());
    let store_path = tmp.path().join("analytics.duckdb");

    let params = call_params(
        "tumult_run_experiment",
        serde_json::json!({
            "experiment_path": "test.toon",
            "journal_path": "custom-run.toon",
            "no_ingest": true,
            "store_path": store_path.to_str().unwrap(),
            "format": "toon",
        }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect("run must produce a result");
    assert!(result.is_error.is_none(), "{}", result_text(&result));

    assert!(tmp.path().join("custom-run.toon").exists());
    assert!(!tmp.path().join("journal.toon").exists());
    assert!(!store_path.exists(), "no_ingest must not create the store");
    let structured = result.structured_content.as_ref().unwrap();
    assert_eq!(structured["ingestion"], "skipped");
    // TOON text format keeps the 2.0.0-era journal payload shape.
    assert!(result_text(&result).contains("MCP test experiment"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_run_experiment_rejects_unknown_rollback_strategy() {
    let tmp = tempfile::tempdir().unwrap();
    crate::tools::test_support::write_valid_experiment(tmp.path());
    let handler = open_handler(tmp.path());

    let params = call_params(
        "tumult_run_experiment",
        serde_json::json!({
            "experiment_path": "test.toon",
            "rollback_strategy": "sometimes",
            "no_ingest": true,
        }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect("tool-level failure must still yield a CallToolResult");
    assert_eq!(result.is_error, Some(true));
    let text = result_text(&result);
    assert!(
        text.contains("sometimes"),
        "must name the bad value: {text}"
    );
    assert!(
        text.contains("on-deviation") && text.contains("always") && text.contains("never"),
        "must list valid values: {text}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_read_journal_structured_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    crate::tools::test_support::write_run_journal(tmp.path());
    let handler = open_handler(tmp.path());

    // Default: full journal as JSON.
    let params = call_params(
        "tumult_read_journal",
        serde_json::json!({ "journal_path": "journal.toon" }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect("read must produce a result");
    assert!(result.is_error.is_none(), "{}", result_text(&result));
    let structured = result
        .structured_content
        .as_ref()
        .expect("read must set structuredContent");
    assert_conforms("tumult_read_journal", structured);
    assert_eq!(
        structured["journal"]["experiment_title"],
        "MCP test experiment"
    );
    // Text is the same object rendered as JSON.
    let text_json: serde_json::Value =
        serde_json::from_str(&result_text(&result)).expect("json text content");
    assert_eq!(
        text_json,
        serde_json::Value::Object(structured.clone()),
        "text content must mirror structuredContent"
    );

    // summary=true drops the full journal but keeps the summary.
    let params = call_params(
        "tumult_read_journal",
        serde_json::json!({ "journal_path": "journal.toon", "summary": true }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .unwrap();
    let structured = result.structured_content.as_ref().unwrap();
    assert_conforms("tumult_read_journal", structured);
    assert!(structured.contains_key("summary"));
    assert!(!structured.contains_key("journal"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)]
async fn structured_content_conforms_to_advertised_schema_for_all_structured_tools() {
    let tmp = tempfile::tempdir().unwrap();
    crate::tools::test_support::write_valid_experiment(tmp.path());
    let handler = open_handler(tmp.path());
    let store_path = tmp.path().join("analytics.duckdb");
    let missing_store = tmp.path().join("missing.duckdb");
    drop(tumult_analytics::AnalyticsStore::open(&store_path).unwrap());

    // Autopilot fixtures: an enabled policy with no playbooks (a pass over
    // this store yields no candidates — an empty decisions array conforms),
    // an export directory, and a seeded `propose` decision so the respond
    // tool's deny path succeeds deterministically.
    let policy_path = tmp.path().join("autopilot.toml");
    std::fs::write(&policy_path, "[autopilot]\nenabled = true\n").unwrap();
    let export_dir = tmp.path().join("autopilot-export");
    {
        let store = tumult_analytics::AnalyticsStore::open(&store_path).unwrap();
        store
            .insert_autopilot_decision(&tumult_analytics::DecisionRecord {
                id: "conf-propose-1".into(),
                decided_at_ns: 1_000,
                trigger: "staleness".into(),
                service_id: "svc:db".into(),
                tier: Some("data".into()),
                plugin: "tumult-db".into(),
                action: "kill-primary".into(),
                article_id: "compliance:DORA/Art. 25".into(),
                score: 1.0,
                reasons: serde_json::json!(["seeded for respond conformance"]),
                confidence: "high".into(),
                playbook: None,
                validator: serde_json::json!({}),
                verdict: "propose".into(),
                gate_rules: serde_json::json!([]),
                gate_detail: serde_json::json!({}),
                policy_hash: "conf-hash".into(),
                autonomy_score: None,
            })
            .unwrap();
    }

    // Ordered so the run writes journal.toon before read_journal uses it.
    let calls: Vec<(&str, serde_json::Value)> = vec![
        (
            "tumult_run_experiment",
            serde_json::json!({
                "experiment_path": "test.toon",
                "store_path": store_path.to_str().unwrap(),
            }),
        ),
        (
            "tumult_read_journal",
            serde_json::json!({ "journal_path": "journal.toon" }),
        ),
        (
            "tumult_report",
            serde_json::json!({ "journal_path": "journal.toon", "format": "junit" }),
        ),
        (
            "tumult_compliance",
            serde_json::json!({ "journals_path": "journal.toon", "framework": "dora" }),
        ),
        (
            "tumult_trend",
            serde_json::json!({ "journals_path": "journal.toon", "metric": "duration_ms" }),
        ),
        (
            "tumult_gameday_create",
            serde_json::json!({ "name": "conformance-gd", "experiments": ["test.toon"] }),
        ),
        ("tumult_agents", serde_json::json!({})),
        ("tumult_whoami", serde_json::json!({})),
        (
            "tumult_recommend",
            serde_json::json!({ "store_path": missing_store.to_str().unwrap() }),
        ),
        (
            "tumult_store_stats",
            serde_json::json!({ "store_path": store_path.to_str().unwrap() }),
        ),
        (
            "tumult_coverage",
            serde_json::json!({ "store_path": missing_store.to_str().unwrap() }),
        ),
        ("tumult_agentic_list_scenarios", serde_json::json!({})),
        ("tumult_agentic_smoke", serde_json::json!({})),
        ("tumult_agentic_run_experiment", serde_json::json!({})),
        ("tumult_list_journals", serde_json::json!({ "path": "." })),
        ("tumult_list_experiments", serde_json::json!({})),
        ("tumult_gameday_list", serde_json::json!({})),
        (
            // The run_experiment call above populated this store's graph.
            "tumult_chaosgraph_query",
            serde_json::json!({
                "kind": "experiment",
                "store_path": store_path.to_str().unwrap(),
            }),
        ),
        (
            "tumult_chaosgraph_neighbors",
            serde_json::json!({
                "node_id": "exp:MCP test experiment",
                "store_path": store_path.to_str().unwrap(),
            }),
        ),
        (
            "tumult_chaosgraph_coverage_gaps",
            serde_json::json!({
                "store_path": store_path.to_str().unwrap(),
            }),
        ),
        ("tumult_fault_catalog", serde_json::json!({})),
        (
            "tumult_scaffold_experiment",
            serde_json::json!({
                "plugin": "tumult-network",
                "action": "add-latency",
                "args": { "delay_ms": 100 },
                "target": "demo-target",
            }),
        ),
        (
            // Import first so the topology tools below see declared services.
            "tumult_topology_import",
            serde_json::json!({
                "toml_content": "[[service]]\nname = \"api\"\ndepends_on = [\"db\"]\n\n[[service]]\nname = \"db\"\n",
                "store_path": store_path.to_str().unwrap(),
            }),
        ),
        (
            "tumult_topology_map",
            serde_json::json!({ "store_path": store_path.to_str().unwrap() }),
        ),
        (
            "tumult_compliance_lineage",
            serde_json::json!({
                "framework": "dora",
                "store_path": store_path.to_str().unwrap(),
            }),
        ),
        (
            "tumult_recommend_injection",
            serde_json::json!({ "store_path": store_path.to_str().unwrap() }),
        ),
        (
            // Enabled policy without playbooks: decide-and-record only,
            // empty decisions is a conforming outcome.
            "tumult_autopilot_run",
            serde_json::json!({
                "policy_path": policy_path.to_str().unwrap(),
                "store_path": store_path.to_str().unwrap(),
            }),
        ),
        (
            "tumult_autopilot_status",
            serde_json::json!({ "store_path": store_path.to_str().unwrap() }),
        ),
        (
            // Deny the seeded propose decision — the approve path would need
            // a runnable playbook; the deny path is the deterministic one.
            "tumult_autopilot_respond",
            serde_json::json!({
                "decision_id": "conf-propose-1",
                "approve": false,
                "reason": "conformance deny",
                "store_path": store_path.to_str().unwrap(),
            }),
        ),
        (
            "tumult_autopilot_export",
            serde_json::json!({
                "dir": export_dir.to_str().unwrap(),
                "store_path": store_path.to_str().unwrap(),
            }),
        ),
    ];

    // This test must exercise every tool that advertises an output schema.
    let mut covered: Vec<&str> = calls.iter().map(|(name, _)| *name).collect();
    covered.sort_unstable();
    let mut expected = super::super::output_schema::STRUCTURED_TOOLS.to_vec();
    expected.sort_unstable();
    assert_eq!(covered, expected, "every structured tool must be covered");

    for (name, args) in calls {
        let params = call_params(name, args, None);
        let result = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .unwrap_or_else(|e| panic!("'{name}' must succeed: {e}"));
        assert!(
            result.is_error.is_none(),
            "'{name}' failed: {}",
            result_text(&result)
        );
        let structured = result
            .structured_content
            .as_ref()
            .unwrap_or_else(|| panic!("'{name}' must set structuredContent"));
        assert_conforms(name, structured);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_report_writes_output_file_and_returns_path() {
    let tmp = tempfile::tempdir().unwrap();
    crate::tools::test_support::write_run_journal(tmp.path());
    let handler = open_handler(tmp.path());

    let params = call_params(
        "tumult_report",
        serde_json::json!({
            "journal_path": "journal.toon",
            "format": "junit",
            "output_path": "report.xml",
        }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect("report must produce a result");
    assert!(result.is_error.is_none(), "{}", result_text(&result));

    let out = tmp.path().join("report.xml");
    assert!(out.exists(), "report file must be written");
    let xml = std::fs::read_to_string(&out).unwrap();
    assert!(xml.contains("<testsuite name=\"MCP test experiment\""));

    let structured = result.structured_content.as_ref().unwrap();
    assert_conforms("tumult_report", structured);
    assert!(!structured.contains_key("content"));
    assert!(result_text(&result).contains("Report generated"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_report_inline_json_and_rejects_html() {
    let tmp = tempfile::tempdir().unwrap();
    crate::tools::test_support::write_run_journal(tmp.path());
    let handler = open_handler(tmp.path());

    let params = call_params(
        "tumult_report",
        serde_json::json!({ "journal_path": "journal.toon" }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .unwrap();
    assert!(result.is_error.is_none(), "{}", result_text(&result));
    let parsed: serde_json::Value =
        serde_json::from_str(&result_text(&result)).expect("inline json report");
    assert_eq!(parsed["experiment_title"], "MCP test experiment");

    // html/pdf are CLI-only; the tool must reject them explicitly.
    let params = call_params(
        "tumult_report",
        serde_json::json!({ "journal_path": "journal.toon", "format": "html" }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    let text = result_text(&result);
    assert!(
        text.contains("json") && text.contains("junit"),
        "must list valid values: {text}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_compliance_rejects_unknown_framework() {
    let tmp = tempfile::tempdir().unwrap();
    crate::tools::test_support::write_run_journal(tmp.path());
    let handler = open_handler(tmp.path());

    let params = call_params(
        "tumult_compliance",
        serde_json::json!({ "journals_path": "journal.toon", "framework": "hipaa" }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    let text = result_text(&result);
    assert!(text.contains("hipaa"), "must name the bad value: {text}");
    assert!(
        text.contains("dora") && text.contains("pci-dss") && text.contains("basel-iii"),
        "must list valid values: {text}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_trend_rejects_unknown_metric() {
    let tmp = tempfile::tempdir().unwrap();
    crate::tools::test_support::write_run_journal(tmp.path());
    let handler = open_handler(tmp.path());

    let params = call_params(
        "tumult_trend",
        serde_json::json!({ "journals_path": "journal.toon", "metric": "latency" }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    let text = result_text(&result);
    assert!(
        text.contains("resilience_score") && text.contains("method_step_count"),
        "must list valid metrics: {text}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_gameday_create_round_trip_writes_parseable_campaign() {
    let tmp = tempfile::tempdir().unwrap();
    crate::tools::test_support::write_valid_experiment(tmp.path());
    let handler = open_handler(tmp.path());

    let params = call_params(
        "tumult_gameday_create",
        serde_json::json!({
            "name": "q3-drill",
            "experiments": ["test.toon"],
            "load_tool": "k6",
            "load_script": "load.js",
            "load_vus": 5,
            "framework": "dora",
        }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect("gameday_create must produce a result");
    assert!(result.is_error.is_none(), "{}", result_text(&result));

    let created = tmp.path().join("q3-drill.gameday.toon");
    assert!(created.exists(), "gameday file must be written");
    let content = std::fs::read_to_string(&created).unwrap();
    let gameday: tumult_core::types::GameDay =
        toon_format::decode_default(&content).expect("created file must parse as GameDay");
    assert_eq!(gameday.title, "q3-drill");
    assert_eq!(gameday.experiments.len(), 1);
    assert!(gameday.load.is_some());
    assert!(content.contains("frameworks[1]: DORA"));

    let structured = result.structured_content.as_ref().unwrap();
    assert_conforms("tumult_gameday_create", structured);
    assert_eq!(structured["experiments"], 1);

    // Second create with the same name must fail (no overwrite).
    let params = call_params(
        "tumult_gameday_create",
        serde_json::json!({ "name": "q3-drill", "experiments": ["test.toon"] }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    assert!(result_text(&result).contains("already exists"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_gameday_create_rejects_name_traversal() {
    let tmp = tempfile::tempdir().unwrap();
    let handler = open_handler(tmp.path());

    let params = call_params(
        "tumult_gameday_create",
        serde_json::json!({ "name": "../escape", "experiments": ["test.toon"] }),
        None,
    );
    assert!(
        handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .is_err(),
        "gameday name traversal must be rejected at the dispatch boundary"
    );
    assert!(
        !tmp.path()
            .parent()
            .unwrap()
            .join("escape.gameday.toon")
            .exists(),
        "no file must be written outside the workspace"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_agents_round_trip_lists_builtin_adapters() {
    let tmp = tempfile::tempdir().unwrap();
    let handler = open_handler(tmp.path());

    let params = call_params("tumult_agents", serde_json::json!({}), None);
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect("agents must produce a result");
    assert!(result.is_error.is_none(), "{}", result_text(&result));

    let structured = result.structured_content.as_ref().unwrap();
    assert_conforms("tumult_agents", structured);
    let names: Vec<&str> = structured["adapters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"claude-code"), "adapters: {names:?}");
    assert!(names.contains(&"codex"), "adapters: {names:?}");
    assert!(result_text(&result).contains("ADAPTER"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_recommend_rejects_agent_params_without_agent() {
    let tmp = tempfile::tempdir().unwrap();
    let handler = open_handler(tmp.path());

    let params = call_params(
        "tumult_recommend",
        serde_json::json!({ "agent_model": "opus" }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    assert!(
        result_text(&result).contains("require the agent parameter"),
        "got: {}",
        result_text(&result)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_recommend_rejects_unknown_adapter_listing_available() {
    let tmp = tempfile::tempdir().unwrap();
    let handler = open_handler(tmp.path());

    let params = call_params(
        "tumult_recommend",
        serde_json::json!({ "agent": "no-such-agent" }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    let text = result_text(&result);
    assert!(
        text.contains("no-such-agent"),
        "must name the bad adapter: {text}"
    );
    assert!(
        text.contains("claude-code") && text.contains("codex"),
        "must list available adapters: {text}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_recommend_heuristic_over_real_store_matches_intelligence_shape() {
    let tmp = tempfile::tempdir().unwrap();
    crate::tools::test_support::write_valid_experiment(tmp.path());
    let handler = open_handler(tmp.path());
    let store_path = tmp.path().join("analytics.duckdb");

    // Populate the store through a real run.
    let params = call_params(
        "tumult_run_experiment",
        serde_json::json!({
            "experiment_path": "test.toon",
            "store_path": store_path.to_str().unwrap(),
        }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .unwrap();
    assert!(result.is_error.is_none(), "{}", result_text(&result));

    let params = call_params(
        "tumult_recommend",
        serde_json::json!({ "store_path": store_path.to_str().unwrap() }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .unwrap();
    assert!(result.is_error.is_none(), "{}", result_text(&result));
    let structured = result.structured_content.as_ref().unwrap();
    assert_conforms("tumult_recommend", structured);
    // The tumult-intelligence pipeline is the source of truth now.
    assert_eq!(structured["source"], "heuristic-fallback");
    assert!(structured["heuristic_context"]
        .as_str()
        .unwrap()
        .contains("Coverage:"));
    assert!(result_text(&result).contains("AI-Powered Tumult Recommendations"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_list_journals_accepts_path_argument() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("a.toon"), "x").unwrap();
    let handler = open_handler(tmp.path());

    let params = call_params(
        "tumult_list_journals",
        serde_json::json!({ "path": "." }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect("list_journals must accept the unified `path` parameter");
    assert!(result_text(&result).contains("a.toon"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_validate_round_trip_returns_summary() {
    let tmp = tempfile::tempdir().unwrap();
    crate::tools::test_support::write_valid_experiment(tmp.path());
    let handler = open_handler(tmp.path());

    let params = call_params(
        "tumult_validate",
        serde_json::json!({ "experiment_path": "test.toon" }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect("valid experiment must produce a result");

    let text = result_text(&result);
    assert!(
        text.contains("Valid: 'MCP test experiment'"),
        "unexpected payload: {text}"
    );
    assert!(
        text.contains("1 method steps"),
        "unexpected payload: {text}"
    );
    assert!(
        !text.starts_with("Error:"),
        "validation must not report an error: {text}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_list_journals_round_trip_filters_toon_files() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("a.toon"), "x").unwrap();
    std::fs::write(tmp.path().join("b.toon"), "x").unwrap();
    std::fs::write(tmp.path().join("notes.txt"), "x").unwrap();
    let handler = open_handler(tmp.path());

    let params = call_params(
        "tumult_list_journals",
        serde_json::json!({ "directory": "." }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect("list_journals must produce a result");

    let text = result_text(&result);
    assert!(text.contains("a.toon"), "a.toon missing from: {text}");
    assert!(text.contains("b.toon"), "b.toon missing from: {text}");
    assert!(
        !text.contains("notes.txt"),
        "non-.toon file must be filtered out: {text}"
    );
}

// ── resource_link content items ───────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_run_experiment_attaches_journal_resource_link() {
    let tmp = tempfile::tempdir().unwrap();
    crate::tools::test_support::write_valid_experiment(tmp.path());
    let handler = open_handler(tmp.path());

    let params = call_params(
        "tumult_run_experiment",
        serde_json::json!({ "experiment_path": "test.toon", "no_ingest": true }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .unwrap();
    assert!(result.is_error.is_none(), "{}", result_text(&result));

    let links = result_links(&result);
    assert_eq!(links.len(), 1, "run must link the written journal");
    assert_eq!(links[0].uri, "tumult://journal/journal.toon");
    assert_eq!(links[0].mime_type.as_deref(), Some("application/json"));
    // Text content is still the first block (backward compat).
    assert!(matches!(result.content[0], ContentBlock::TextContent(_)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_gameday_create_attaches_gameday_resource_link() {
    let tmp = tempfile::tempdir().unwrap();
    crate::tools::test_support::write_valid_experiment(tmp.path());
    let handler = open_handler(tmp.path());

    let params = call_params(
        "tumult_gameday_create",
        serde_json::json!({ "name": "linked", "experiments": ["test.toon"] }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .unwrap();
    assert!(result.is_error.is_none(), "{}", result_text(&result));

    let links = result_links(&result);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].uri, "tumult://gameday/linked.gameday.toon");
    assert_eq!(links[0].mime_type.as_deref(), Some("application/toon"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_report_attaches_link_only_when_output_path_given() {
    let tmp = tempfile::tempdir().unwrap();
    crate::tools::test_support::write_run_journal(tmp.path());
    let handler = open_handler(tmp.path());

    let params = call_params(
        "tumult_report",
        serde_json::json!({
            "journal_path": "journal.toon",
            "format": "junit",
            "output_path": "report.xml",
        }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .unwrap();
    let links = result_links(&result);
    assert_eq!(links.len(), 1, "written report must be linked");
    assert!(links[0].uri.starts_with("file://"), "uri: {}", links[0].uri);
    assert!(
        links[0].uri.ends_with("/report.xml"),
        "uri: {}",
        links[0].uri
    );
    assert_eq!(links[0].mime_type.as_deref(), Some("application/xml"));

    // Inline report (no output_path): no link.
    let params = call_params(
        "tumult_report",
        serde_json::json!({ "journal_path": "journal.toon" }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .unwrap();
    assert!(result_links(&result).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_list_journals_attaches_links_capped_at_fifty() {
    let tmp = tempfile::tempdir().unwrap();
    for i in 0..55 {
        std::fs::write(tmp.path().join(format!("j-{i:02}.toon")), "x").unwrap();
    }
    let handler = open_handler(tmp.path());

    let params = call_params(
        "tumult_list_journals",
        serde_json::json!({ "path": "." }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .unwrap();
    assert!(result.is_error.is_none(), "{}", result_text(&result));

    let links = result_links(&result);
    assert_eq!(links.len(), 50, "links must be capped at the first 50");
    assert_eq!(links[0].uri, "tumult://journal/j-00.toon");
    // All 55 paths still appear in the text and structured items.
    let structured = result.structured_content.as_ref().unwrap();
    assert_eq!(structured["total"], 55);
    assert_eq!(structured["items"].as_array().unwrap().len(), 55);
}

// ── list tool pagination ──────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_list_journals_honors_limit_offset_and_totals() {
    let tmp = tempfile::tempdir().unwrap();
    for name in ["a.toon", "b.toon", "c.toon"] {
        std::fs::write(tmp.path().join(name), "x").unwrap();
    }
    let handler = open_handler(tmp.path());

    let params = call_params(
        "tumult_list_journals",
        serde_json::json!({ "path": ".", "limit": 1, "offset": 1 }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .unwrap();
    assert!(result.is_error.is_none(), "{}", result_text(&result));

    let structured = result.structured_content.as_ref().unwrap();
    assert_conforms("tumult_list_journals", structured);
    assert_eq!(structured["total"], 3);
    assert_eq!(structured["offset"], 1);
    assert_eq!(structured["limit"], 1);
    let items = structured["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert!(items[0].as_str().unwrap().ends_with("b.toon"));
    let text = result_text(&result);
    assert!(
        text.contains("b.toon") && !text.contains("a.toon"),
        "{text}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_list_tools_reject_limit_over_max() {
    let tmp = tempfile::tempdir().unwrap();
    let handler = open_handler(tmp.path());

    for (name, mut args) in [
        ("tumult_list_journals", serde_json::json!({ "path": "." })),
        ("tumult_list_experiments", serde_json::json!({})),
        ("tumult_gameday_list", serde_json::json!({})),
    ] {
        args["limit"] = serde_json::json!(1001);
        let err = handler
            .handle_call_tool_request(call_params(name, args, None), stub_runtime())
            .await
            .expect_err("limit over 1000 must be rejected");
        assert!(err.to_string().contains("1000"), "{name}: {err}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_list_experiments_and_gamedays_paginate_with_totals() {
    let tmp = tempfile::tempdir().unwrap();
    for i in 0..4 {
        std::fs::write(
            tmp.path().join(format!("exp-{i}.toon")),
            format!("title: Experiment {i}\n"),
        )
        .unwrap();
        std::fs::write(
            tmp.path().join(format!("gd-{i}.gameday.toon")),
            format!("title: GD {i}\n"),
        )
        .unwrap();
    }
    let handler = open_handler(tmp.path());

    let params = call_params(
        "tumult_list_experiments",
        serde_json::json!({ "limit": 2, "offset": 2 }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .unwrap();
    let structured = result.structured_content.as_ref().unwrap();
    assert_conforms("tumult_list_experiments", structured);
    // The 4 .gameday.toon files also carry titles, so they are counted
    // as experiments by the recursive .toon discovery (existing tool
    // behavior); totals must reflect everything found.
    assert_eq!(structured["total"], 8);
    assert_eq!(structured["items"].as_array().unwrap().len(), 2);

    let params = call_params(
        "tumult_gameday_list",
        serde_json::json!({ "limit": 3, "offset": 2 }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .unwrap();
    let structured = result.structured_content.as_ref().unwrap();
    assert_conforms("tumult_gameday_list", structured);
    assert_eq!(structured["total"], 4);
    assert_eq!(structured["offset"], 2);
    let items = structured["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "only two entries remain after offset 2");
    assert_eq!(items[0]["title"], "GD 2");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_tool_failure_is_reported_as_error_text() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("bad.toon"), "not a valid experiment {{{").unwrap();
    let handler = open_handler(tmp.path());

    let params = call_params(
        "tumult_validate",
        serde_json::json!({ "experiment_path": "bad.toon" }),
        None,
    );
    // Tool-level failures are embedded in the result payload (per MCP spec),
    // not surfaced as protocol errors.
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect("tool failure must still yield a CallToolResult");

    let text = result_text(&result);
    assert!(
        text.starts_with("Error:"),
        "tool failure must be reported in the payload: {text}"
    );
    assert_eq!(
        result.is_error,
        Some(true),
        "tool failure must set isError so clients can detect it"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_create_experiment_round_trip_writes_file() {
    let tmp = tempfile::tempdir().unwrap();
    let handler = open_handler(tmp.path());

    let params = call_params(
        "tumult_create_experiment",
        serde_json::json!({ "output_path": "new-experiment.toon" }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect("create_experiment must produce a result");

    let text = result_text(&result);
    assert!(
        text.starts_with("Created"),
        "creation must be reported in the payload: {text}"
    );
    assert!(
        result.is_error.is_none(),
        "successful creation must not set isError"
    );
    let created = tmp.path().join("new-experiment.toon");
    assert!(created.exists(), "output file must exist in the workspace");
    let content = std::fs::read_to_string(&created).unwrap();
    assert!(content.contains("title:"), "template must be written");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_create_experiment_rejects_output_path_traversal() {
    let tmp = tempfile::tempdir().unwrap();
    let handler = open_handler(tmp.path());

    let params = call_params(
        "tumult_create_experiment",
        serde_json::json!({ "output_path": "../escape.toon" }),
        None,
    );
    assert!(
        handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .is_err(),
        "output path traversal must be rejected at the dispatch boundary"
    );
    assert!(
        !tmp.path().parent().unwrap().join("escape.toon").exists(),
        "no file must be written outside the workspace"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_unknown_name_returns_error() {
    let tmp = tempfile::tempdir().unwrap();
    let handler = open_handler(tmp.path());

    let params = call_params("tumult_no_such_tool", serde_json::json!({}), None);
    let err = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect_err("unknown tool must be a protocol-level error");
    assert!(
        err.to_string().contains("tumult_no_such_tool"),
        "error must name the unknown tool: {err}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_missing_required_argument_returns_error_not_panic() {
    let tmp = tempfile::tempdir().unwrap();
    let handler = open_handler(tmp.path());

    // `tumult_validate` requires `experiment_path`; omit it entirely.
    let params = call_params("tumult_validate", serde_json::json!({}), None);
    let err = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect_err("missing required argument must be an error");
    assert!(
        err.to_string().contains("experiment_path"),
        "error must mention the missing field: {err}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_wrong_argument_type_returns_error_not_panic() {
    let tmp = tempfile::tempdir().unwrap();
    let handler = open_handler(tmp.path());

    // Wrong type (number instead of string) and absent arguments object
    // must both fail gracefully.
    let params = call_params(
        "tumult_validate",
        serde_json::json!({ "experiment_path": 42 }),
        None,
    );
    assert!(handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .is_err());

    let params = call_params("tumult_validate", serde_json::Value::Null, None);
    assert!(handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_path_traversal_rejected_through_dispatch() {
    let tmp = tempfile::tempdir().unwrap();
    let handler = open_handler(tmp.path());

    let params = call_params(
        "tumult_validate",
        serde_json::json!({ "experiment_path": "../../etc/passwd" }),
        None,
    );
    assert!(
        handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .is_err(),
        "path traversal must be rejected at the dispatch boundary"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_rejects_wrong_bearer_token() {
    let tmp = tempfile::tempdir().unwrap();
    crate::tools::test_support::write_valid_experiment(tmp.path());
    let handler = TumultHandler::with_auth(
        tmp.path().to_path_buf(),
        McpAuth::single_operator("dispatch-secret".into()),
    );

    let params = call_params(
        "tumult_validate",
        serde_json::json!({ "experiment_path": "test.toon" }),
        Some("Bearer wrong-token"),
    );
    let err = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect_err("wrong bearer token must be rejected");
    assert!(
        err.to_string().contains("Unauthorized"),
        "error must indicate authorization failure: {err}"
    );
    assert!(
        !err.to_string().contains("Unknown tool"),
        "auth failure must not masquerade as an unknown tool: {err}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_rejects_missing_token_when_auth_configured() {
    let tmp = tempfile::tempdir().unwrap();
    let handler = TumultHandler::with_auth(
        tmp.path().to_path_buf(),
        McpAuth::single_operator("dispatch-secret".into()),
    );

    // No `_meta.authorization` supplied at all.
    let params = call_params("tumult_discover", serde_json::json!({}), None);
    let err = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect_err("missing token must be rejected when auth is configured");
    assert!(err.to_string().contains("Unauthorized"), "got: {err}");
    assert!(
        !err.to_string().contains("Unknown tool"),
        "auth failure must not masquerade as an unknown tool: {err}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_accepts_correct_bearer_token() {
    let tmp = tempfile::tempdir().unwrap();
    crate::tools::test_support::write_valid_experiment(tmp.path());
    let handler = TumultHandler::with_auth(
        tmp.path().to_path_buf(),
        McpAuth::single_operator("dispatch-secret".into()),
    );

    let params = call_params(
        "tumult_validate",
        serde_json::json!({ "experiment_path": "test.toon" }),
        Some("Bearer dispatch-secret"),
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect("correct bearer token must be accepted");
    assert!(
        result_text(&result).contains("Valid: 'MCP test experiment'"),
        "authorized call must reach the tool"
    );
}

// ── Role-based access control (RBAC) ─────────────────────────

/// The dispatch role table must agree with every tool's declared
/// `read_only_hint`: read-only ⇒ Viewer, otherwise Operator. This guards
/// against a hint change silently drifting from the enforced role.
#[test]
fn required_roles_match_annotations() {
    let tools = vec![
        RunExperimentTool::tool(),
        ValidateTool::tool(),
        AnalyzeTool::tool(),
        ReadJournalTool::tool(),
        ListJournalsTool::tool(),
        DiscoverTool::tool(),
        CreateExperimentTool::tool(),
        QueryTracesTool::tool(),
        StoreStatsTool::tool(),
        AnalyzeStoreTool::tool(),
        ListExperimentsTool::tool(),
        ReportTool::tool(),
        ComplianceTool::tool(),
        TrendTool::tool(),
        AgentsTool::tool(),
        GameDayCreateTool::tool(),
        GameDayRunTool::tool(),
        GameDayAnalyzeTool::tool(),
        GameDayListTool::tool(),
        RecommendTool::tool(),
        CoverageTool::tool(),
        AgenticListScenariosTool::tool(),
        AgenticSmokeTool::tool(),
        AgenticRunExperimentTool::tool(),
        ChaosGraphQueryTool::tool(),
        ChaosGraphNeighborsTool::tool(),
        ChaosGraphCoverageGapsTool::tool(),
        ChaosGraphCypherTool::tool(),
        FaultCatalogTool::tool(),
        ScaffoldExperimentTool::tool(),
        TopologyImportTool::tool(),
        TopologyMapTool::tool(),
        ComplianceLineageTool::tool(),
        RecommendInjectionTool::tool(),
        AutopilotRunTool::tool(),
        AutopilotStatusTool::tool(),
        AutopilotRespondTool::tool(),
        AutopilotExportTool::tool(),
        AutopilotNotifyTool::tool(),
        WhoamiTool::tool(),
    ];
    for tool in &tools {
        let read_only = tool
            .annotations
            .as_ref()
            .and_then(|a| a.read_only_hint)
            .unwrap_or(false);
        let expected = if read_only {
            Role::Viewer
        } else {
            Role::Operator
        };
        assert_eq!(
            required_role_for_tool(&tool.name),
            expected,
            "required role for '{}' must match its read_only_hint",
            tool.name
        );
    }
    // Fail-closed default for an unknown tool.
    assert_eq!(required_role_for_tool("tumult_nonexistent"), Role::Operator);
}

/// A viewer token may call a read-only tool.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn viewer_allowed_on_read_only_tool() {
    let tmp = tempfile::tempdir().unwrap();
    crate::tools::test_support::write_valid_experiment(tmp.path());
    let handler = TumultHandler::with_auth(
        tmp.path().to_path_buf(),
        McpAuth::from_tokens(vec![
            ("view-tok".into(), Role::Viewer),
            ("op-tok".into(), Role::Operator),
        ]),
    );

    let params = call_params(
        "tumult_validate",
        serde_json::json!({ "experiment_path": "test.toon" }),
        Some("Bearer view-tok"),
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect("viewer must reach a read-only tool");
    assert!(result.is_error.is_none(), "{}", result_text(&result));
    assert!(result_text(&result).contains("Valid: 'MCP test experiment'"));
}

/// A viewer token is rejected on operator-only tools with a clear role
/// error (`run_experiment` and `gameday_run` — fault injection/execution).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn viewer_rejected_on_operator_tools() {
    let tmp = tempfile::tempdir().unwrap();
    crate::tools::test_support::write_valid_experiment(tmp.path());
    let handler = TumultHandler::with_auth(
        tmp.path().to_path_buf(),
        McpAuth::from_tokens(vec![("view-tok".into(), Role::Viewer)]),
    );

    for tool in ["tumult_run_experiment", "tumult_gameday_run"] {
        let params = call_params(
            tool,
            serde_json::json!({
                "experiment_path": "test.toon",
                "gameday_path": "x.gameday.toon",
                "no_ingest": true,
            }),
            Some("Bearer view-tok"),
        );
        let err = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .expect_err("viewer must be rejected on an operator-only tool");
        let msg = err.to_string();
        assert!(msg.contains("Unauthorized"), "got: {msg}");
        assert!(msg.contains("requires the 'operator' role"), "got: {msg}");
        assert!(msg.contains("token has 'viewer'"), "got: {msg}");
        // A role rejection must not run the destructive tool.
        assert!(
            !tmp.path().join("journal.toon").exists(),
            "run_experiment must not execute for a viewer"
        );
    }
}

/// A viewer token is rejected on `tumult_topology_import` (the one topology
/// tool that writes to the store) without the write being attempted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn viewer_rejected_on_topology_import() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("analytics.duckdb");
    drop(tumult_analytics::AnalyticsStore::open(&store).unwrap());
    let handler = TumultHandler::with_auth(
        tmp.path().to_path_buf(),
        McpAuth::from_tokens(vec![
            ("view-tok".into(), Role::Viewer),
            ("op-tok".into(), Role::Operator),
        ]),
    );

    let args = serde_json::json!({
        "toml_content": "[[service]]\nname = \"db\"\n",
        "store_path": store.to_str().unwrap(),
    });
    let params = call_params(
        "tumult_topology_import",
        args.clone(),
        Some("Bearer view-tok"),
    );
    let err = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect_err("viewer must be rejected on topology import");
    let msg = err.to_string();
    assert!(msg.contains("Unauthorized"), "got: {msg}");
    assert!(msg.contains("requires the 'operator' role"), "got: {msg}");
    assert!(msg.contains("token has 'viewer'"), "got: {msg}");

    // The same call succeeds for an operator token.
    let params = call_params("tumult_topology_import", args, Some("Bearer op-tok"));
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect("operator must reach topology import");
    assert!(result.is_error.is_none(), "{}", result_text(&result));
    assert!(result_text(&result).contains("imported 1 services"));
}

/// A viewer token is rejected on `tumult_autopilot_run` (every pass writes
/// decision records, and execute=true injects faults) without the pass
/// being attempted; the same call succeeds for an operator token.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn viewer_rejected_on_autopilot_run() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("analytics.duckdb");
    drop(tumult_analytics::AnalyticsStore::open(&store).unwrap());
    let policy = tmp.path().join("autopilot.toml");
    std::fs::write(&policy, "[autopilot]\nenabled = true\n").unwrap();
    let handler = TumultHandler::with_auth(
        tmp.path().to_path_buf(),
        McpAuth::from_tokens(vec![
            ("view-tok".into(), Role::Viewer),
            ("op-tok".into(), Role::Operator),
        ]),
    );

    let args = serde_json::json!({
        "policy_path": policy.to_str().unwrap(),
        "store_path": store.to_str().unwrap(),
    });
    let params = call_params(
        "tumult_autopilot_run",
        args.clone(),
        Some("Bearer view-tok"),
    );
    let err = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect_err("viewer must be rejected on autopilot run");
    let msg = err.to_string();
    assert!(msg.contains("Unauthorized"), "got: {msg}");
    assert!(msg.contains("requires the 'operator' role"), "got: {msg}");
    assert!(msg.contains("token has 'viewer'"), "got: {msg}");

    // The same call succeeds for an operator token (empty pass — no
    // candidates on a fresh store).
    let params = call_params("tumult_autopilot_run", args, Some("Bearer op-tok"));
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect("operator must reach autopilot run");
    assert!(result.is_error.is_none(), "{}", result_text(&result));

    // The Viewer-gated status readback works for the viewer token.
    let params = call_params(
        "tumult_autopilot_status",
        serde_json::json!({ "store_path": store.to_str().unwrap() }),
        Some("Bearer view-tok"),
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect("viewer must reach autopilot status");
    assert!(result.is_error.is_none(), "{}", result_text(&result));
}

/// An operator token may call both read-only and operator-only tools.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn operator_allowed_on_read_only_and_operator_tools() {
    let tmp = tempfile::tempdir().unwrap();
    crate::tools::test_support::write_valid_experiment(tmp.path());
    let store = tmp.path().join("analytics.duckdb");
    let handler = TumultHandler::with_auth(
        tmp.path().to_path_buf(),
        McpAuth::from_tokens(vec![
            ("view-tok".into(), Role::Viewer),
            ("op-tok".into(), Role::Operator),
        ]),
    );

    // Read-only tool.
    let params = call_params(
        "tumult_validate",
        serde_json::json!({ "experiment_path": "test.toon" }),
        Some("Bearer op-tok"),
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect("operator must reach a read-only tool");
    assert!(result.is_error.is_none(), "{}", result_text(&result));

    // Operator-only tool actually executes.
    let params = call_params(
        "tumult_run_experiment",
        serde_json::json!({
            "experiment_path": "test.toon",
            "store_path": store.to_str().unwrap(),
        }),
        Some("Bearer op-tok"),
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect("operator must reach an operator-only tool");
    assert!(result.is_error.is_none(), "{}", result_text(&result));
    assert!(tmp.path().join("journal.toon").exists());
}

/// A token not present in the map is rejected (never elevated).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_token_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let handler = TumultHandler::with_auth(
        tmp.path().to_path_buf(),
        McpAuth::from_tokens(vec![("view-tok".into(), Role::Viewer)]),
    );
    let params = call_params(
        "tumult_discover",
        serde_json::json!({}),
        Some("Bearer not-a-real-token"),
    );
    let err = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect_err("unknown token must be rejected");
    assert!(err.to_string().contains("Unauthorized"), "got: {err}");
    assert!(
        err.to_string().contains("invalid bearer token"),
        "got: {err}"
    );
}

/// Backward-compat: a single operator token (legacy `TUMULT_MCP_TOKEN` shape)
/// can call every tool, read-only and operator-only alike.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_single_token_is_operator() {
    let tmp = tempfile::tempdir().unwrap();
    crate::tools::test_support::write_valid_experiment(tmp.path());
    let store = tmp.path().join("analytics.duckdb");
    let handler = TumultHandler::with_auth(
        tmp.path().to_path_buf(),
        McpAuth::single_operator("legacy-secret".into()),
    );

    let params = call_params(
        "tumult_run_experiment",
        serde_json::json!({
            "experiment_path": "test.toon",
            "store_path": store.to_str().unwrap(),
        }),
        Some("Bearer legacy-secret"),
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect("legacy single token must call operator tools");
    assert!(result.is_error.is_none(), "{}", result_text(&result));
}

/// `tumult_whoami` reports the caller's resolved role. A viewer token sees
/// `viewer`, an operator token sees `operator`, and both are marked
/// authenticated — the role plumbing (`_meta.authorization` → auth layer →
/// tool handler) must carry the resolved role all the way through.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn whoami_reports_the_callers_role() {
    let tmp = tempfile::tempdir().unwrap();
    let handler = TumultHandler::with_auth(
        tmp.path().to_path_buf(),
        McpAuth::from_tokens(vec![
            ("view-tok".into(), Role::Viewer),
            ("op-tok".into(), Role::Operator),
        ]),
    );

    for (token, expected_role) in [("view-tok", "viewer"), ("op-tok", "operator")] {
        let params = call_params(
            "tumult_whoami",
            serde_json::json!({}),
            Some(&format!("Bearer {token}")),
        );
        let result = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .expect("whoami must succeed for a valid token");
        assert!(result.is_error.is_none(), "{}", result_text(&result));
        let structured = result
            .structured_content
            .as_ref()
            .expect("whoami must set structuredContent");
        assert_conforms("tumult_whoami", structured);
        assert_eq!(
            structured["role"], expected_role,
            "whoami must report the token's role"
        );
        assert_eq!(
            structured["authenticated"], true,
            "a validated token is authenticated"
        );
    }
}

/// In open mode (no auth configured) `tumult_whoami` reports an
/// unauthenticated operator: full access for loopback dev, no token.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn whoami_open_mode_is_unauthenticated_operator() {
    let tmp = tempfile::tempdir().unwrap();
    let handler = open_handler(tmp.path());
    let params = call_params("tumult_whoami", serde_json::json!({}), None);
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect("whoami must succeed in open mode");
    let structured = result.structured_content.as_ref().unwrap();
    assert_eq!(structured["role"], "operator");
    assert_eq!(structured["authenticated"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_closed_semaphore_returns_error() {
    let tmp = tempfile::tempdir().unwrap();
    let handler = open_handler(tmp.path());
    handler.semaphore.close();

    let params = call_params("tumult_discover", serde_json::json!({}), None);
    let err = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect_err("closed semaphore must fail the call");
    assert!(err.to_string().contains("semaphore closed"), "got: {err}");
    assert!(
        !err.to_string().contains("Unknown tool"),
        "semaphore failure must not masquerade as an unknown tool: {err}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_releases_semaphore_permit_after_completion() {
    let tmp = tempfile::tempdir().unwrap();
    crate::tools::test_support::write_valid_experiment(tmp.path());
    let handler = open_handler(tmp.path());

    for _ in 0..3 {
        let params = call_params(
            "tumult_validate",
            serde_json::json!({ "experiment_path": "test.toon" }),
            None,
        );
        handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .expect("call must succeed");
    }
    assert_eq!(
        handler.semaphore.available_permits(),
        MAX_CONCURRENT_TOOL_CALLS,
        "permits must be released after each call"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_waits_while_semaphore_saturated_and_resumes_on_release() {
    let tmp = tempfile::tempdir().unwrap();
    crate::tools::test_support::write_valid_experiment(tmp.path());
    let handler = Arc::new(open_handler(tmp.path()));

    // Saturate the semaphore.
    let permits = handler
        .semaphore
        .acquire_many(u32::try_from(MAX_CONCURRENT_TOOL_CALLS).unwrap())
        .await
        .unwrap();

    let call_handler = Arc::clone(&handler);
    let task = tokio::spawn(async move {
        let params = call_params(
            "tumult_validate",
            serde_json::json!({ "experiment_path": "test.toon" }),
            None,
        );
        // CallToolError is not Send; map both arms to Send types before
        // crossing the task boundary.
        call_handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .map(|result| result_text(&result))
            .map_err(|e| e.to_string())
    });

    // Give the spawned call ample opportunity to run: it must stay parked
    // on the semaphore while all permits are held.
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    assert!(
        !task.is_finished(),
        "call must not proceed while the semaphore is saturated"
    );

    // Releasing the permits must let the call complete.
    drop(permits);
    let text = task
        .await
        .unwrap()
        .expect("call must succeed after release");
    assert!(text.contains("Valid: 'MCP test experiment'"));
}

// ── tools/list authentication gate ─────────────────────────────

/// With auth configured, `tools/list` requires a valid token (any role):
/// the listing names the destructive tools, so it is gated exactly like
/// `resources/list`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_tools_requires_auth_when_configured() {
    let tmp = tempfile::tempdir().unwrap();
    let handler = TumultHandler::with_auth(
        tmp.path().to_path_buf(),
        McpAuth::from_tokens(vec![("view-tok".into(), Role::Viewer)]),
    );

    // No token → refused before any tool is named.
    let err = handler
        .handle_list_tools_request(None, stub_runtime())
        .await
        .expect_err("tools/list must require a token when auth is configured");
    assert!(err.to_string().contains("Unauthorized"), "got: {err}");

    // A viewer token suffices (the listing is role-agnostic).
    let mut extra = serde_json::Map::new();
    extra.insert(
        "authorization".into(),
        serde_json::Value::String("Bearer view-tok".into()),
    );
    let params = PaginatedRequestParams {
        cursor: None,
        meta: Some(rust_mcp_sdk::schema::PaginatedMeta {
            progress_token: None,
            extra: Some(extra),
        }),
    };
    let result = handler
        .handle_list_tools_request(Some(params), stub_runtime())
        .await
        .expect("a valid viewer token must list tools");
    assert_eq!(result.tools.len(), 40);
}

/// With no auth configured (loopback-only mode) `tools/list` stays open —
/// pinned separately from the round-trip content test.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_tools_open_when_auth_not_configured() {
    let tmp = tempfile::tempdir().unwrap();
    let handler = open_handler(tmp.path());
    let result = handler
        .handle_list_tools_request(None, stub_runtime())
        .await
        .expect("tools/list must stay open in open mode");
    assert_eq!(result.tools.len(), 40);
}

// ── HTTP Authorization header channel ──────────────────────────

/// A bearer token captured from the HTTP `Authorization` header (carried on
/// the session runtime as `AuthInfo`) authenticates the call exactly like an
/// explicit `_meta.authorization`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_authorization_header_authenticates_call() {
    let tmp = tempfile::tempdir().unwrap();
    let handler = TumultHandler::with_auth(
        tmp.path().to_path_buf(),
        McpAuth::from_tokens(vec![("view-tok".into(), Role::Viewer)]),
    );
    let params = call_params("tumult_whoami", serde_json::json!({}), None);
    let result = handler
        .handle_call_tool_request(params, stub_runtime_with_bearer("view-tok"))
        .await
        .expect("header-authenticated call must succeed");
    assert!(result.is_error.is_none(), "{}", result_text(&result));
    let structured = result.structured_content.as_ref().unwrap();
    assert_eq!(structured["role"], "viewer");
    assert_eq!(structured["authenticated"], true);
}

/// When both channels are present, the explicit `_meta.authorization` wins.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn meta_authorization_takes_precedence_over_header() {
    let tmp = tempfile::tempdir().unwrap();
    let handler = TumultHandler::with_auth(
        tmp.path().to_path_buf(),
        McpAuth::from_tokens(vec![
            ("view-tok".into(), Role::Viewer),
            ("op-tok".into(), Role::Operator),
        ]),
    );
    // Header says operator, explicit _meta says viewer → viewer.
    let params = call_params(
        "tumult_whoami",
        serde_json::json!({}),
        Some("Bearer view-tok"),
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime_with_bearer("op-tok"))
        .await
        .expect("call must succeed");
    let structured = result.structured_content.as_ref().unwrap();
    assert_eq!(
        structured["role"], "viewer",
        "explicit _meta.authorization must win over the header"
    );
}

/// A wrong token fails closed regardless of the channel it arrives on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_authorization_header_wrong_token_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let handler = TumultHandler::with_auth(
        tmp.path().to_path_buf(),
        McpAuth::from_tokens(vec![("view-tok".into(), Role::Viewer)]),
    );
    let params = call_params("tumult_whoami", serde_json::json!({}), None);
    let err = handler
        .handle_call_tool_request(params, stub_runtime_with_bearer("not-a-token"))
        .await
        .expect_err("a wrong header token must be rejected");
    assert!(err.to_string().contains("Unauthorized"), "got: {err}");
    assert!(
        err.to_string().contains("invalid bearer token"),
        "got: {err}"
    );
}

// ── Viewer store_path restriction ──────────────────────────────

/// A viewer's `store_path` override is ignored — viewers always read the
/// default/configured store — while an operator's override is honored.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn viewer_store_path_override_is_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    let default_store = tmp.path().join("default.duckdb");
    drop(tumult_analytics::AnalyticsStore::open(&default_store).unwrap());
    // The viewer's target deliberately does not exist: if the override were
    // honored the call would error on the missing store.
    let evil = tmp.path().join("evil.duckdb");
    std::env::set_var("TUMULT_ANALYTICS_PATH", &default_store);
    let handler = TumultHandler::with_auth(
        tmp.path().to_path_buf(),
        McpAuth::from_tokens(vec![
            ("view-tok".into(), Role::Viewer),
            ("op-tok".into(), Role::Operator),
        ]),
    );

    // Viewer: the override is dropped; the default store answers.
    let params = call_params(
        "tumult_store_stats",
        serde_json::json!({ "store_path": evil.to_str().unwrap() }),
        Some("Bearer view-tok"),
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect("viewer call must dispatch");
    assert!(
        result.is_error.is_none(),
        "viewer must be served by the default store, not the override: {}",
        result_text(&result)
    );

    // Operator: the override is honored and surfaces the missing store.
    let params = call_params(
        "tumult_store_stats",
        serde_json::json!({ "store_path": evil.to_str().unwrap() }),
        Some("Bearer op-tok"),
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect("operator call must dispatch");
    assert_eq!(
        result.is_error,
        Some(true),
        "the operator override must reach the store layer"
    );
    assert!(
        result_text(&result).contains("evil.duckdb"),
        "the operator error must name the honored override: {}",
        result_text(&result)
    );
    std::env::remove_var("TUMULT_ANALYTICS_PATH");
}

// ── Server-wide enactment lock (concurrency veto) ──────────────

/// While an enactment holds the server-wide slot, a second enact attempt
/// gates against `concurrent_experiments = 1` and is vetoed instead of
/// running — two overlapping enact attempts never both execute.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overlapping_enact_attempt_is_vetoed() {
    let tmp = tempfile::tempdir().unwrap();
    let store_path = tmp.path().join("analytics.duckdb");
    let playbook = crate::tools::test_support::write_valid_experiment(tmp.path());
    let policy = tmp.path().join("autopilot.toml");
    std::fs::write(&policy, "[autopilot]\nenabled = true\n").unwrap();
    let hash = tumult_autopilot::policy_hash(&std::fs::read_to_string(&policy).unwrap());
    {
        let store = tumult_analytics::AnalyticsStore::open(&store_path).unwrap();
        let record = tumult_analytics::DecisionRecord {
            id: "d-conc".into(),
            decided_at_ns: 1_000,
            trigger: "staleness".into(),
            service_id: "svc:db".into(),
            tier: Some("data".into()),
            plugin: "tumult-db".into(),
            action: "kill-primary".into(),
            article_id: "compliance:DORA/Art. 25".into(),
            score: 1.5,
            reasons: serde_json::json!([]),
            confidence: "high".into(),
            playbook: Some(playbook),
            validator: serde_json::json!({}),
            verdict: "propose".into(),
            gate_rules: serde_json::json!([]),
            gate_detail: serde_json::json!({}),
            policy_hash: hash,
            autonomy_score: None,
        };
        store.insert_autopilot_decision(&record).unwrap();
    }
    let handler = open_handler(tmp.path());

    // Hold the enactment slot — simulates another enactment in flight.
    let guard = handler.enact_lock.try_acquire().expect("slot starts free");

    let params = call_params(
        "tumult_autopilot_respond",
        serde_json::json!({
            "decision_id": "d-conc",
            "approve": true,
            "policy_path": policy.to_str().unwrap(),
            "store_path": store_path.to_str().unwrap(),
        }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect("respond must dispatch");
    assert_eq!(result.is_error, Some(true));
    let text = result_text(&result);
    assert!(
        text.contains("approval refused by gate re-evaluation"),
        "{text}"
    );
    assert!(text.contains("no_concurrent_experiment"), "{text}");
    assert!(
        !tmp.path().join("autopilot-journals").exists(),
        "a vetoed approval must not run the playbook"
    );
    drop(guard);
}

/// The enactment lock covers the direct execution tools too: while an
/// enactment is in flight, `tumult_run_experiment` refuses fast instead of
/// injecting a second fault.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overlapping_run_experiment_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    crate::tools::test_support::write_valid_experiment(tmp.path());
    let handler = open_handler(tmp.path());
    let guard = handler.enact_lock.try_acquire().expect("slot starts free");

    let params = call_params(
        "tumult_run_experiment",
        serde_json::json!({ "experiment_path": "test.toon", "no_ingest": true }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect("run_experiment must dispatch");
    assert_eq!(result.is_error, Some(true));
    assert!(
        result_text(&result).contains("already running"),
        "got: {}",
        result_text(&result)
    );
    assert!(
        !tmp.path().join("journal.toon").exists(),
        "a refused run must not execute"
    );

    // Once the slot is released the same call runs.
    drop(guard);
    let params = call_params(
        "tumult_run_experiment",
        serde_json::json!({ "experiment_path": "test.toon", "no_ingest": true }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect("run_experiment must dispatch");
    assert!(
        result.is_error.is_none(),
        "released slot must let the run through: {}",
        result_text(&result)
    );
}

// ── Per-client rate limiting ───────────────────────────────────

/// After the burst is exhausted, further requests from the same client are
/// refused before any dispatch work happens.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rate_limiter_refuses_requests_after_burst() {
    let tmp = tempfile::tempdir().unwrap();
    let mut handler = open_handler(tmp.path());
    handler.set_rate_limiter(crate::handler::rate_limit::RateLimiter::new(0.001, 2));

    for attempt in 0..2 {
        let params = call_params("tumult_discover", serde_json::json!({}), None);
        handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .unwrap_or_else(|e| panic!("call {attempt} within burst must succeed: {e}"));
    }
    let params = call_params("tumult_discover", serde_json::json!({}), None);
    let err = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect_err("the request after the burst must be refused");
    assert!(
        err.to_string().contains("rate limit exceeded"),
        "got: {err}"
    );
}
