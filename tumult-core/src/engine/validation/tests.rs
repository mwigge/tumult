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
        recovery: None,
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
        recovery: None,
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
        recovery: None,
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

fn with_recovery(window_s: f64, interval_s: f64) -> Experiment {
    with_method(Experiment {
        recovery: Some(RecoveryConfig {
            window_s,
            interval_s,
        }),
        ..Default::default()
    })
}

fn recovery_error(exp: &Experiment) -> String {
    match validate_experiment(exp) {
        Err(EngineError::RecoveryInvalid { reason }) => reason,
        other => panic!("expected a recovery error, got {other:?}"),
    }
}

#[test]
fn a_recovery_window_of_up_to_an_hour_is_valid() {
    for (window_s, interval_s) in [(30.0, 1.0), (3600.0, 1.0), (0.5, 0.5), (90.0, 5.0)] {
        let outcome = validate_experiment(&with_recovery(window_s, interval_s));
        assert!(outcome.is_ok(), "{window_s}/{interval_s}: {outcome:?}");
    }
}

#[test]
fn a_recovery_window_above_an_hour_is_refused() {
    for window_s in [3600.5, 86_400.0, f64::INFINITY] {
        let reason = recovery_error(&with_recovery(window_s, 1.0));
        assert!(
            reason.contains("window_s") && reason.contains("3600"),
            "{reason}"
        );
    }
}

#[test]
fn a_recovery_window_and_interval_must_be_above_zero() {
    for window_s in [0.0, -1.0, f64::NAN] {
        let reason = recovery_error(&with_recovery(window_s, 1.0));
        assert_eq!(reason, "window_s must be above 0", "{window_s}");
    }
    for interval_s in [0.0, -0.5, f64::NAN] {
        let reason = recovery_error(&with_recovery(30.0, interval_s));
        assert_eq!(reason, "interval_s must be above 0", "{interval_s}");
    }
}

#[test]
fn a_recovery_interval_longer_than_the_window_is_refused() {
    let reason = recovery_error(&with_recovery(5.0, 10.0));
    assert_eq!(reason, "interval_s must not exceed window_s");
}

/// A run that recovers after its window but within its RTO is breached:
/// validation says so once, and the experiment stays valid.
#[test]
fn an_rto_longer_than_the_recovery_window_is_valid_with_one_warning() {
    let warnings_of = |exp: &Experiment| {
        let captured = tumult_otel::logging::testing::capture_json(tracing::Level::TRACE, || {
            let outcome = validate_experiment(exp);
            assert!(outcome.is_ok(), "{outcome:?}");
        });
        captured
            .events
            .into_iter()
            .filter(|event| event["message"] == "recovery.window.shorter_than_rto")
            .collect::<Vec<_>>()
    };
    let dr = |rto_target_s: f64, recovery: Option<RecoveryConfig>| {
        with_method(Experiment {
            kind: TestKind::Dr,
            rto_target_s: Some(rto_target_s),
            recovery,
            ..Default::default()
        })
    };

    // Without a block the window is 30 seconds.
    let warnings = warnings_of(&dr(60.0, None));
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0]["level"], "WARN");
    assert_eq!(warnings[0]["rto_target_s"], 60.0);
    assert_eq!(warnings[0]["window_s"], 30.0);

    let watched = Some(RecoveryConfig {
        window_s: 60.0,
        interval_s: 1.0,
    });
    assert!(warnings_of(&dr(60.0, watched)).is_empty());
    assert!(warnings_of(&dr(30.0, None)).is_empty());
    assert!(warnings_of(&with_method(Experiment::default())).is_empty());
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

/// A flat load of `tool` with `thresholds`.
fn with_tool(tool: LoadTool, thresholds: &[(&str, f64)]) -> Experiment {
    let mut exp = with_load(None, None, None);
    let load = exp.load.as_mut().expect("a load");
    load.tool = tool;
    load.thresholds = thresholds
        .iter()
        .map(|(name, bound)| ((*name).to_string(), *bound))
        .collect();
    exp
}

/// A load whose tool has no adapter can never run. Validation refuses it
/// with the reason the runner gives, which it asks of the same function.
#[test]
fn a_load_tool_without_an_adapter_is_refused_with_the_reason_of_the_runner() {
    let Err(runner_reason) = crate::runner::load_executor_for(&LoadTool::Jmeter) else {
        panic!("jmeter has no adapter");
    };
    assert_eq!(load_error(&with_tool(LoadTool::Jmeter, &[])), runner_reason);
    assert!(validate_experiment(&with_tool(LoadTool::K6, &[])).is_ok());
    assert!(validate_experiment(&with_tool(LoadTool::Oha, &[])).is_ok());
}

/// oha checks five threshold names; a run with any other name deviates.
/// Validation refuses such a name before the run and names it.
#[test]
fn an_oha_threshold_name_the_adapter_does_not_check_is_refused_and_named() {
    let reason = load_error(&with_tool(LoadTool::Oha, &[("latency_p90_ms", 200.0)]));
    assert!(reason.contains("latency_p90_ms"), "{reason}");
    assert!(reason.contains("oha"), "{reason}");

    let every_known_name = [
        ("latency_p50_ms", 100.0),
        ("latency_p95_ms", 200.0),
        ("latency_p99_ms", 300.0),
        ("error_rate", 0.01),
        ("throughput_rps", 10.0),
    ];
    let outcome = validate_experiment(&with_tool(LoadTool::Oha, &every_known_name));
    assert!(outcome.is_ok(), "{outcome:?}");

    // Of several unknown names the first in order is named, so the reason
    // reads the same on every run.
    let several = [("zz_ms", 1.0), ("aa_ms", 1.0), ("mm_ms", 1.0)];
    let reason = load_error(&with_tool(LoadTool::Oha, &several));
    assert!(reason.contains("'aa_ms'"), "{reason}");
}

/// k6 applies the thresholds of its script, not those of the file. A k6
/// load with thresholds stays valid, and validation says once that they
/// are not applied.
#[test]
fn k6_thresholds_are_accepted_with_one_warning_that_they_are_not_applied() {
    let warnings_of = |exp: &Experiment| {
        let captured = tumult_otel::logging::testing::capture_json(tracing::Level::TRACE, || {
            let outcome = validate_experiment(exp);
            assert!(outcome.is_ok(), "{outcome:?}");
        });
        captured
            .events
            .into_iter()
            .filter(|event| event["message"] == "load.thresholds.not_applied")
            .collect::<Vec<_>>()
    };

    let bounded = with_tool(
        LoadTool::K6,
        &[("latency_p95_ms", 200.0), ("http_req_failed", 0.01)],
    );
    let warnings = warnings_of(&bounded);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0]["tool"], "k6");
    assert_eq!(warnings[0]["level"], "WARN");

    assert!(warnings_of(&with_tool(LoadTool::K6, &[])).is_empty());
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

// ── The metric catalogue ───────────────────────────────────────

fn hypothesis(probe: &str) -> Hypothesis {
    Hypothesis {
        title: "steady".into(),
        probes: vec![Activity {
            name: probe.into(),
            activity_type: ActivityType::Probe,
            ..Default::default()
        }],
    }
}

/// An experiment with one objective on `metric` and a hypothesis probe
/// named `health`.
fn with_metric(metric: &str) -> Experiment {
    with_kind_and_metric(NfrKind::Latency, metric)
}

/// An experiment with one objective of `kind` on `metric` and a
/// hypothesis probe named `health`.
fn with_kind_and_metric(kind: NfrKind, metric: &str) -> Experiment {
    let mut measured = objective("a");
    measured.kind = kind;
    measured.metric = metric.into();
    with_method(Experiment {
        nfr: vec![measured],
        steady_state_hypothesis: Some(hypothesis("health")),
        ..Default::default()
    })
}

#[test]
fn every_metric_of_the_catalogue_is_accepted_with_a_kind_it_measures() {
    for (metric, kind) in [
        ("load.throughput_rps", NfrKind::Throughput),
        ("load.throughput_rps", NfrKind::Capacity),
        ("load.latency_p50_ms", NfrKind::Latency),
        ("load.latency_p95_ms", NfrKind::Latency),
        ("load.latency_p99_ms", NfrKind::Latency),
        ("load.error_rate", NfrKind::ErrorRate),
        ("load.error_rate", NfrKind::Availability),
        ("load.total_requests", NfrKind::Throughput),
        ("recovery.mttr_s", NfrKind::Rto),
        ("recovery.time_s", NfrKind::Recovery),
        ("during.error_rate", NfrKind::ErrorRate),
        ("during.availability", NfrKind::Availability),
        ("post.error_rate", NfrKind::ErrorRate),
        ("post.availability", NfrKind::Availability),
        ("fault.duration_s", NfrKind::Recovery),
        ("probe.health.mean_ms", NfrKind::Latency),
        ("probe.health.max_ms", NfrKind::Latency),
        ("probe.health.p95_ms", NfrKind::Latency),
        ("probe.health.error_rate", NfrKind::ErrorRate),
        ("probe.health.error_rate", NfrKind::DataIntegrity),
    ] {
        let outcome = validate_experiment(&with_kind_and_metric(kind, metric));
        assert!(outcome.is_ok(), "{kind} on {metric}: {outcome:?}");
    }
}

#[test]
fn an_objective_whose_kind_its_metric_does_not_measure_is_refused_and_named() {
    for (kind, metric) in [
        (NfrKind::Latency, "during.availability"),
        (NfrKind::Rto, "load.latency_p95_ms"),
        (NfrKind::Availability, "recovery.mttr_s"),
        (NfrKind::Throughput, "probe.health.p95_ms"),
        (NfrKind::DataIntegrity, "load.error_rate"),
        (NfrKind::Capacity, "fault.duration_s"),
    ] {
        let err = validate_experiment(&with_kind_and_metric(kind, metric)).unwrap_err();
        assert!(
            matches!(err, EngineError::NfrInvalid { .. }),
            "{kind} on {metric}: {err}"
        );
        let named = format!("kind {kind} does not fit metric '{metric}'");
        assert!(err.to_string().contains(&named), "{err}");
    }
}

#[test]
fn a_refused_kind_is_told_the_metrics_that_measure_it() {
    let err = validate_experiment(&with_kind_and_metric(
        NfrKind::Latency,
        "during.availability",
    ))
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "nfr objective 'a' is invalid: kind latency does not fit metric \
         'during.availability', which measures error_rate or availability; the metrics of \
         latency are load.latency_p50_ms, load.latency_p95_ms, load.latency_p99_ms, \
         probe.<name>.mean_ms, probe.<name>.max_ms, probe.<name>.p95_ms"
    );
}

/// No metric of the catalogue counts lost data, so an `rpo` objective has
/// nothing to measure it. Before, any metric passed for one.
#[test]
fn an_rpo_objective_is_refused_on_every_metric_of_the_catalogue() {
    for metric in [
        "load.throughput_rps",
        "load.latency_p95_ms",
        "load.error_rate",
        "load.total_requests",
        "recovery.mttr_s",
        "recovery.time_s",
        "during.availability",
        "post.error_rate",
        "fault.duration_s",
        "probe.health.mean_ms",
        "probe.health.error_rate",
    ] {
        let err = validate_experiment(&with_kind_and_metric(NfrKind::Rpo, metric)).unwrap_err();
        assert!(
            err.to_string()
                .contains("no metric of the catalogue measures rpo"),
            "{metric}: {err}"
        );
    }
}

#[test]
fn a_metric_outside_the_catalogue_is_rejected_and_named() {
    for metric in [
        "load.latency_p90_ms",
        "latency_p95_ms",
        "during.latency",
        "recovery.rto",
        "probe.health",
        "probe.health.p99_ms",
    ] {
        let err = validate_experiment(&with_metric(metric)).unwrap_err();
        assert!(
            matches!(err, EngineError::NfrInvalid { .. }),
            "{metric}: {err}"
        );
        assert!(err.to_string().contains(metric), "{err}");
    }
}

#[test]
fn a_probe_metric_must_name_a_hypothesis_probe() {
    let err = validate_experiment(&with_metric("probe.helth.p95_ms")).unwrap_err();
    assert!(err.to_string().contains("'helth'"), "{err}");

    // A probe name may hold dots: the statistic is what follows the last one.
    let mut dotted = with_metric("probe.api.v2.p95_ms");
    dotted.steady_state_hypothesis = Some(hypothesis("api.v2"));
    let outcome = validate_experiment(&dotted);
    assert!(outcome.is_ok(), "{outcome:?}");
}

// ── Guards and controls run through a provider too ─────────────

fn bad_script() -> Provider {
    Provider::Script {
        plugin: "../outside".into(),
        function: "probe".into(),
        arguments: HashMap::new(),
        timeout_s: None,
    }
}

#[test]
fn validate_checks_the_script_provider_of_a_guard() {
    let mut guard = guard_with_tolerance(
        "watch",
        Some(Tolerance::Exact {
            value: serde_json::json!("ok"),
        }),
        1,
    );
    guard.probe.provider = bad_script();
    let exp = with_method(Experiment {
        guards: vec![guard],
        ..Default::default()
    });
    let err = validate_experiment(&exp).unwrap_err();
    assert!(err.to_string().contains("invalid script provider"), "{err}");
    assert!(err.to_string().contains("watch"), "{err}");
}

#[test]
fn validate_checks_the_script_provider_of_a_control() {
    let exp = with_method(Experiment {
        controls: vec![Control {
            name: "audit".into(),
            provider: bad_script(),
        }],
        ..Default::default()
    });
    let err = validate_experiment(&exp).unwrap_err();
    assert!(err.to_string().contains("invalid script provider"), "{err}");
    assert!(err.to_string().contains("audit"), "{err}");
}

// ── Seconds the runner waits on: timeouts, pauses, a flat load ─

/// An experiment file with a step in every place a step can sit. Each
/// `{place}` holds a number of seconds.
const EVERY_PLACE: &str = "\
title: seconds in every place
controls[1]:
  - name: logging
    provider:
      type: script
      plugin: logger
      function: record
      timeout_s: {control}
steady_state_hypothesis:
  title: the target answers
  probes[1]:
    - name: answers
      activity_type: probe
      provider:
        type: process
        path: sh
        timeout_s: {probe}
      pause_before_s: {probe_pause}
guards[1]:
  - name: safety
    probe:
      name: watch
      activity_type: probe
      provider:
        type: script
        plugin: health
        function: check
        timeout_s: {guard}
      tolerance:
        type: regex
        pattern: ok
      pause_after_s: {guard_pause}
method[2]:
  - name: inject
    activity_type: action
    provider:
      type: process
      path: sh
      timeout_s: {method}
    pause_before_s: {pause_before}
    pause_after_s: {pause_after}
  - name: load-in-background
    activity_type: action
    background: true
    provider:
      type: process
      path: sh
      timeout_s: {background}
rollbacks[1]:
  - name: heal
    activity_type: action
    provider:
      type: process
      path: sh
      timeout_s: {rollback}
";

/// The places of [`EVERY_PLACE`].
const PLACES: &[&str] = &[
    "control",
    "probe",
    "probe_pause",
    "guard",
    "guard_pause",
    "method",
    "pause_before",
    "pause_after",
    "background",
    "rollback",
];

/// [`EVERY_PLACE`] read by the real parser, with `value` in `place` and
/// `others` in every other place.
fn file_with(place: &str, value: &str, others: &str) -> Experiment {
    let text = PLACES.iter().fold(EVERY_PLACE.to_string(), |text, p| {
        let seconds = if *p == place { value } else { others };
        text.replace(&format!("{{{p}}}"), seconds)
    });
    crate::engine::parse_experiment(&text).expect("the file parses")
}

fn seconds_error(exp: &Experiment) -> String {
    validate_experiment(exp)
        .expect_err("a bad number of seconds must be refused")
        .to_string()
}

#[test]
fn a_negative_timeout_is_refused_with_the_step_and_the_value_in_every_place() {
    for (place, owner) in [
        ("method", "activity 'inject'"),
        ("background", "activity 'load-in-background'"),
        ("rollback", "activity 'heal'"),
        ("probe", "activity 'answers'"),
        ("guard", "guard 'safety'"),
        ("control", "control 'logging'"),
    ] {
        assert_eq!(
            seconds_error(&file_with(place, "-1", "5")),
            format!(
                "invalid timeout_s in {owner}: -1 is not a number of seconds from 0 up to 2^64"
            ),
            "{place}"
        );
    }
}

#[test]
fn a_negative_pause_is_refused_with_the_step_and_the_value_in_every_place() {
    for (place, owner, field) in [
        ("pause_before", "activity 'inject'", "pause_before_s"),
        ("pause_after", "activity 'inject'", "pause_after_s"),
        ("probe_pause", "activity 'answers'", "pause_before_s"),
        ("guard_pause", "guard 'safety'", "pause_after_s"),
    ] {
        assert_eq!(
            seconds_error(&file_with(place, "-0.5", "5")),
            format!(
                "invalid {field} in {owner}: -0.5 is not a number of seconds from 0 up to 2^64"
            ),
            "{place}"
        );
    }
}

#[test]
fn a_timeout_too_large_to_wait_on_is_refused() {
    let error = seconds_error(&file_with("method", "1e20", "5"));
    assert!(
        error.starts_with("invalid timeout_s in activity 'inject': 100000000000000000000 is not"),
        "{error}"
    );
}

#[test]
fn a_timeout_that_is_not_a_number_or_is_infinite_is_refused() {
    // A file cannot spell these: the parser reads them as text or refuses
    // them. A caller that builds the experiment in code can.
    for (seconds, shown) in [
        (f64::NAN, "NaN"),
        (f64::INFINITY, "inf"),
        (f64::NEG_INFINITY, "-inf"),
    ] {
        let mut exp = file_with("method", "5", "5");
        exp.method[0].provider = Provider::Process {
            path: "sh".into(),
            arguments: vec![],
            env: HashMap::new(),
            timeout_s: Some(seconds),
        };
        assert_eq!(
            seconds_error(&exp),
            format!("invalid timeout_s in activity 'inject': {shown} is not a number of seconds from 0 up to 2^64")
        );
    }
}

#[test]
fn zero_and_a_fraction_of_a_second_are_accepted_in_every_place() {
    // Guard: these pass today and must keep passing. A zero timeout times
    // the step out at once, and a zero pause does not wait.
    for seconds in ["0", "0.5", "5"] {
        let exp = file_with("method", seconds, seconds);
        let outcome = validate_experiment(&exp);
        assert!(outcome.is_ok(), "{seconds}: {outcome:?}");
    }
}

#[test]
fn a_flat_load_with_a_duration_that_is_not_a_number_of_seconds_is_refused() {
    for (seconds, shown) in [(-30.0, "-30"), (f64::NAN, "NaN"), (f64::INFINITY, "inf")] {
        assert_eq!(
            seconds_error(&with_load(None, None, Some(seconds))),
            format!(
                "invalid duration_s in load: {shown} is not a number of seconds from 0 up to 2^64"
            )
        );
    }
    assert!(validate_experiment(&with_load(None, None, Some(0.0))).is_ok());
    assert!(validate_experiment(&with_load(None, None, Some(30.0))).is_ok());
}

/// An experiment with one method step and a regulatory block of
/// `frameworks` and requirement `ids`.
fn mapped(frameworks: &[&str], ids: &[&str]) -> Experiment {
    Experiment {
        title: "mapped".into(),
        method: vec![Activity {
            name: "action".into(),
            ..Default::default()
        }],
        regulatory: Some(RegulatoryMapping {
            frameworks: frameworks.iter().map(|f| (*f).to_owned()).collect(),
            requirements: ids
                .iter()
                .map(|id| RegulatoryRequirement {
                    id: (*id).to_owned(),
                    description: "d".into(),
                    evidence: "e".into(),
                })
                .collect(),
            tester: None,
        }),
        ..Default::default()
    }
}

#[test]
fn validate_refuses_a_dora_requirement_the_registry_does_not_know_and_names_the_nearest() {
    let err = validate_regulatory(&mapped(&["DORA"], &["Art. 25", "dora.art24.7"])).unwrap_err();
    assert!(
        matches!(&err, EngineError::UnknownRequirement(e) if e.reference == "dora.art24.7"),
        "{err:?}"
    );
    assert_eq!(
        err.to_string(),
        "regulatory requirement: 'dora.art24.7' is not a requirement id of the DORA \
         registry; did you mean 'dora.art24.1'?"
    );
    let bare = validate_regulatory(&mapped(&["DORA"], &["Art. 99"])).unwrap_err();
    assert!(
        bare.to_string().contains("did you mean 'dora.art9'"),
        "{bare}"
    );
}

#[test]
fn validate_accepts_the_registry_id_and_the_old_forms_of_a_run_block() {
    for (frameworks, id) in [
        (&["DORA"][..], "dora.art12.1"),
        (&["DORA"][..], "Art. 11"),
        (&["DORA"][..], "Art. 24(6)"),
        (&["DORA", "NIS2"][..], "DORA-Art24"),
        (&["DORA", "NIS2"][..], "Art. 21(2)(c)"),
        (&["NIS2"][..], "Art. 21(2)(c)"),
        (&["PCI-DSS"][..], "Req. 12.10.2"),
    ] {
        assert!(
            validate_regulatory(&mapped(frameworks, &[id])).is_ok(),
            "{frameworks:?} {id}"
        );
    }
}

#[test]
fn validate_refuses_a_tester_with_no_name() {
    let mut exp = mapped(&["DORA"], &["Art. 25"]);
    if let Some(mapping) = exp.regulatory.as_mut() {
        mapping.tester = Some(Tester {
            name: "  ".into(),
            kind: TesterKind::External,
        });
    }
    let err = validate_regulatory(&exp).unwrap_err();
    assert_eq!(err.to_string(), "regulatory.tester.name must not be empty");
}

#[test]
fn an_experiment_with_a_requirement_the_registry_does_not_know_still_runs() {
    let exp = mapped(&["DORA"], &["DORA-25"]);
    assert!(validate_regulatory(&exp).is_err());
    assert!(validate_experiment(&exp).is_ok());
}
