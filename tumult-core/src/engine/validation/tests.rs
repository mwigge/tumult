use super::*;
use crate::types::*;
use indexmap::IndexMap;
use std::collections::HashMap;

#[test]
fn validate_rejects_unsupported_version() {
    let exp = Experiment {
        version: "v2".into(),
        title: "version-test".into(),
        method: vec![Activity {
            name: "action".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let err = validate_experiment(&exp).unwrap_err();
    assert!(err.to_string().contains("unsupported experiment version"));
}

#[test]
fn validate_accepts_v1_version() {
    let exp = Experiment {
        version: "v1".into(),
        title: "version-test".into(),
        method: vec![Activity {
            name: "action".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    assert!(validate_experiment(&exp).is_ok());
}

#[test]
fn validate_rejects_empty_method() {
    let exp = Experiment {
        version: "v1".into(),
        title: "empty".into(),
        description: None,
        tags: vec![],
        configuration: IndexMap::new(),
        secrets: IndexMap::new(),
        controls: vec![],
        steady_state_hypothesis: None,
        method: vec![],
        rollbacks: vec![],
        estimate: None,
        baseline: None,
        load: None,
        regulatory: None,
        guards: vec![],
        blast_radius: None,
        max_concurrent_faults: None,
    };
    assert!(validate_experiment(&exp).is_err());
}

#[test]
fn validate_rejects_empty_hypothesis_probes() {
    let exp = Experiment {
        version: "v1".into(),
        title: "empty-probes".into(),
        description: None,
        tags: vec![],
        configuration: IndexMap::new(),
        secrets: IndexMap::new(),
        controls: vec![],
        steady_state_hypothesis: Some(Hypothesis {
            title: "System is healthy".into(),
            probes: vec![], // Empty probes
        }),
        method: vec![Activity {
            name: "test-action".into(),
            activity_type: ActivityType::Action,
            provider: Provider::Native {
                plugin: "test".into(),
                function: "noop".into(),
                arguments: HashMap::new(),
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
            label_selector: None,
        }],
        rollbacks: vec![],
        estimate: None,
        baseline: None,
        load: None,
        regulatory: None,
        guards: vec![],
        blast_radius: None,
        max_concurrent_faults: None,
    };
    let err = validate_experiment(&exp).unwrap_err();
    assert!(err.to_string().contains("no probes"));
}

#[test]
fn validate_accepts_experiment_with_method() {
    let exp = Experiment {
        version: "v1".into(),
        title: "valid".into(),
        description: None,
        tags: vec![],
        configuration: IndexMap::new(),
        secrets: IndexMap::new(),
        controls: vec![],
        steady_state_hypothesis: None,
        method: vec![Activity {
            name: "test-action".into(),
            activity_type: ActivityType::Action,
            provider: Provider::Native {
                plugin: "test".into(),
                function: "noop".into(),
                arguments: HashMap::new(),
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
            label_selector: None,
        }],
        rollbacks: vec![],
        estimate: None,
        baseline: None,
        load: None,
        regulatory: None,
        guards: vec![],
        blast_radius: None,
        max_concurrent_faults: None,
    };
    assert!(validate_experiment(&exp).is_ok());
}

fn guard_with_tolerance(name: &str, tolerance: Option<Tolerance>, min_breaches: u32) -> Guard {
    Guard {
        name: name.into(),
        probe: Activity {
            name: format!("{name}-probe"),
            activity_type: ActivityType::Probe,
            tolerance,
            ..Default::default()
        },
        min_breaches,
    }
}

#[test]
fn validate_accepts_well_formed_guard() {
    let exp = Experiment {
        version: "v1".into(),
        title: "guarded".into(),
        guards: vec![guard_with_tolerance(
            "slo",
            Some(Tolerance::Range {
                from: 0.0,
                to: 0.05,
            }),
            2,
        )],
        method: vec![Activity {
            name: "inject".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    assert!(validate_experiment(&exp).is_ok());
}

#[test]
fn validate_rejects_guard_without_tolerance() {
    let exp = Experiment {
        version: "v1".into(),
        title: "guarded".into(),
        guards: vec![guard_with_tolerance("slo", None, 1)],
        method: vec![Activity {
            name: "inject".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let err = validate_experiment(&exp).unwrap_err();
    assert!(err.to_string().contains("no tolerance"), "{err}");
}

#[test]
fn validate_rejects_guard_with_zero_min_breaches() {
    let exp = Experiment {
        version: "v1".into(),
        title: "guarded".into(),
        guards: vec![guard_with_tolerance(
            "slo",
            Some(Tolerance::Range { from: 0.0, to: 1.0 }),
            0,
        )],
        method: vec![Activity {
            name: "inject".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let err = validate_experiment(&exp).unwrap_err();
    assert!(err.to_string().contains("min_breaches"), "{err}");
}

#[test]
fn validate_rejects_guard_with_bad_regex() {
    let exp = Experiment {
        version: "v1".into(),
        title: "guarded".into(),
        guards: vec![guard_with_tolerance(
            "slo",
            Some(Tolerance::Regex {
                pattern: "(unclosed".into(),
            }),
            1,
        )],
        method: vec![Activity {
            name: "inject".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let err = validate_experiment(&exp).unwrap_err();
    assert!(err.to_string().contains("invalid regex"), "{err}");
}

#[test]
fn validate_rejects_guard_with_inverted_range() {
    let exp = Experiment {
        version: "v1".into(),
        title: "guarded".into(),
        guards: vec![guard_with_tolerance(
            "slo",
            Some(Tolerance::Range { from: 1.0, to: 0.0 }),
            1,
        )],
        method: vec![Activity {
            name: "inject".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let err = validate_experiment(&exp).unwrap_err();
    assert!(err.to_string().contains("invalid tolerance range"), "{err}");
}

#[test]
fn validate_rejects_zero_max_concurrent_faults() {
    let exp = Experiment {
        version: "v1".into(),
        title: "zero-cap".into(),
        max_concurrent_faults: Some(0),
        method: vec![Activity {
            name: "inject".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let err = validate_experiment(&exp).unwrap_err();
    assert!(err.to_string().contains("max_concurrent_faults"), "{err}");
}

#[test]
fn validate_accepts_nonzero_max_concurrent_faults() {
    let exp = Experiment {
        version: "v1".into(),
        title: "capped".into(),
        max_concurrent_faults: Some(2),
        method: vec![Activity {
            name: "inject".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    assert!(validate_experiment(&exp).is_ok());
}

fn script_experiment(plugin: &str, function: &str) -> Experiment {
    Experiment {
        version: "v1".into(),
        title: "script-provider".into(),
        method: vec![Activity {
            name: "inject".into(),
            provider: Provider::Script {
                plugin: plugin.into(),
                function: function.into(),
                arguments: HashMap::new(),
                timeout_s: None,
            },
            ..Default::default()
        }],
        ..Default::default()
    }
}

#[test]
fn validate_accepts_well_formed_script_provider() {
    assert!(validate_experiment(&script_experiment("tumult-network", "redirect-dns")).is_ok());
    assert!(validate_experiment(&script_experiment("tumult-kafka", "kill_broker-2")).is_ok());
}

#[test]
fn validate_rejects_script_provider_bad_names() {
    let cases = [
        ("", "redirect-dns"),
        ("tumult network", "redirect-dns"),
        ("tumult/network", "redirect-dns"),
        ("tumult\\network", "redirect-dns"),
        ("..", "redirect-dns"),
        ("tumult-..-network", "redirect-dns"),
        ("tumult-network", ""),
        ("tumult-network", "redirect dns"),
        ("tumult-network", "../redirect-dns"),
        ("tumult-network", "redirect/dns"),
    ];
    for (plugin, function) in cases {
        let err = validate_experiment(&script_experiment(plugin, function)).unwrap_err();
        assert!(
            err.to_string().contains("invalid script provider"),
            "plugin={plugin:?} function={function:?} should be rejected, got: {err}"
        );
    }
}

#[test]
fn validate_rejects_script_provider_null_bytes() {
    let err =
        validate_experiment(&script_experiment("tumult\0network", "redirect-dns")).unwrap_err();
    assert!(err.to_string().contains("null bytes"), "{err}");
    let err =
        validate_experiment(&script_experiment("tumult-network", "redirect\0dns")).unwrap_err();
    assert!(err.to_string().contains("null bytes"), "{err}");
}

#[test]
fn validate_checks_script_provider_in_rollbacks_and_hypothesis() {
    let mut exp = script_experiment("tumult-network", "redirect-dns");
    exp.rollbacks.push(Activity {
        name: "cleanup".into(),
        provider: Provider::Script {
            plugin: "bad plugin".into(),
            function: "reset-tc".into(),
            arguments: HashMap::new(),
            timeout_s: None,
        },
        ..Default::default()
    });
    let err = validate_experiment(&exp).unwrap_err();
    assert!(err.to_string().contains("invalid script provider"), "{err}");

    let mut exp = script_experiment("tumult-network", "redirect-dns");
    exp.steady_state_hypothesis = Some(Hypothesis {
        title: "dns resolves".into(),
        probes: vec![Activity {
            name: "probe".into(),
            activity_type: ActivityType::Probe,
            provider: Provider::Script {
                plugin: "tumult-network".into(),
                function: String::new(),
                arguments: HashMap::new(),
                timeout_s: None,
            },
            ..Default::default()
        }],
    });
    let err = validate_experiment(&exp).unwrap_err();
    assert!(err.to_string().contains("must not be empty"), "{err}");
}
