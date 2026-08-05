use super::*;

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
    let store = tumult_lake::AnalyticsStore::open(&store_path).unwrap();
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
    // `tumult_scaffold_experiment` resolves its action against the live
    // fault catalog; point discovery at the workspace's real `plugins/`.
    let plugins = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../plugins")
        .canonicalize()
        .unwrap();
    std::env::set_var("TUMULT_PLUGIN_PATH", plugins);
    let tmp = tempfile::tempdir().unwrap();
    crate::tools::test_support::write_valid_experiment(tmp.path());
    let handler = open_handler(tmp.path());
    let store_path = tmp.path().join("analytics.duckdb");
    let missing_store = tmp.path().join("missing.duckdb");
    drop(tumult_lake::AnalyticsStore::open(&store_path).unwrap());

    // Autopilot fixtures: an enabled policy with no playbooks (a pass over
    // this store yields no candidates — an empty decisions array conforms),
    // an export directory, and a seeded `propose` decision so the respond
    // tool's deny path succeeds deterministically.
    let policy_path = tmp.path().join("autopilot.toml");
    std::fs::write(&policy_path, "[autopilot]\nenabled = true\n").unwrap();
    let export_dir = tmp.path().join("autopilot-export");
    {
        let store = tumult_lake::AnalyticsStore::open(&store_path).unwrap();
        store
            .insert_autopilot_decision(&tumult_lake::DecisionRecord {
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
    let mut expected = crate::handler::output_schema::STRUCTURED_TOOLS.to_vec();
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
