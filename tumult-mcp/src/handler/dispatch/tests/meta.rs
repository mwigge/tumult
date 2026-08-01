use super::*;

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
        let expected =
            crate::handler::output_schema::STRUCTURED_TOOLS.contains(&tool.name.as_str());
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
