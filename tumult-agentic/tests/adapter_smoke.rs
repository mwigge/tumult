//! Behavior tests for adapter smoke-target validation and smoke-report
//! feedback lines.

use tumult_agentic::adapters::{adapter_failure_expectation, target_type, validate_target};
use tumult_agentic::model::{
    AgenticError, AgenticRunResult, AgenticTarget, CapturePolicy, PrivacyConfig,
};
use tumult_agentic::smoke::{smoke_failure_output, SmokeReport};

fn privacy(allowlist: &[&str]) -> PrivacyConfig {
    PrivacyConfig {
        capture_policy: CapturePolicy::MetadataOnly,
        target_allowlist: allowlist.iter().map(|s| (*s).to_string()).collect(),
    }
}

#[test]
fn target_type_names_each_target_variant() {
    assert_eq!(
        target_type(&AgenticTarget::Http {
            endpoint: "http://localhost".to_string()
        }),
        "http"
    );
    assert_eq!(
        target_type(&AgenticTarget::Mcp {
            server: "local".to_string(),
            tool: "lookup".to_string()
        }),
        "mcp"
    );
    assert_eq!(
        target_type(&AgenticTarget::Replay {
            fixture: "case.json".to_string()
        }),
        "replay"
    );
}

#[test]
fn validate_target_allows_everything_without_an_allowlist() {
    let target = AgenticTarget::Http {
        endpoint: "http://anything.example".to_string(),
    };
    validate_target(&target, &privacy(&[])).expect("empty allowlist allows all");
}

#[test]
fn validate_target_matches_each_variant_against_the_allowlist() {
    let allowlist = privacy(&["http://allowed", "mcp-server", "fixtures/"]);

    validate_target(
        &AgenticTarget::Http {
            endpoint: "http://allowed/agent".to_string(),
        },
        &allowlist,
    )
    .expect("allowlisted endpoint prefix");
    validate_target(
        &AgenticTarget::Mcp {
            server: "mcp-server-2".to_string(),
            tool: "lookup".to_string(),
        },
        &allowlist,
    )
    .expect("allowlisted server prefix");
    validate_target(
        &AgenticTarget::Replay {
            fixture: "fixtures/case.json".to_string(),
        },
        &allowlist,
    )
    .expect("allowlisted fixture prefix");
}

#[test]
fn validate_target_rejects_values_outside_the_allowlist() {
    let target = AgenticTarget::Http {
        endpoint: "http://evil.example".to_string(),
    };

    let error = validate_target(&target, &privacy(&["http://allowed"]))
        .expect_err("non-allowlisted endpoint must fail");

    assert_eq!(
        error,
        AgenticError::TargetNotAllowed("http://evil.example".to_string())
    );
}

#[test]
fn failure_expectation_renders_a_single_diagnostic_line() {
    let expectation = adapter_failure_expectation(
        "http",
        "tool-timeout",
        "latency",
        "fallback_used",
        "fallback",
        "no fallback",
        "tumult replay run-1",
    );

    assert_eq!(
        expectation.failure_message(),
        "adapter=http scenario=tool-timeout fault=latency contract=fallback_used \
         expected=fallback actual=no fallback next_diagnostic_command=tumult replay run-1"
    );
}

fn report(passed: bool) -> SmokeReport {
    SmokeReport {
        adapter: "http".to_string(),
        scenario: "tool-timeout".to_string(),
        fault: "latency".to_string(),
        contract: "fallback_used".to_string(),
        expected: "fallback".to_string(),
        actual: "no fallback".to_string(),
        next_diagnostic_command: "tumult replay run-1".to_string(),
        passed,
        run_result: AgenticRunResult {
            target_type: "http".to_string(),
            scenarios: Vec::new(),
            faults: Vec::new(),
            contracts: Vec::new(),
            resilience_score: 0.0,
            trace_id: None,
            replay_id: None,
        },
    }
}

#[test]
fn passing_report_feedback_is_a_pass_line_with_no_failure_output() {
    let report = report(true);

    assert!(report.feedback_line().starts_with("pass adapter=http"));
    assert!(smoke_failure_output(&report).is_none());
}

#[test]
fn failing_report_feedback_is_the_failure_output() {
    let report = report(false);

    let line = report.feedback_line();
    assert!(!line.starts_with("pass"));
    assert!(line.contains("contract=fallback_used"));
    // The failure line is the expectation's diagnostic rendering.
    assert_eq!(report.expectation().failure_message(), line);
    assert_eq!(smoke_failure_output(&report), Some(line));
}
