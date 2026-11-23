//! The manual registry over MCP: an agent lists and reads records and
//! plans, and drafts a record. No tool submits, signs off, verifies or
//! attaches: a person does that in the daemon.

use super::*;

use tumult_lake::{ExerciseDetail, ExerciseType, ManualOutcome, NewManualExperiment, Store};

/// A lake in `dir`, migrated, with no record in it.
fn lake(dir: &std::path::Path) -> String {
    let path = dir.join("lake.duckdb");
    Store::open(&path).unwrap();
    path.to_str().unwrap().to_string()
}

/// A record entered by `by` in the lake at `store`; its id.
fn record(
    store: &str,
    name: &str,
    exercise_type: ExerciseType,
    target: &str,
    at_ns: i64,
) -> String {
    let new = NewManualExperiment {
        experiment_name: name.into(),
        exercise_type,
        executed_at_ns: at_ns,
        hypothesis: "The service is back within five minutes".into(),
        method: "Stop the primary".into(),
        outcome: ManualOutcome::Passed,
        hypothesis_met: Some(true),
        findings: None,
        action_items: vec![],
        target_system: Some(target.into()),
        target_environment: Some("production".into()),
        blast_radius: None,
        recovery_time_s: Some(240.0),
        duration_s: None,
        entered_by: "alice".into(),
        attestation: "I attest this record is the exercise as it was run.".into(),
        renewal_due_ns: None,
        framework_refs: vec![],
        exercise: ExerciseDetail::default(),
    };
    let writer = Store::at(std::path::Path::new(store)).writer().unwrap();
    writer.create_manual_draft(&new).unwrap()
}

/// The body of a draft a tool call sends.
fn draft(store: &str) -> serde_json::Value {
    serde_json::json!({
        "store_path": store,
        "experiment_name": "Payments database failover",
        "exercise_type": "failover",
        "executed_at_ns": 1_795_000_000_000_000_000_i64,
        "hypothesis": "The replica takes over within five minutes",
        "method": "Stop the primary and promote the replica",
        "outcome_status": "passed",
        "target_system": "payments-db",
        "target_environment": "production",
        "recovery_time_s": 240.0,
        "rto_target_s": 300.0,
        "entered_by": "alice",
        "attestation": "I attest this record is the exercise as it was run.",
        "participants": [{"name": "alice", "role": "dr_owner"}],
        "steps": [{"title": "Promote the replica", "actor": "bob", "result": "done"}],
        "actions": [{"title": "Write the runbook down", "owner": "carol"}],
        "links": [{"kind": "function", "linked_id": "cif-0001"}]
    })
}

async fn call(
    handler: &TumultHandler,
    name: &str,
    arguments: serde_json::Value,
    authorization: Option<&str>,
) -> std::result::Result<CallToolResult, CallToolError> {
    handler
        .handle_call_tool_request(call_params(name, arguments, authorization), stub_runtime())
        .await
}

/// Call `name` and give its structured content, checked against its
/// output schema.
async fn structured(
    handler: &TumultHandler,
    name: &str,
    arguments: serde_json::Value,
) -> serde_json::Map<String, serde_json::Value> {
    let result = call(handler, name, arguments, None)
        .await
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    assert!(
        result.is_error.is_none(),
        "{name}: {}",
        result_text(&result)
    );
    let content = result
        .structured_content
        .unwrap_or_else(|| panic!("{name} gives no structured content"));
    assert_conforms(name, &content);
    content
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_drafts_a_record_and_reads_it_back() {
    let tmp = tempfile::tempdir().unwrap();
    let store = lake(tmp.path());
    let handler = open_handler(tmp.path());

    let created = structured(&handler, "tumult_manual_create_draft", draft(&store)).await;
    let id = created["id"].as_str().expect("an id").to_string();
    assert_eq!(created["status"], "draft");

    let listed = structured(
        &handler,
        "tumult_manual_list",
        serde_json::json!({"store_path": store}),
    )
    .await;
    assert_eq!(listed["total"], 1, "{listed:?}");
    assert_eq!(listed["items"][0]["id"], id.as_str());
    assert_eq!(listed["items"][0]["status"], "draft");

    let detail = structured(
        &handler,
        "tumult_manual_get",
        serde_json::json!({"store_path": store, "id": id}),
    )
    .await;
    assert_eq!(detail["experiment"]["entered_by"], "alice");
    assert_eq!(detail["experiment"]["rto_target_s"], 300.0);
    assert_eq!(detail["steps"][0]["title"], "Promote the replica");
    assert_eq!(detail["actions"][0]["owner"], "carol");
    assert_eq!(detail["links"][0]["linked_id"], "cif-0001");
    assert_eq!(detail["signoffs"], serde_json::json!([]));

    let text = result_text(
        &call(
            &handler,
            "tumult_manual_get",
            serde_json::json!({"store_path": store, "id": id}),
            None,
        )
        .await
        .unwrap(),
    );
    assert!(text.contains("Payments database failover"), "{text}");
    assert!(text.contains("draft"), "{text}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_draft_cannot_carry_a_status_a_reviewer_or_a_field_the_api_does_not_know() {
    let tmp = tempfile::tempdir().unwrap();
    let store = lake(tmp.path());
    let handler = open_handler(tmp.path());

    for (field, value) in [
        ("status", serde_json::json!("verified")),
        ("reviewed_by", serde_json::json!("bob")),
        ("rpo_lost_s", serde_json::json!(0)),
    ] {
        let mut body = draft(&store);
        body[field] = value;
        let refused = call(&handler, "tumult_manual_create_draft", body, None)
            .await
            .expect_err("a field the record body does not have is refused");
        assert!(refused.to_string().contains(field), "{field}: {refused}");
    }
    let mut step = draft(&store);
    step["steps"][0]["signed_by"] = serde_json::json!("bob");
    let refused = call(&handler, "tumult_manual_create_draft", step, None)
        .await
        .expect_err("a nested field the record body does not have is refused");
    assert!(refused.to_string().contains("signed_by"), "{refused}");

    let listed = structured(
        &handler,
        "tumult_manual_list",
        serde_json::json!({"store_path": store}),
    )
    .await;
    assert_eq!(listed["total"], 0, "nothing was written: {listed:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_draft_the_lake_refuses_is_a_tool_error_that_says_why() {
    let tmp = tempfile::tempdir().unwrap();
    let store = lake(tmp.path());
    let handler = open_handler(tmp.path());
    let mut body = draft(&store);
    body["exercise_type"] = serde_json::json!("picnic");

    let result = call(&handler, "tumult_manual_create_draft", body, None)
        .await
        .unwrap();

    assert_eq!(result.is_error, Some(true));
    assert!(
        result_text(&result).contains("picnic"),
        "{}",
        result_text(&result)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_tool_submits_signs_off_verifies_or_attaches() {
    let tmp = tempfile::tempdir().unwrap();
    let handler = open_handler(tmp.path());
    let listed = handler
        .handle_list_tools_request(None, stub_runtime())
        .await
        .unwrap();

    let manual: Vec<&str> = listed
        .tools
        .iter()
        .map(|t| t.name.as_str())
        .filter(|name| name.contains("manual"))
        .collect();
    assert_eq!(
        manual,
        [
            "tumult_manual_list",
            "tumult_manual_get",
            "tumult_manual_create_draft",
            "tumult_manual_plans_list"
        ]
    );
    for tool in &listed.tools {
        for word in [
            "submit", "sign", "verify", "reject", "approve", "attach", "upload",
        ] {
            assert!(
                !tool.name.contains(word),
                "{} looks like a tool that does what a person must do",
                tool.name
            );
        }
    }
    for tool in listed.tools.iter().filter(|t| t.name.contains("manual")) {
        let description = tool.description.as_deref().unwrap_or_default();
        assert!(
            description.contains("an agent may draft, a person signs"),
            "{}: {description}",
            tool.name
        );
    }
    for name in [
        "tumult_manual_submit",
        "tumult_manual_sign_off",
        "tumult_manual_verify",
        "tumult_manual_attach",
    ] {
        let refused = call(&handler, name, serde_json::json!({"id": "x"}), None)
            .await
            .expect_err("there is no such tool");
        assert!(refused.to_string().contains(name), "{name}: {refused}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_viewer_lists_and_reads_but_only_an_operator_drafts() {
    let tmp = tempfile::tempdir().unwrap();
    let store = lake(tmp.path());
    let _home = private_home(tmp.path());
    let handler = TumultHandler::with_auth(
        tmp.path().to_path_buf(),
        McpAuth::from_tokens(vec![
            ("view-tok".into(), Role::Viewer),
            ("op-tok".into(), Role::Operator),
        ]),
    );

    let refused = call(
        &handler,
        "tumult_manual_create_draft",
        draft(&store),
        Some("Bearer view-tok"),
    )
    .await
    .expect_err("a viewer does not draft");
    assert!(refused.to_string().contains("operator"), "{refused}");

    let made = call(
        &handler,
        "tumult_manual_create_draft",
        draft(&store),
        Some("Bearer op-tok"),
    )
    .await
    .unwrap();
    assert!(made.is_error.is_none(), "{}", result_text(&made));

    // A viewer reads the store of the server, not a path it names: here
    // `TUMULT_LAKE_PATH`, the same lake.
    let read = call(
        &handler,
        "tumult_manual_list",
        serde_json::json!({"store_path": "/somewhere/else.duckdb"}),
        Some("Bearer view-tok"),
    )
    .await
    .unwrap();
    assert!(read.is_error.is_none(), "{}", result_text(&read));
    assert_eq!(read.structured_content.unwrap()["total"], 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_list_narrows_by_status_type_target_and_time() {
    let tmp = tempfile::tempdir().unwrap();
    let store = lake(tmp.path());
    let handler = open_handler(tmp.path());
    // 2026-11-18 and 2026-11-06, UTC.
    let failover = record(
        &store,
        "Failover",
        ExerciseType::Failover,
        "payments-db",
        1_795_000_000_000_000_000,
    );
    let restore = record(
        &store,
        "Restore",
        ExerciseType::Restore,
        "ledger",
        1_794_000_000_000_000_000,
    );

    let ids = |listed: serde_json::Map<String, serde_json::Value>| -> Vec<String> {
        listed["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap().to_string())
            .collect()
    };
    for (filter, want) in [
        (
            serde_json::json!({"exercise_type": "restore"}),
            vec![restore.clone()],
        ),
        (
            serde_json::json!({"target": "payments-db"}),
            vec![failover.clone()],
        ),
        (serde_json::json!({"status": "submitted"}), vec![]),
        (
            serde_json::json!({"from": "2026-11-10"}),
            vec![failover.clone()],
        ),
        (
            serde_json::json!({"to": "2026-11-06"}),
            vec![restore.clone()],
        ),
        (
            serde_json::json!({"from": "2026-11-18T00:00:00Z", "to": "2026-11-19"}),
            vec![failover.clone()],
        ),
    ] {
        let mut arguments = filter.clone();
        arguments["store_path"] = serde_json::json!(store);
        let listed = structured(&handler, "tumult_manual_list", arguments).await;
        assert_eq!(ids(listed), want, "{filter}");
    }
    let page = structured(
        &handler,
        "tumult_manual_list",
        serde_json::json!({"store_path": store, "limit": 1, "offset": 1}),
    )
    .await;
    assert_eq!(page["total"], 2);
    assert_eq!(page["items"].as_array().map(Vec::len), Some(1));

    let bad = call(
        &handler,
        "tumult_manual_list",
        serde_json::json!({"store_path": store, "from": "next week"}),
        None,
    )
    .await
    .unwrap();
    assert_eq!(bad.is_error, Some(true));
    assert!(
        result_text(&bad).contains("next week"),
        "{}",
        result_text(&bad)
    );
    let typo = call(
        &handler,
        "tumult_manual_list",
        serde_json::json!({"store_path": store, "type": "restore"}),
        None,
    )
    .await
    .expect_err("a filter the tool does not have is refused");
    assert!(typo.to_string().contains("type"), "{typo}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_record_that_is_not_there_is_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let store = lake(tmp.path());
    let handler = open_handler(tmp.path());

    let result = call(
        &handler,
        "tumult_manual_get",
        serde_json::json!({"store_path": store, "id": "01NOSUCHRECORD"}),
        None,
    )
    .await
    .unwrap();

    assert_eq!(result.is_error, Some(true));
    assert!(
        result_text(&result).contains("01NOSUCHRECORD"),
        "{}",
        result_text(&result)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plans_are_listed_soonest_due_first() {
    let tmp = tempfile::tempdir().unwrap();
    let store = lake(tmp.path());
    let handler = open_handler(tmp.path());
    let writer = Store::at(std::path::Path::new(&store)).writer().unwrap();
    let plan = |name: &str, target: &str, due_at_ns: i64| tumult_lake::NewExercisePlan {
        name: name.into(),
        exercise_type: ExerciseType::Failover,
        target_system: Some(target.into()),
        target_environment: Some("production".into()),
        function_id: None,
        scope: None,
        owner: "alice".into(),
        planned_at_ns: None,
        due_at_ns,
        interval_days: Some(365),
        rto_target_s: Some(300.0),
        rpo_target_s: None,
    };
    let later = writer
        .create_manual_plan(
            &plan("Later", "payments-db", 1_800_000_000_000_000_000),
            "alice",
        )
        .unwrap();
    let sooner = writer
        .create_manual_plan(
            &plan("Sooner", "ledger", 1_799_000_000_000_000_000),
            "alice",
        )
        .unwrap();
    drop(writer);

    let listed = structured(
        &handler,
        "tumult_manual_plans_list",
        serde_json::json!({"store_path": store}),
    )
    .await;
    let ids: Vec<&str> = listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [sooner.as_str(), later.as_str()]);
    let one = structured(
        &handler,
        "tumult_manual_plans_list",
        serde_json::json!({"store_path": store, "target": "payments-db", "status": "planned"}),
    )
    .await;
    assert_eq!(one["total"], 1);
    assert_eq!(one["items"][0]["id"], later.as_str());
}

#[test]
fn the_draft_schema_is_the_record_body_of_the_api_and_nothing_more() {
    let schema = ManualCreateDraftTool::json_schema();
    let mut fields: Vec<&str> = schema["properties"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .filter(|field| *field != "store_path")
        .collect();
    fields.sort_unstable();
    let mut body = vec![
        "experiment_name",
        "exercise_type",
        "executed_at_ns",
        "hypothesis",
        "method",
        "outcome_status",
        "hypothesis_met",
        "findings",
        "action_items",
        "target_system",
        "target_environment",
        "blast_radius",
        "recovery_time_s",
        "duration_s",
        "entered_by",
        "attestation",
        "renewal_due_ns",
        "framework_refs",
        "rto_target_s",
        "rpo_target_s",
        "rpo_measured_s",
        "data_verified",
        "participants",
        "steps",
        "actions",
        "links",
        "plan_id",
    ];
    body.sort_unstable();
    assert_eq!(fields, body);
    let required: Vec<&str> = schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    for field in [
        "experiment_name",
        "exercise_type",
        "executed_at_ns",
        "hypothesis",
        "method",
        "outcome_status",
        "entered_by",
        "attestation",
    ] {
        assert!(
            required.contains(&field),
            "{field} is required: {required:?}"
        );
    }
    let step = &schema["properties"]["steps"]["items"]["properties"];
    assert!(step.get("result").is_some(), "{step}");
}
