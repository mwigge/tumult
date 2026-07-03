//! Resolution of configuration values and secrets from the environment and files.

use std::collections::HashMap;

use indexmap::IndexMap;

use crate::types::{ConfigValue, SecretValue};

use super::EngineError;

/// Resolve configuration values by reading environment variables.
///
/// # Errors
///
/// Returns [`EngineError::ConfigResolutionFailed`] if a required environment variable is not set.
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

/// Resolve secret values by reading environment variables or files.
///
/// # Errors
///
/// Returns [`EngineError::SecretResolutionFailed`] if a required environment variable is not set.
/// Returns [`EngineError::SecretFileNotFound`] if a secret file does not exist or cannot be read.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;
    use indexmap::IndexMap;

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
}
