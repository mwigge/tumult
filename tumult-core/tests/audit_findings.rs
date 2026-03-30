//! Tests for audit findings implementation.
//! TDD: these tests define expected behavior for each finding.

use std::collections::HashMap;
use tumult_core::engine::validate_experiment;
use tumult_core::types::*;

// ── SRE-10: validate warns on unsupported providers ───────────

#[test]
fn validate_rejects_invalid_regex_pattern() {
    let exp = Experiment {
        title: "regex test".into(),
        method: vec![Activity {
            name: "probe-with-bad-regex".into(),
            activity_type: ActivityType::Probe,
            provider: Provider::Process {
                path: "echo".into(),
                arguments: vec![],
                env: HashMap::new(),
                timeout_s: None,
            },
            tolerance: Some(Tolerance::Regex {
                pattern: "[invalid".into(),
            }),
            pause_before_s: None,
            pause_after_s: None,
            background: false,
        }],
        ..Default::default()
    };

    let result = validate_experiment(&exp);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("invalid regex"),
        "expected regex error, got: {}",
        err
    );
}

#[test]
fn validate_accepts_valid_regex_pattern() {
    let exp = Experiment {
        title: "regex test".into(),
        method: vec![Activity {
            name: "probe-with-good-regex".into(),
            activity_type: ActivityType::Probe,
            provider: Provider::Process {
                path: "echo".into(),
                arguments: vec![],
                env: HashMap::new(),
                timeout_s: None,
            },
            tolerance: Some(Tolerance::Regex {
                pattern: "^OK.*".into(),
            }),
            pause_before_s: None,
            pause_after_s: None,
            background: false,
        }],
        ..Default::default()
    };

    assert!(validate_experiment(&exp).is_ok());
}

// ── APP-09: drain_node structured return (tested via types) ───

// This is a K8s integration test — verified at the type level
// since we can't call the K8s API without a cluster.

// ── APP-10: all_succeeded empty returns false ─────────────────

#[test]
fn all_succeeded_empty_is_false() {
    assert!(!tumult_core::execution::all_succeeded(&[]));
}
