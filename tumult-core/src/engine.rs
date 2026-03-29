//! Experiment engine — orchestrates the five-phase execution lifecycle.

use std::collections::HashMap;

use crate::types::{ConfigValue, Experiment, ExperimentStatus};

use thiserror::Error;

#[derive(Error, Debug)]
pub enum EngineError {
    #[error("experiment has no method steps")]
    EmptyMethod,
    #[error("configuration key '{key}' references env var '{env_key}' which is not set")]
    ConfigResolutionFailed { key: String, env_key: String },
    #[error("experiment file parse error: {0}")]
    ParseError(String),
}

/// Resolve configuration values by reading environment variables.
pub fn resolve_config(
    config: &HashMap<String, ConfigValue>,
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
pub fn validate_experiment(experiment: &Experiment) -> Result<(), EngineError> {
    if experiment.method.is_empty() {
        return Err(EngineError::EmptyMethod);
    }
    Ok(())
}

/// Parse an experiment from a TOON string.
pub fn parse_experiment(toon: &str) -> Result<Experiment, EngineError> {
    toon_format::decode_default(toon).map_err(|e| EngineError::ParseError(e.to_string()))
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
    use std::collections::HashMap;

    // ── resolve_config ─────────────────────────────────────────

    #[test]
    fn resolve_inline_config() {
        let config = HashMap::from([(
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
        let config = HashMap::from([(
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
        let config = HashMap::from([(
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
        let resolved = resolve_config(&HashMap::new()).unwrap();
        assert!(resolved.is_empty());
    }

    // ── validate_experiment ────────────────────────────────────

    #[test]
    fn validate_rejects_empty_method() {
        let exp = Experiment {
            title: "empty".into(),
            description: None,
            tags: vec![],
            configuration: HashMap::new(),
            secrets: HashMap::new(),
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
    fn validate_accepts_experiment_with_method() {
        let exp = Experiment {
            title: "valid".into(),
            description: None,
            tags: vec![],
            configuration: HashMap::new(),
            secrets: HashMap::new(),
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

    // ── parse_experiment ───────────────────────────────────────

    #[test]
    fn parse_invalid_toon_returns_error() {
        let result = parse_experiment("not valid toon {{{");
        assert!(result.is_err());
    }
}
