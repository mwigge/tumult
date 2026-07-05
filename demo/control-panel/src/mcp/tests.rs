use super::*;
use serde_json::json;

#[test]
fn builds_jsonrpc_request_envelope() {
    let r = build_rpc_request(7, "tools/list", json!({"a": 1}));
    assert_eq!(r["jsonrpc"], "2.0");
    assert_eq!(r["id"], 7);
    assert_eq!(r["method"], "tools/list");
    assert_eq!(r["params"]["a"], 1);
}

#[test]
fn notification_has_no_id() {
    let n = build_notification("notifications/initialized", json!({}));
    assert_eq!(n["jsonrpc"], "2.0");
    assert!(n.get("id").is_none());
    assert_eq!(n["method"], "notifications/initialized");
}

#[test]
fn tools_call_params_shape() {
    let p = tools_call_params(
        "tumult_run_experiment",
        json!({"experiment_path": "x.toon"}),
    );
    assert_eq!(p["name"], "tumult_run_experiment");
    assert_eq!(p["arguments"]["experiment_path"], "x.toon");
}

#[test]
fn parses_sse_framed_payload() {
    let body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"ok\":true}}\n\n";
    let v = parse_sse_body(body).unwrap();
    assert_eq!(v["result"]["ok"], true);
}

#[test]
fn parses_multiline_sse_data() {
    let body = "data: {\"jsonrpc\":\"2.0\",\ndata: \"id\":1,\"result\":{\"n\":5}}\n\n";
    let v = parse_sse_body(body).unwrap();
    assert_eq!(v["result"]["n"], 5);
}

#[test]
fn parses_plain_json_body() {
    let body = "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"x\":9}}";
    let v = parse_sse_body(body).unwrap();
    assert_eq!(v["result"]["x"], 9);
}

#[test]
fn empty_body_is_transport_error() {
    assert!(matches!(parse_sse_body(""), Err(McpError::Transport(_))));
}

#[test]
fn rpc_error_maps_to_rpc_variant() {
    let env = json!({"jsonrpc":"2.0","id":1,"error":{"code":-32600,"message":"bad"}});
    let err = rpc_result(&env).unwrap_err();
    match err {
        McpError::Rpc(m) => assert!(m.contains("bad") && m.contains("-32600")),
        other => panic!("expected Rpc, got {other:?}"),
    }
}

#[test]
fn rpc_result_extracts_result() {
    let env = json!({"jsonrpc":"2.0","id":1,"result":{"tools":[]}});
    assert!(rpc_result(&env).unwrap().get("tools").is_some());
}

#[test]
fn reads_tool_annotations() {
    let result = json!({
        "tools": [
            {
                "name": "tumult_run_experiment",
                "description": "Execute a Tumult chaos experiment.",
                "annotations": { "destructiveHint": true, "readOnlyHint": false }
            },
            {
                "name": "tumult_validate",
                "description": "Validate an experiment file.",
                "annotations": { "destructiveHint": false, "readOnlyHint": true }
            },
            { "name": "tumult_no_annotations", "description": "n/a" }
        ]
    });
    let tools = parse_tools_list(&result);
    assert_eq!(tools.len(), 3);
    let run = tools
        .iter()
        .find(|t| t.name == "tumult_run_experiment")
        .unwrap();
    assert!(run.destructive);
    assert!(!run.read_only);
    let val = tools.iter().find(|t| t.name == "tumult_validate").unwrap();
    assert!(!val.destructive);
    assert!(val.read_only);
    // Missing annotations default to non-destructive / non-read-only.
    let none = tools
        .iter()
        .find(|t| t.name == "tumult_no_annotations")
        .unwrap();
    assert!(!none.destructive);
    assert!(!none.read_only);
}

#[test]
fn parses_completed_run_as_passed() {
    // Mirrors Wave A/B run_experiment structuredContent shape.
    let result = json!({
        "content": [{"type":"text","text":"status: completed"}],
        "isError": false,
        "structuredContent": {
            "journal": { "status": "completed", "duration_ms": 228 },
            "journal_path": "/demo/journals/demo-net.toon",
            "ingestion": "ingested"
        }
    });
    let out = parse_run_result(&result).unwrap();
    assert_eq!(out.status, "completed");
    assert_eq!(out.outcome, "passed");
    assert_eq!(out.duration_ms, Some(228));
    assert_eq!(
        out.journal_path.as_deref(),
        Some("/demo/journals/demo-net.toon")
    );
    assert_eq!(out.ingestion.as_deref(), Some("ingested"));
}

#[test]
fn deviated_and_failed_verdicts() {
    let deviated = json!({
        "structuredContent": { "journal": { "status": "deviated", "duration_ms": 10 } }
    });
    assert_eq!(parse_run_result(&deviated).unwrap().outcome, "deviated");
    let failed = json!({
        "structuredContent": { "journal": { "status": "aborted", "duration_ms": 3 } }
    });
    assert_eq!(parse_run_result(&failed).unwrap().outcome, "failed");
}

#[test]
fn tool_level_error_is_protocol_error() {
    let result = json!({
        "isError": true,
        "content": [{"type":"text","text":"experiment file not found"}]
    });
    match parse_run_result(&result) {
        Err(McpError::Protocol(m)) => assert!(m.contains("not found")),
        other => panic!("expected Protocol error, got {other:?}"),
    }
}

#[test]
fn missing_structured_content_is_protocol_error() {
    let result = json!({ "content": [] });
    assert!(matches!(
        parse_run_result(&result),
        Err(McpError::Protocol(_))
    ));
}

// ── Halted status (auto-halt) ─────────────────────────────

#[test]
fn halted_status_maps_to_halted_verdict() {
    assert_eq!(verdict_for("halted"), "halted");
    let result = json!({
        "structuredContent": {
            "journal": { "status": "halted", "duration_ms": 42 },
            "journal_path": "/demo/journals/demo-postgres.toon",
            "ingestion": "ingested"
        }
    });
    let out = parse_run_result(&result).unwrap();
    assert_eq!(out.status, "halted");
    assert_eq!(out.outcome, "halted");
    assert_eq!(out.duration_ms, Some(42));
}

// ── discover ──────────────────────────────────────────────

#[test]
fn parses_discover_counts() {
    let result = json!({
        "isError": false,
        "content": [{
            "type": "text",
            "text": "Plugins: 3\n  a (native)\n  b (script)\n  c (native)\nActions: 5\n  a::x\n  a::y\n  b::z\n  c::p\n  c::q\n"
        }]
    });
    let d = parse_discover_result(&result).unwrap();
    assert_eq!(d.plugins, 3);
    assert_eq!(d.actions, 5);
}

#[test]
fn discover_missing_counts_is_protocol_error() {
    let result = json!({ "content": [{"type":"text","text":"nothing here"}] });
    assert!(matches!(
        parse_discover_result(&result),
        Err(McpError::Protocol(_))
    ));
}

// ── validate ──────────────────────────────────────────────

#[test]
fn parses_valid_experiment_summary() {
    let result = json!({
        "isError": false,
        "content": [{
            "type": "text",
            "text": "Valid: 'Kill Postgres connections' — 3 method steps, 1 rollbacks"
        }]
    });
    let v = parse_validate_result(&result).unwrap();
    assert!(v.valid);
    assert_eq!(v.title.as_deref(), Some("Kill Postgres connections"));
    assert_eq!(v.method_steps, 3);
    assert_eq!(v.rollbacks, 1);
}

#[test]
fn invalid_experiment_is_protocol_error() {
    let result = json!({
        "isError": true,
        "content": [{"type":"text","text":"validation error: unknown provider 'nope'"}]
    });
    match parse_validate_result(&result) {
        Err(McpError::Protocol(m)) => assert!(m.contains("unknown provider")),
        other => panic!("expected Protocol error, got {other:?}"),
    }
}

// ── analyze_store ─────────────────────────────────────────

#[test]
fn parses_analyze_store_table() {
    let result = json!({
        "isError": false,
        "content": [{
            "type": "text",
            "text": "title\tstatus\tduration_ms\nKill connections\thalted\t42\nAdd latency\tcompleted\t228\n2 row(s)"
        }]
    });
    let t = parse_analyze_store_result(&result).unwrap();
    assert_eq!(t.columns, vec!["title", "status", "duration_ms"]);
    assert_eq!(t.row_count, 2);
    assert_eq!(t.rows[0], vec!["Kill connections", "halted", "42"]);
    assert_eq!(t.rows[1], vec!["Add latency", "completed", "228"]);
}

#[test]
fn analyze_store_empty_result_has_no_rows() {
    let result = json!({
        "content": [{"type":"text","text":"title\tstatus\n0 row(s)"}]
    });
    let t = parse_analyze_store_result(&result).unwrap();
    assert_eq!(t.columns, vec!["title", "status"]);
    assert_eq!(t.row_count, 0);
    assert!(t.rows.is_empty());
}

#[test]
fn analyze_store_tool_error_is_protocol_error() {
    let result = json!({
        "isError": true,
        "content": [{"type":"text","text":"store not found: /x/analytics.db"}]
    });
    assert!(matches!(
        parse_analyze_store_result(&result),
        Err(McpError::Protocol(_))
    ));
}

// ── recommend ─────────────────────────────────────────────

#[test]
fn parses_recommendations_from_structured_content() {
    let result = json!({
        "isError": false,
        "structuredContent": {
            "recommendations": [
                { "rank": 1, "title": "Test Postgres failover", "rationale": "never exercised" },
                { "rank": 2, "title": "Add network latency", "rationale": "low coverage" }
            ]
        }
    });
    let r = parse_recommend_result(&result).unwrap();
    assert!(r.message.is_none());
    assert_eq!(r.recommendations.len(), 2);
    assert_eq!(r.recommendations[0].rank, 1);
    assert_eq!(r.recommendations[0].title, "Test Postgres failover");
    assert_eq!(r.recommendations[1].rationale, "low coverage");
}

#[test]
fn recommend_message_when_no_store() {
    let result = json!({
        "structuredContent": { "message": "No analytics store found. Run some experiments first." }
    });
    let r = parse_recommend_result(&result).unwrap();
    assert_eq!(
        r.message.as_deref(),
        Some("No analytics store found. Run some experiments first.")
    );
    assert!(r.recommendations.is_empty());
}

#[test]
fn recommend_tool_error_is_protocol_error() {
    let result = json!({
        "isError": true,
        "content": [{"type":"text","text":"intelligence backend unavailable"}]
    });
    assert!(matches!(
        parse_recommend_result(&result),
        Err(McpError::Protocol(_))
    ));
}

// ── compliance ────────────────────────────────────────────

#[test]
fn parses_compliance_structured_content() {
    let result = json!({
        "isError": false,
        "structuredContent": {
            "framework": "DORA",
            "pass_rate": 0.875,
            "recovery_compliance": 0.9,
            "verdict": "COMPLIANT",
            "journals_evaluated": 8,
            "disclaimer": "Evidence toward controls, not a compliance determination.",
            "source_url": "https://eur-lex.europa.eu/eli/reg/2022/2554/oj",
            "citations": [
                { "control_id": "Art. 24", "title": "Testing", "requires": "x", "evidence_type": "y", "strength": "direct", "evidence_note": "z", "source_url": "u", "last_verified": "2025-01-01" },
                { "control_id": "Art. 25", "title": "Scenario testing", "requires": "x", "evidence_type": "y", "strength": "supporting", "evidence_note": "z", "source_url": "u", "last_verified": "2025-01-01" }
            ]
        }
    });
    let c = parse_compliance_result(&result).unwrap();
    assert_eq!(c.framework, "DORA");
    assert!((c.pass_rate - 0.875).abs() < 1e-9);
    assert_eq!(c.recovery_compliance, Some(0.9));
    assert_eq!(c.verdict, "COMPLIANT");
    assert_eq!(c.journals_evaluated, 8);
    assert!(c.disclaimer.contains("not a compliance determination"));
    assert_eq!(c.citations.len(), 2);
    assert_eq!(c.citations[0].control_id, "Art. 24");
    assert_eq!(c.citations[1].strength, "supporting");
}

#[test]
fn compliance_pass_rate_only_has_null_recovery() {
    let result = json!({
        "structuredContent": {
            "framework": "DORA",
            "pass_rate": 1.0,
            "recovery_compliance": null,
            "verdict": "COMPLIANT (pass-rate only)",
            "journals_evaluated": 1,
            "disclaimer": "scope note"
        }
    });
    let c = parse_compliance_result(&result).unwrap();
    assert_eq!(c.recovery_compliance, None);
    assert!(c.verdict.contains("pass-rate only"));
    assert!(c.citations.is_empty());
}

#[test]
fn compliance_tool_error_is_protocol_error() {
    let result = json!({
        "isError": true,
        "content": [{"type":"text","text":"unknown framework 'nope'"}]
    });
    assert!(matches!(
        parse_compliance_result(&result),
        Err(McpError::Protocol(_))
    ));
}

// ── chaosgraph ────────────────────────────────────────────

#[test]
fn parses_graph_query_nodes() {
    let result = json!({
        "structuredContent": {
            "kind": "fault",
            "count": 2,
            "nodes": [
                { "id": "fault:tumult-net::inject_latency", "kind": "fault", "label": "net latency" },
                { "id": "fault:tumult-ssh::execute", "kind": "fault", "label": "ssh execute" }
            ]
        }
    });
    let q = parse_graph_query_result(&result).unwrap();
    assert_eq!(q.kind, "fault");
    assert_eq!(q.count, 2);
    assert_eq!(q.nodes.len(), 2);
    assert_eq!(q.nodes[0].id, "fault:tumult-net::inject_latency");
}

#[test]
fn parses_graph_neighbors_edges() {
    let result = json!({
        "structuredContent": {
            "node_id": "exp:Kill Postgres connections",
            "depth": 1,
            "nodes": [
                { "id": "exp:Kill Postgres connections", "kind": "experiment", "label": "Kill Postgres connections" },
                { "id": "fault:tumult-db-postgres::kill", "kind": "fault", "label": "kill" }
            ],
            "edges": [
                { "src": "exp:Kill Postgres connections", "rel": "injects", "dst": "fault:tumult-db-postgres::kill" }
            ]
        }
    });
    let ego = parse_graph_neighbors_result(&result).unwrap();
    assert_eq!(ego.node_id, "exp:Kill Postgres connections");
    assert_eq!(ego.depth, 1);
    assert_eq!(ego.nodes.len(), 2);
    assert_eq!(ego.edges.len(), 1);
    assert_eq!(ego.edges[0].rel, "injects");
}

#[test]
fn parses_coverage_gaps_with_framework() {
    let result = json!({
        "structuredContent": {
            "count": 1,
            "gaps": [
                { "id": "gap:tumult-net::partition", "plugin": "tumult-net", "action": "partition", "domain": "domain:tumult-net" }
            ],
            "framework": "DORA",
            "unevidenced_articles": [
                { "id": "art:dora:11", "control_id": "Art. 11", "strength": "direct" }
            ]
        }
    });
    let g = parse_coverage_gaps_result(&result).unwrap();
    assert_eq!(g.count, 1);
    assert_eq!(g.gaps[0].plugin, "tumult-net");
    assert_eq!(g.framework.as_deref(), Some("DORA"));
    assert_eq!(g.unevidenced_articles.len(), 1);
    assert_eq!(g.unevidenced_articles[0].control_id, "Art. 11");
}

#[test]
fn coverage_gaps_without_framework_has_no_articles() {
    let result = json!({
        "structuredContent": { "count": 0, "gaps": [] }
    });
    let g = parse_coverage_gaps_result(&result).unwrap();
    assert_eq!(g.count, 0);
    assert!(g.gaps.is_empty());
    assert!(g.framework.is_none());
    assert!(g.unevidenced_articles.is_empty());
}

#[test]
fn graph_tool_error_is_protocol_error() {
    let result = json!({
        "isError": true,
        "content": [{"type":"text","text":"no analytics store found"}]
    });
    assert!(matches!(
        parse_graph_query_result(&result),
        Err(McpError::Protocol(_))
    ));
}

// ── fault_catalog ─────────────────────────────────────────

#[test]
fn parses_fault_catalog_structured_content() {
    let result = json!({
        "isError": false,
        "content": [{"type":"text","text":"full catalog in structuredContent"}],
        "structuredContent": {
            "action_count": 3,
            "domains": [
                {
                    "domain": "network",
                    "label": "Network",
                    "actions": [
                        {
                            "plugin": "tumult-network",
                            "name": "add-latency",
                            "description": "Inject latency on the network path.",
                            "kind": "action",
                            "args": [
                                { "name": "delay_ms", "required": true, "description": "milliseconds of delay" },
                                { "name": "jitter_ms", "required": false, "description": "delay jitter" }
                            ]
                        },
                        {
                            "plugin": "tumult-network",
                            "name": "http-probe",
                            "description": "HTTP steady-state probe.",
                            "kind": "probe",
                            "args": []
                        }
                    ]
                },
                {
                    "domain": "database",
                    "label": "Database",
                    "actions": [
                        {
                            "plugin": "tumult-db-postgres",
                            "name": "kill-connections",
                            "description": "Kill active connections.",
                            "kind": "action",
                            "args": []
                        }
                    ]
                }
            ]
        }
    });
    let c = parse_catalog_result(&result).unwrap();
    assert_eq!(c.action_count, 3);
    assert_eq!(c.domains.len(), 2);
    let net = &c.domains[0];
    assert_eq!(net.domain, "network");
    assert_eq!(net.label, "Network");
    assert_eq!(net.actions.len(), 2);
    let lat = &net.actions[0];
    assert_eq!(lat.plugin, "tumult-network");
    assert_eq!(lat.name, "add-latency");
    assert_eq!(lat.kind, "action");
    assert_eq!(lat.args.len(), 2);
    assert_eq!(lat.args[0].name, "delay_ms");
    assert!(lat.args[0].required);
    assert!(!lat.args[1].required);
    // Probe kind is carried through and args may be empty.
    assert_eq!(net.actions[1].kind, "probe");
    assert!(net.actions[1].args.is_empty());
    assert_eq!(c.domains[1].actions[0].plugin, "tumult-db-postgres");
}

#[test]
fn catalog_missing_structured_content_is_protocol_error() {
    let result = json!({ "content": [{"type":"text","text":"n/a"}] });
    assert!(matches!(
        parse_catalog_result(&result),
        Err(McpError::Protocol(_))
    ));
}

#[test]
fn catalog_tool_error_is_protocol_error() {
    let result = json!({
        "isError": true,
        "content": [{"type":"text","text":"plugin discovery failed"}]
    });
    match parse_catalog_result(&result) {
        Err(McpError::Protocol(m)) => assert!(m.contains("discovery")),
        other => panic!("expected Protocol error, got {other:?}"),
    }
}

// ── scaffold_experiment ───────────────────────────────────

#[test]
fn parses_valid_scaffold_result() {
    let result = json!({
        "isError": false,
        "content": [{"type":"text","text":"version: 0.1\n…"}],
        "structuredContent": {
            "action": "tumult-network::add-latency",
            "toon": "version: 0.1\ntitle: add-latency — demo-app\n",
            "valid": true
        }
    });
    let s = parse_scaffold_result(&result).unwrap();
    assert_eq!(s.action, "tumult-network::add-latency");
    assert!(s.toon.contains("add-latency"));
    assert!(s.valid);
    assert!(s.validation_error.is_none());
}

#[test]
fn parses_invalid_scaffold_result_without_erroring() {
    // A scaffold that fails validation is reported in-band (valid: false),
    // NOT as a tool error — the UI badges the validation message.
    let result = json!({
        "isError": false,
        "structuredContent": {
            "action": "tumult-network::add-latency",
            "toon": "version: 0.1\n…",
            "valid": false,
            "validation_error": "missing required arg 'delay_ms'"
        }
    });
    let s = parse_scaffold_result(&result).unwrap();
    assert!(!s.valid);
    assert_eq!(
        s.validation_error.as_deref(),
        Some("missing required arg 'delay_ms'")
    );
}

#[test]
fn scaffold_missing_structured_content_is_protocol_error() {
    let result = json!({ "content": [] });
    assert!(matches!(
        parse_scaffold_result(&result),
        Err(McpError::Protocol(_))
    ));
}

#[test]
fn scaffold_tool_error_is_protocol_error() {
    let result = json!({
        "isError": true,
        "content": [{"type":"text","text":"provide `plugin`, or a fully-qualified `action`"}]
    });
    match parse_scaffold_result(&result) {
        Err(McpError::Protocol(m)) => assert!(m.contains("plugin")),
        other => panic!("expected Protocol error, got {other:?}"),
    }
}

#[test]
fn parses_whoami_operator_and_viewer() {
    let op = json!({ "structuredContent": { "role": "operator", "authenticated": true } });
    assert_eq!(
        parse_whoami_result(&op).unwrap(),
        WhoamiOutcome {
            role: "operator".into(),
            authenticated: true
        }
    );
    let view = json!({ "structuredContent": { "role": "viewer", "authenticated": true } });
    assert_eq!(
        parse_whoami_result(&view).unwrap(),
        WhoamiOutcome {
            role: "viewer".into(),
            authenticated: true
        }
    );
    // Open mode (no auth configured): unauthenticated, defaults to false.
    let open = json!({ "structuredContent": { "role": "operator" } });
    assert_eq!(
        parse_whoami_result(&open).unwrap(),
        WhoamiOutcome {
            role: "operator".into(),
            authenticated: false
        }
    );
}

#[test]
fn whoami_missing_role_is_protocol_error() {
    let result = json!({ "structuredContent": { "authenticated": true } });
    assert!(matches!(
        parse_whoami_result(&result),
        Err(McpError::Protocol(_))
    ));
}

#[test]
fn whoami_tool_error_is_protocol_error() {
    let result = json!({
        "isError": true,
        "content": [{"type":"text","text":"Unauthorized: invalid bearer token"}]
    });
    assert!(matches!(
        parse_whoami_result(&result),
        Err(McpError::Protocol(_))
    ));
}
