use tumult_agentic::adapters::AgentResponse;
use tumult_agentic::contracts::{evaluate_contract, ContractSpec};

fn response(body: &str) -> AgentResponse {
    AgentResponse {
        body: body.to_string(),
        latency_ms: 125,
        tool_calls: 2,
        retry_count: 1,
        input_tokens: 30,
        output_tokens: 40,
        fallback_used: false,
    }
}

#[test]
fn valid_json_contract_reports_clear_invalid_json_label() {
    let outcome = evaluate_contract(
        "fake-http-malformed-json",
        &ContractSpec::ValidJson {
            severity: Some(2.0),
        },
        &response("{malformed-json"),
    );

    assert!(!outcome.passed);
    assert_eq!(outcome.contract_type, "valid_json");
    assert_eq!(outcome.reason.as_deref(), Some("invalid_json"));
    assert_eq!(outcome.severity, 2.0);
}

#[test]
fn no_pii_contract_redacts_email_evidence() {
    let outcome = evaluate_contract(
        "support-order-lookup",
        &ContractSpec::NoPii { severity: None },
        &response(r#"{"email":"customer@example.test"}"#),
    );

    assert!(!outcome.passed);
    assert_eq!(outcome.reason.as_deref(), Some("pii_detected:email"));
    assert!(
        !format!("{outcome:?}").contains("customer@example.test"),
        "expected contract evidence to omit raw PII"
    );
}

#[test]
fn no_secret_contract_reports_secret_family_not_secret_value() {
    let outcome = evaluate_contract(
        "support-order-lookup",
        &ContractSpec::NoSecretLeakage { severity: None },
        &response(r#"{"token":"sk-live-secret-value"}"#),
    );

    assert!(!outcome.passed);
    assert_eq!(outcome.reason.as_deref(), Some("secret_detected:api_key"));
    assert!(
        !format!("{outcome:?}").contains("sk-live-secret-value"),
        "expected contract evidence to omit raw secret values"
    );
}

#[test]
fn deterministic_operational_contracts_use_expected_labels() {
    let target = AgentResponse {
        body: "panic: failed".to_string(),
        latency_ms: 501,
        tool_calls: 4,
        retry_count: 3,
        input_tokens: 100,
        output_tokens: 120,
        fallback_used: false,
    };

    let contracts = [
        ContractSpec::MaxLatency {
            max_ms: 500,
            severity: None,
        },
        ContractSpec::RetryBudget {
            max_retries: 2,
            severity: None,
        },
        ContractSpec::MaxToolCalls {
            max_calls: 3,
            severity: None,
        },
        ContractSpec::MaxTokenUsage {
            max_tokens: 200,
            severity: None,
        },
        ContractSpec::FallbackUsed { severity: None },
        ContractSpec::GracefulError { severity: None },
        ContractSpec::RequiredCitation { severity: None },
    ];

    let reasons = contracts
        .iter()
        .map(|contract| evaluate_contract("support-order-lookup", contract, &target).reason)
        .collect::<Vec<_>>();

    assert_eq!(
        reasons,
        vec![
            Some("latency_exceeded".to_string()),
            Some("retry_budget_exceeded".to_string()),
            Some("tool_call_budget_exceeded".to_string()),
            Some("token_budget_exceeded".to_string()),
            Some("fallback_not_used".to_string()),
            Some("ungraceful_error".to_string()),
            Some("citation_missing".to_string()),
        ]
    );
}
