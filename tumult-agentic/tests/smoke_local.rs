use tumult_agentic::smoke::{
    fake_http_malformed_json_smoke, fake_mcp_tool_failure_smoke, replay_validation_smoke,
    run_local_smoke_suite, smoke_failure_output,
};

#[test]
fn smoke_fake_http_malformed_json_feedback_is_actionable() {
    let report = fake_http_malformed_json_smoke().expect("fake HTTP smoke should run");

    assert!(report.passed);
    assert_eq!(report.adapter, "fake_http");
    assert_eq!(report.scenario, "fake-http-malformed-json");
    assert_eq!(report.fault, "malformed_output");
    assert_eq!(report.contract, "valid_json");
    assert_eq!(report.expected, "contract_failed:invalid_json");
    assert_eq!(report.actual, "contract_failed:invalid_json");

    let feedback = report.feedback_line();
    assert!(feedback.contains("adapter=fake_http"));
    assert!(feedback.contains("scenario=fake-http-malformed-json"));
    assert!(feedback.contains("fault=malformed_output"));
    assert!(feedback.contains("contract=valid_json"));
    assert!(feedback.contains("expected=contract_failed:invalid_json"));
    assert!(feedback.contains("actual=contract_failed:invalid_json"));
    assert!(feedback.contains(
        "next_diagnostic_command=cargo test -p tumult-agentic smoke_fake_http -- --nocapture"
    ));
    assert!(smoke_failure_output(&report).is_none());
}

#[test]
fn smoke_fake_mcp_tool_failure_feedback_is_actionable() {
    let report = fake_mcp_tool_failure_smoke().expect("fake MCP smoke should run");

    assert!(report.passed);
    assert_eq!(report.adapter, "fake_mcp");
    assert_eq!(report.fault, "tool_failure");
    assert_eq!(report.contract, "graceful_error");
    assert_eq!(report.expected, "contract_passed");
    assert_eq!(report.actual, "contract_passed");
    assert!(report.feedback_line().contains("adapter=fake_mcp"));
    assert!(report.feedback_line().contains(
        "next_diagnostic_command=cargo test -p tumult-agentic smoke_fake_mcp -- --nocapture"
    ));
}

#[test]
fn smoke_replay_validation_feedback_is_actionable() {
    let report = replay_validation_smoke().expect("replay smoke should run");

    assert!(report.passed);
    assert_eq!(report.adapter, "replay");
    assert_eq!(report.fault, "replay_validation");
    assert_eq!(report.contract, "missing_output_ref");
    assert_eq!(report.expected, "incomplete_replay_rejected");
    assert_eq!(report.actual, "incomplete_replay_rejected");
    assert!(report.feedback_line().contains("adapter=replay"));
    assert!(report.feedback_line().contains(
        "next_diagnostic_command=cargo test -p tumult-agentic replay_validation -- --nocapture"
    ));
}

#[test]
fn smoke_local_suite_exercises_http_mcp_and_replay() {
    let reports = run_local_smoke_suite().expect("local smoke suite should run");
    let adapters = reports
        .iter()
        .map(|report| report.adapter.as_str())
        .collect::<Vec<_>>();

    assert_eq!(adapters, vec!["fake_http", "fake_mcp", "replay"]);
    assert!(reports.iter().all(|report| report.passed));
}
