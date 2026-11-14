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
        kind: TestKind::default(),
        target: None,
        nfr: vec![],
        rto_target_s: None,
        rpo_target_s: None,
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
        kind: TestKind::default(),
        target: None,
        nfr: vec![],
        rto_target_s: None,
        rpo_target_s: None,
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
        kind: TestKind::default(),
        target: None,
        nfr: vec![],
        rto_target_s: None,
        rpo_target_s: None,
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

fn with_method(mut exp: Experiment) -> Experiment {
    exp.method = vec![Activity {
        name: "action".into(),
        ..Default::default()
    }];
    exp
}

fn objective(id: &str) -> NfrObjective {
    NfrObjective {
        id: id.into(),
        kind: NfrKind::Latency,
        metric: "load.latency_p95_ms".into(),
        operator: NfrOperator::Lte,
        threshold: 200.0,
        window_s: None,
        criticality: NfrCriticality::Major,
        description: None,
    }
}

#[test]
fn validate_accepts_well_formed_objectives_and_targets() {
    let exp = with_method(Experiment {
        kind: TestKind::Dr,
        rto_target_s: Some(900.0),
        rpo_target_s: Some(60.0),
        nfr: vec![objective("a"), objective("b")],
        ..Default::default()
    });
    let outcome = validate_experiment(&exp);
    assert!(outcome.is_ok(), "{outcome:?}");
}

#[test]
fn validate_rejects_duplicate_and_malformed_objectives() {
    let dup = with_method(Experiment {
        nfr: vec![objective("a"), objective("a")],
        ..Default::default()
    });
    assert!(matches!(
        validate_experiment(&dup),
        Err(EngineError::NfrDuplicateId { .. })
    ));

    let mut bad = objective("nan");
    bad.threshold = f64::NAN;
    let exp = with_method(Experiment {
        nfr: vec![bad],
        ..Default::default()
    });
    assert!(matches!(
        validate_experiment(&exp),
        Err(EngineError::NfrInvalid { .. })
    ));

    let mut empty = objective("");
    empty.metric = "x".into();
    let exp = with_method(Experiment {
        nfr: vec![empty],
        ..Default::default()
    });
    assert!(validate_experiment(&exp).is_err());

    let mut window = objective("w");
    window.window_s = Some(0.0);
    let exp = with_method(Experiment {
        nfr: vec![window],
        ..Default::default()
    });
    assert!(validate_experiment(&exp).is_err());
}

#[test]
fn validate_rejects_bad_recovery_targets_and_dr_without_rto() {
    let exp = with_method(Experiment {
        rto_target_s: Some(-1.0),
        ..Default::default()
    });
    assert!(matches!(
        validate_experiment(&exp),
        Err(EngineError::InvalidRecoveryTarget { .. })
    ));

    let exp = with_method(Experiment {
        kind: TestKind::Dr,
        ..Default::default()
    });
    assert!(matches!(
        validate_experiment(&exp),
        Err(EngineError::DrRequiresRto)
    ));
}

fn with_load(
    profile: Option<LoadProfile>,
    vus: Option<u32>,
    duration_s: Option<f64>,
) -> Experiment {
    with_method(Experiment {
        load: Some(LoadConfig {
            tool: LoadTool::K6,
            script: "load.js".into(),
            vus,
            duration_s,
            thresholds: HashMap::new(),
            profile,
        }),
        ..Default::default()
    })
}

fn load_error(exp: &Experiment) -> String {
    match validate_experiment(exp) {
        Err(EngineError::LoadProfileInvalid { reason }) => reason,
        other => panic!("expected LoadProfileInvalid, got {other:?}"),
    }
}

#[test]
fn validate_accepts_well_formed_load_profiles() {
    let ramp = LoadProfile::Ramp {
        start_vus: 1,
        ramp_up_s: 30.0,
    };
    assert!(validate_experiment(&with_load(Some(ramp), Some(50), Some(120.0))).is_ok());
    let spike = LoadProfile::Spike {
        peak_vus: 200,
        after_s: 20.0,
        hold_s: 10.0,
    };
    assert!(validate_experiment(&with_load(Some(spike), Some(10), Some(60.0))).is_ok());
    let soak = LoadProfile::Soak {
        sample_interval_s: 10.0,
    };
    assert!(validate_experiment(&with_load(Some(soak), None, Some(3600.0))).is_ok());
    // No profile: a flat load needs no duration.
    assert!(validate_experiment(&with_load(None, None, None)).is_ok());
}

#[test]
fn validate_rejects_a_profile_without_duration() {
    let soak = LoadProfile::Soak {
        sample_interval_s: 10.0,
    };
    assert!(load_error(&with_load(Some(soak), None, None)).contains("duration_s"));
}

#[test]
fn validate_rejects_ramp_that_does_not_fit() {
    let too_long = LoadProfile::Ramp {
        start_vus: 1,
        ramp_up_s: 200.0,
    };
    assert!(load_error(&with_load(Some(too_long), Some(50), Some(120.0))).contains("ramp_up_s"));
    let backwards = LoadProfile::Ramp {
        start_vus: 60,
        ramp_up_s: 10.0,
    };
    assert!(load_error(&with_load(Some(backwards), Some(50), Some(120.0))).contains("start_vus"));
    let nan = LoadProfile::Ramp {
        start_vus: 1,
        ramp_up_s: f64::NAN,
    };
    assert!(load_error(&with_load(Some(nan), Some(50), Some(120.0))).contains("finite"));
}

#[test]
fn validate_rejects_spike_that_does_not_fit_or_does_not_rise() {
    let overflow = LoadProfile::Spike {
        peak_vus: 200,
        after_s: 55.0,
        hold_s: 10.0,
    };
    assert!(
        load_error(&with_load(Some(overflow), Some(10), Some(60.0))).contains("after_s + hold_s")
    );
    let flat = LoadProfile::Spike {
        peak_vus: 10,
        after_s: 5.0,
        hold_s: 10.0,
    };
    assert!(load_error(&with_load(Some(flat), Some(10), Some(60.0))).contains("peak_vus"));
}

#[test]
fn validate_rejects_soak_interval_longer_than_the_run() {
    let soak = LoadProfile::Soak {
        sample_interval_s: 100.0,
    };
    assert!(load_error(&with_load(Some(soak), None, Some(60.0))).contains("sample_interval_s"));
}

#[test]
fn validate_rejects_a_soak_interval_below_one_second() {
    let soak = LoadProfile::Soak {
        sample_interval_s: 0.5,
    };
    let error = load_error(&with_load(Some(soak), None, Some(60.0)));
    assert!(error.contains("at least 1 second"), "{error}");
    let soak = LoadProfile::Soak {
        sample_interval_s: 1.0,
    };
    assert!(validate_experiment(&with_load(Some(soak), None, Some(60.0))).is_ok());
}

#[test]
fn validate_rejects_zero_users_anywhere() {
    assert!(load_error(&with_load(None, Some(0), None)).contains("vus"));
    let ramp = LoadProfile::Ramp {
        start_vus: 0,
        ramp_up_s: 10.0,
    };
    assert!(load_error(&with_load(Some(ramp), Some(5), Some(60.0))).contains("start_vus"));
    let spike = LoadProfile::Spike {
        peak_vus: 0,
        after_s: 1.0,
        hold_s: 1.0,
    };
    assert!(load_error(&with_load(Some(spike), None, Some(60.0))).contains("peak_vus"));
}

#[test]
fn a_target_version_beyond_the_bounds_is_refused() {
    let with_version = |version: &str| Experiment {
        title: "versioned".into(),
        method: vec![Activity {
            name: "action".into(),
            ..Default::default()
        }],
        target: Some(ExperimentTarget {
            system: "checkout".into(),
            technology: None,
            environment: Some("demo".into()),
            criticality: None,
            namespace: None,
            version: Some(version.into()),
        }),
        ..Default::default()
    };
    assert!(validate_experiment(&with_version("1.5.0")).is_ok());
    assert!(validate_experiment(&with_version("  ")).is_ok());
    let long = validate_experiment(&with_version(&"9".repeat(MAX_VERSION_LEN + 1))).unwrap_err();
    assert!(
        long.to_string()
            .contains("target.version is invalid: a version is at most 128 bytes"),
        "{long}"
    );
    let control = validate_experiment(&with_version("1.5.0\u{1b}[2J")).unwrap_err();
    assert!(
        control.to_string().contains("no control characters"),
        "{control}"
    );
}
