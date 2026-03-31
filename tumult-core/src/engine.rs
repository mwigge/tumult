//! Experiment engine — orchestrates the five-phase execution lifecycle.

use std::collections::HashMap;

use indexmap::IndexMap;

use crate::types::{ConfigValue, Experiment, ExperimentStatus, SecretValue, Tolerance};

use thiserror::Error;

#[derive(Error, Debug)]
pub enum EngineError {
    #[error("experiment has no method steps")]
    EmptyMethod,
    #[error("configuration key '{key}' references env var '{env_key}' which is not set")]
    ConfigResolutionFailed { key: String, env_key: String },
    #[error("secret '{group}.{key}' references env var '{env_key}' which is not set")]
    SecretResolutionFailed {
        group: String,
        key: String,
        env_key: String,
    },
    #[error("secret '{group}.{key}' references file '{path}' which does not exist")]
    SecretFileNotFound {
        group: String,
        key: String,
        path: String,
    },
    #[error("experiment file parse error: {0}")]
    ParseError(String),
    #[error("invalid regex pattern in activity '{activity}': {pattern}")]
    InvalidRegex { activity: String, pattern: String },
    #[error(
        "invalid tolerance range in activity '{activity}': lower ({from}) must be <= upper ({to})"
    )]
    InvalidToleranceBounds {
        activity: String,
        from: f64,
        to: f64,
    },
    #[error("hypothesis '{title}' has no probes defined")]
    EmptyHypothesisProbes { title: String },
    #[error("unsupported experiment version '{version}' (supported: v1)")]
    UnsupportedVersion { version: String },
}

/// Resolve configuration values by reading environment variables.
pub fn resolve_config(
    config: &IndexMap<String, ConfigValue>,
) -> Result<HashMap<String, String>, EngineError> {
    let mut resolved = HashMap::new();
    for (key, value) in config {
        match value {
            ConfigValue::Env { key: env_key } => {
                let val =
                    std::env::var(env_key).map_err(|_| EngineError::ConfigResolutionFailed {
                        key: key.clone(),
                        env_key: env_key.clone(),
                    })?;
                resolved.insert(key.clone(), val);
            }
            ConfigValue::Inline { value } => {
                resolved.insert(key.clone(), value.clone());
            }
        }
    }
    Ok(resolved)
}

/// Validate an experiment definition before execution.
///
/// Checks: method is non-empty, regex patterns compile, hypothesis probes exist.
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
            Some(Tolerance::Range { from, to }) => {
                if from > to {
                    return Err(EngineError::InvalidToleranceBounds {
                        activity: activity.name.clone(),
                        from: *from,
                        to: *to,
                    });
                }
            }
            _ => {}
        }
    }

    Ok(())
}

/// Parse an experiment from a TOON string.
pub fn parse_experiment(toon: &str) -> Result<Experiment, EngineError> {
    toon_format::decode_default(toon).map_err(|e| EngineError::ParseError(e.to_string()))
}

/// Resolve secret values by reading environment variables or files.
pub fn resolve_secrets(
    secrets: &IndexMap<String, IndexMap<String, SecretValue>>,
) -> Result<HashMap<String, HashMap<String, String>>, EngineError> {
    let mut resolved = HashMap::new();
    for (group, group_secrets) in secrets {
        let mut group_resolved = HashMap::new();
        for (key, value) in group_secrets {
            let val = match value {
                SecretValue::Env { key: env_key } => {
                    std::env::var(env_key).map_err(|_| EngineError::SecretResolutionFailed {
                        group: group.clone(),
                        key: key.clone(),
                        env_key: env_key.clone(),
                    })?
                }
                SecretValue::File { path } => {
                    if !path.exists() {
                        return Err(EngineError::SecretFileNotFound {
                            group: group.clone(),
                            key: key.clone(),
                            path: path.display().to_string(),
                        });
                    }
                    std::fs::read_to_string(path).map_err(|_| EngineError::SecretFileNotFound {
                        group: group.clone(),
                        key: key.clone(),
                        path: path.display().to_string(),
                    })?
                }
            };
            group_resolved.insert(key.clone(), val);
        }
        resolved.insert(group.clone(), group_resolved);
    }
    Ok(resolved)
}

/// Evaluate a tolerance check: does the actual value match the expected?
pub fn evaluate_tolerance(actual: &serde_json::Value, tolerance: &Tolerance) -> bool {
    match tolerance {
        Tolerance::Exact { value } => actual == value,
        Tolerance::Range { from, to } => {
            if let Some(n) = actual.as_f64() {
                n >= *from && n <= *to
            } else {
                false
            }
        }
        Tolerance::Regex { pattern } => {
            if let Some(s) = actual.as_str() {
                regex_lite::Regex::new(pattern)
                    .map(|re| re.is_match(s))
                    .unwrap_or(false)
            } else {
                false
            }
        }
    }
}

/// Determine the experiment status from method results.
pub fn determine_status(
    hypothesis_before_met: Option<bool>,
    hypothesis_after_met: Option<bool>,
    all_actions_succeeded: bool,
) -> ExperimentStatus {
    if let Some(false) = hypothesis_before_met {
        return ExperimentStatus::Aborted;
    }
    if !all_actions_succeeded {
        return ExperimentStatus::Failed;
    }
    if let Some(false) = hypothesis_after_met {
        return ExperimentStatus::Deviated;
    }
    ExperimentStatus::Completed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;
    use indexmap::IndexMap;
    use std::collections::HashMap;

    // ── resolve_config ─────────────────────────────────────────

    #[test]
    fn resolve_inline_config() {
        let config = IndexMap::from([(
            "db_host".into(),
            ConfigValue::Inline {
                value: "localhost".into(),
            },
        )]);
        let resolved = resolve_config(&config).unwrap();
        assert_eq!(resolved.get("db_host").unwrap(), "localhost");
    }

    #[test]
    fn resolve_env_config() {
        std::env::set_var("TEST_TUMULT_DB_HOST", "prod-db.example.com");
        let config = IndexMap::from([(
            "db_host".into(),
            ConfigValue::Env {
                key: "TEST_TUMULT_DB_HOST".into(),
            },
        )]);
        let resolved = resolve_config(&config).unwrap();
        assert_eq!(resolved.get("db_host").unwrap(), "prod-db.example.com");
        std::env::remove_var("TEST_TUMULT_DB_HOST");
    }

    #[test]
    fn resolve_missing_env_returns_error() {
        std::env::remove_var("NONEXISTENT_VAR_TUMULT_TEST");
        let config = IndexMap::from([(
            "db_host".into(),
            ConfigValue::Env {
                key: "NONEXISTENT_VAR_TUMULT_TEST".into(),
            },
        )]);
        let result = resolve_config(&config);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("NONEXISTENT_VAR_TUMULT_TEST"));
    }

    #[test]
    fn resolve_empty_config_succeeds() {
        let resolved = resolve_config(&IndexMap::new()).unwrap();
        assert!(resolved.is_empty());
    }

    // ── validate_experiment ────────────────────────────────────

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
            }],
            rollbacks: vec![],
            estimate: None,
            baseline: None,
            load: None,
            regulatory: None,
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
            }],
            rollbacks: vec![],
            estimate: None,
            baseline: None,
            load: None,
            regulatory: None,
        };
        assert!(validate_experiment(&exp).is_ok());
    }

    // ── determine_status ───────────────────────────────────────

    #[test]
    fn status_completed_when_all_pass() {
        assert_eq!(
            determine_status(Some(true), Some(true), true),
            ExperimentStatus::Completed
        );
    }

    #[test]
    fn status_deviated_when_after_hypothesis_fails() {
        assert_eq!(
            determine_status(Some(true), Some(false), true),
            ExperimentStatus::Deviated
        );
    }

    #[test]
    fn status_aborted_when_before_hypothesis_fails() {
        assert_eq!(
            determine_status(Some(false), None, true),
            ExperimentStatus::Aborted
        );
    }

    #[test]
    fn status_failed_when_actions_fail() {
        assert_eq!(
            determine_status(Some(true), Some(true), false),
            ExperimentStatus::Failed
        );
    }

    #[test]
    fn status_completed_when_no_hypothesis() {
        assert_eq!(
            determine_status(None, None, true),
            ExperimentStatus::Completed
        );
    }

    // ── resolve_secrets ────────────────────────────────────────

    #[test]
    fn resolve_env_secret() {
        std::env::set_var("TEST_SECRET_TUMULT_PW", "s3cret");
        let secrets = IndexMap::from([(
            "db".into(),
            IndexMap::from([(
                "password".into(),
                SecretValue::Env {
                    key: "TEST_SECRET_TUMULT_PW".into(),
                },
            )]),
        )]);
        let resolved = resolve_secrets(&secrets).unwrap();
        assert_eq!(resolved["db"]["password"], "s3cret");
        std::env::remove_var("TEST_SECRET_TUMULT_PW");
    }

    #[test]
    fn resolve_file_secret() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("token.txt");
        std::fs::write(&path, "my-token-123").unwrap();

        let secrets = IndexMap::from([(
            "api".into(),
            IndexMap::from([("token".into(), SecretValue::File { path: path.clone() })]),
        )]);
        let resolved = resolve_secrets(&secrets).unwrap();
        assert_eq!(resolved["api"]["token"], "my-token-123");
    }

    #[test]
    fn resolve_missing_env_secret_returns_error() {
        std::env::remove_var("NONEXISTENT_SECRET_TUMULT");
        let secrets = IndexMap::from([(
            "db".into(),
            IndexMap::from([(
                "password".into(),
                SecretValue::Env {
                    key: "NONEXISTENT_SECRET_TUMULT".into(),
                },
            )]),
        )]);
        let result = resolve_secrets(&secrets);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("NONEXISTENT_SECRET_TUMULT"));
    }

    #[test]
    fn resolve_missing_file_secret_returns_error() {
        let secrets = IndexMap::from([(
            "db".into(),
            IndexMap::from([(
                "password".into(),
                SecretValue::File {
                    path: "/nonexistent/secret.txt".into(),
                },
            )]),
        )]);
        let result = resolve_secrets(&secrets);
        assert!(result.is_err());
    }

    #[test]
    fn resolve_empty_secrets_succeeds() {
        let resolved = resolve_secrets(&IndexMap::new()).unwrap();
        assert!(resolved.is_empty());
    }

    // ── evaluate_tolerance ─────────────────────────────────────

    #[test]
    fn exact_tolerance_matches_integer() {
        let actual = serde_json::Value::Number(200.into());
        let tolerance = Tolerance::Exact {
            value: serde_json::Value::Number(200.into()),
        };
        assert!(evaluate_tolerance(&actual, &tolerance));
    }

    #[test]
    fn exact_tolerance_rejects_mismatch() {
        let actual = serde_json::Value::Number(500.into());
        let tolerance = Tolerance::Exact {
            value: serde_json::Value::Number(200.into()),
        };
        assert!(!evaluate_tolerance(&actual, &tolerance));
    }

    #[test]
    fn exact_tolerance_matches_string() {
        let actual = serde_json::Value::String("OK".into());
        let tolerance = Tolerance::Exact {
            value: serde_json::Value::String("OK".into()),
        };
        assert!(evaluate_tolerance(&actual, &tolerance));
    }

    #[test]
    fn range_tolerance_accepts_within() {
        let actual = serde_json::json!(50.0);
        let tolerance = Tolerance::Range {
            from: 0.0,
            to: 100.0,
        };
        assert!(evaluate_tolerance(&actual, &tolerance));
    }

    #[test]
    fn range_tolerance_rejects_outside() {
        let actual = serde_json::json!(150.0);
        let tolerance = Tolerance::Range {
            from: 0.0,
            to: 100.0,
        };
        assert!(!evaluate_tolerance(&actual, &tolerance));
    }

    #[test]
    fn regex_tolerance_matches_pattern() {
        let actual = serde_json::Value::String("OK: all systems operational".into());
        let tolerance = Tolerance::Regex {
            pattern: "^OK".into(),
        };
        assert!(evaluate_tolerance(&actual, &tolerance));
    }

    #[test]
    fn regex_tolerance_rejects_non_match() {
        let actual = serde_json::Value::String("ERROR: timeout".into());
        let tolerance = Tolerance::Regex {
            pattern: "^OK".into(),
        };
        assert!(!evaluate_tolerance(&actual, &tolerance));
    }

    #[test]
    fn regex_tolerance_returns_false_for_non_string() {
        let actual = serde_json::json!(42);
        let tolerance = Tolerance::Regex {
            pattern: ".*".into(),
        };
        assert!(!evaluate_tolerance(&actual, &tolerance));
    }

    // ── parse_experiment ───────────────────────────────────────

    #[test]
    fn parse_invalid_toon_returns_error() {
        let result = parse_experiment("not valid toon {{{");
        assert!(result.is_err());
    }
}
