use tumult_agentic::adapters::validate_target;
use tumult_agentic::contracts::ContractSpec;
use tumult_agentic::faults::FaultSpec;
use tumult_agentic::model::{
    validate_experiment, AgenticError, AgenticExperiment, AgenticScenario, AgenticTarget,
    CapturePolicy, PrivacyConfig,
};

fn scenario() -> AgenticScenario {
    AgenticScenario {
        name: "support-order-lookup".to_string(),
        input: "order metadata hash:abc123".to_string(),
        expected_behavior: Some("graceful_degradation".to_string()),
    }
}

#[test]
fn privacy_defaults_to_metadata_only_capture() {
    let privacy = PrivacyConfig::default();

    assert_eq!(privacy.capture_policy, CapturePolicy::MetadataOnly);
    assert!(
        privacy.target_allowlist.is_empty(),
        "expected an empty allowlist to preserve existing local-only test ergonomics"
    );
}

#[test]
fn target_allowlist_failure_names_blocked_target() {
    let privacy = PrivacyConfig {
        capture_policy: CapturePolicy::MetadataOnly,
        target_allowlist: vec!["http://127.0.0.1:8080".to_string()],
    };
    let target = AgenticTarget::Http {
        endpoint: "https://api.example.test/agent".to_string(),
    };

    let error = validate_target(&target, &privacy)
        .expect_err("expected target allowlist validation to reject an unlisted HTTP endpoint");

    assert_eq!(
        error.to_string(),
        "target is not allowed: https://api.example.test/agent"
    );
}

#[test]
fn experiment_validation_rejects_empty_matrix_with_clear_feedback() {
    let experiment = AgenticExperiment {
        target: AgenticTarget::Replay {
            fixture: "fixtures/replay.toon".to_string(),
        },
        scenarios: Vec::new(),
        faults: vec![FaultSpec::MalformedOutput { probability: 1.0 }],
        contracts: vec![ContractSpec::ValidJson {
            severity: Some(1.0),
        }],
        privacy: PrivacyConfig::default(),
    };

    let error = validate_experiment(&experiment)
        .expect_err("expected validation to reject an experiment with no scenarios");

    assert_eq!(
        error.to_string(),
        "invalid agentic configuration: at least one scenario is required"
    );
}

#[test]
fn experiment_validation_rejects_invalid_probability() {
    let experiment = AgenticExperiment {
        target: AgenticTarget::Replay {
            fixture: "fixtures/replay.toon".to_string(),
        },
        scenarios: vec![scenario()],
        faults: vec![FaultSpec::MalformedOutput { probability: 1.5 }],
        contracts: vec![ContractSpec::ValidJson {
            severity: Some(1.0),
        }],
        privacy: PrivacyConfig::default(),
    };

    let error = validate_experiment(&experiment)
        .expect_err("expected validation to reject a fault probability above 1.0");

    assert_eq!(
        error,
        AgenticError::InvalidConfig(
            "fault malformed_output probability must be between 0.0 and 1.0".to_string()
        )
    );
}
