//! Static validation of experiment definitions before execution.

use crate::types::{Experiment, Tolerance};

use super::EngineError;

/// Validate an experiment definition before execution.
///
/// Checks: method is non-empty, regex patterns compile, hypothesis probes exist.
///
/// # Errors
///
/// Returns [`EngineError::UnsupportedVersion`] if the experiment version is not `"v1"`.
/// Returns [`EngineError::EmptyMethod`] if the method contains no steps.
/// Returns [`EngineError::EmptyHypothesisProbes`] if the hypothesis has no probes.
/// Returns [`EngineError::InvalidRegex`] if a regex tolerance pattern fails to compile.
/// Returns [`EngineError::InvalidToleranceBounds`] if a range tolerance has lower > upper.
///
/// # Examples
///
/// ```
/// use tumult_core::engine::validate_experiment;
/// use tumult_core::types::*;
/// use std::collections::HashMap;
/// use indexmap::IndexMap;
///
/// let experiment = Experiment {
///     version: "v1".into(),
///     title: "validate-demo".into(),
///     description: None,
///     tags: vec![],
///     configuration: IndexMap::new(),
///     secrets: IndexMap::new(),
///     controls: vec![],
///     steady_state_hypothesis: None,
///     guards: vec![],
///     blast_radius: None,
///     max_concurrent_faults: None,
///     method: vec![Activity {
///         name: "action-1".into(),
///         activity_type: ActivityType::Action,
///         provider: Provider::Native {
///             plugin: "test".into(),
///             function: "noop".into(),
///             arguments: HashMap::new(),
///         },
///         tolerance: None,
///         pause_before_s: None,
///         pause_after_s: None,
///         background: false,
///         label_selector: None,
///     }],
///     rollbacks: vec![],
///     estimate: None,
///     baseline: None,
///     load: None,
///     regulatory: None,
/// };
///
/// assert!(validate_experiment(&experiment).is_ok());
///
/// // An experiment with no method steps fails validation
/// let empty = Experiment {
///     version: "v1".into(),
///     title: "empty".into(),
///     description: None,
///     tags: vec![],
///     configuration: IndexMap::new(),
///     secrets: IndexMap::new(),
///     controls: vec![],
///     steady_state_hypothesis: None,
///     guards: vec![],
///     blast_radius: None,
///     max_concurrent_faults: None,
///     method: vec![],
///     rollbacks: vec![],
///     estimate: None,
///     baseline: None,
///     load: None,
///     regulatory: None,
/// };
///
/// assert!(validate_experiment(&empty).is_err());
/// ```
pub fn validate_experiment(experiment: &Experiment) -> Result<(), EngineError> {
    // Version check — only "v1" is supported
    if experiment.version != "v1" {
        return Err(EngineError::UnsupportedVersion {
            version: experiment.version.clone(),
        });
    }

    if experiment.method.is_empty() {
        return Err(EngineError::EmptyMethod);
    }

    // Validate hypothesis has probes if defined
    if let Some(ref hypothesis) = experiment.steady_state_hypothesis {
        if hypothesis.probes.is_empty() {
            return Err(EngineError::EmptyHypothesisProbes {
                title: hypothesis.title.clone(),
            });
        }
    }

    // Validate guards: each guard's probe must carry a tolerance (the safe
    // condition), min_breaches must be at least 1, and any regex/range
    // tolerance must be well-formed.
    for guard in &experiment.guards {
        if guard.min_breaches == 0 {
            return Err(EngineError::GuardInvalidMinBreaches {
                guard: guard.name.clone(),
            });
        }
        match &guard.probe.tolerance {
            None => {
                return Err(EngineError::GuardMissingTolerance {
                    guard: guard.name.clone(),
                });
            }
            Some(Tolerance::Regex { pattern }) => {
                if regex_lite::Regex::new(pattern).is_err() {
                    return Err(EngineError::InvalidRegex {
                        activity: guard.name.clone(),
                        pattern: pattern.clone(),
                    });
                }
            }
            Some(Tolerance::Range { from, to }) if from > to => {
                return Err(EngineError::InvalidToleranceBounds {
                    activity: guard.name.clone(),
                    from: *from,
                    to: *to,
                });
            }
            Some(_) => {}
        }
    }

    // Validate all regex tolerance patterns compile
    let all_activities = experiment
        .method
        .iter()
        .chain(experiment.rollbacks.iter())
        .chain(
            experiment
                .steady_state_hypothesis
                .as_ref()
                .map(|h| h.probes.iter())
                .into_iter()
                .flatten(),
        );
    for activity in all_activities {
        match &activity.tolerance {
            Some(Tolerance::Regex { pattern }) => {
                if regex_lite::Regex::new(pattern).is_err() {
                    return Err(EngineError::InvalidRegex {
                        activity: activity.name.clone(),
                        pattern: pattern.clone(),
                    });
                }
            }
            Some(Tolerance::Range { from, to }) if from > to => {
                return Err(EngineError::InvalidToleranceBounds {
                    activity: activity.name.clone(),
                    from: *from,
                    to: *to,
                });
            }
            _ => {}
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
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
}
