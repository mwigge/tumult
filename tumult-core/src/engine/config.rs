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

/// Flatten resolved secrets (`group -> key -> value`) into `"group.key" ->
/// value` pairs. This is the shape template substitution consumes:
/// `${secrets.<group>.<key>}` looks up `"group.key"` in the flattened map.
#[must_use]
pub fn flatten_secrets<S: ::std::hash::BuildHasher, S1: ::std::hash::BuildHasher>(
    secrets: &HashMap<String, HashMap<String, String, S1>, S>,
) -> HashMap<String, String> {
    let mut flat = HashMap::new();
    for (group, group_secrets) in secrets {
        for (key, value) in group_secrets {
            flat.insert(format!("{group}.{key}"), value.clone());
        }
    }
    flat
}

/// Build the `TUMULT_CONFIG_<NAME>` environment pairs that are injected into
/// `process` and `script` provider subprocesses from resolved configuration
/// values.
///
/// Returns the env map plus the keys that were skipped. A key is skipped when
/// its uppercased form is not a valid shell identifier
/// (`[A-Za-z_][A-Za-z0-9_]*` — the same rule the plugin executor enforces for
/// script arguments) or when it collides with another key after uppercasing;
/// skipping loudly beats silently exporting a mangled name.
#[must_use]
pub fn build_config_env<S: ::std::hash::BuildHasher>(
    config: &HashMap<String, String, S>,
) -> (HashMap<String, String>, Vec<String>) {
    build_prefixed_env(config, "TUMULT_CONFIG_", None)
}

/// Build the `TUMULT_SECRET_<GROUP>_<KEY>` environment pairs injected into
/// `process` and `script` provider subprocesses from resolved (flattened)
/// secret values. The `.` separator in a flattened `"group.key"` name becomes
/// `_` in the env var name.
///
/// Skipping rules are identical to [`build_config_env`].
#[must_use]
pub fn build_secret_env<S: ::std::hash::BuildHasher>(
    secrets_flat: &HashMap<String, String, S>,
) -> (HashMap<String, String>, Vec<String>) {
    build_prefixed_env(secrets_flat, "TUMULT_SECRET_", Some('.'))
}

/// Shared implementation for [`build_config_env`] / [`build_secret_env`]:
/// uppercase each key (with `separator` mapped to `_`), prefix it, and keep
/// only names that form valid shell identifiers without colliding.
/// Iteration is sorted so collision winners are deterministic.
fn build_prefixed_env<S: ::std::hash::BuildHasher>(
    values: &HashMap<String, String, S>,
    prefix: &str,
    separator: Option<char>,
) -> (HashMap<String, String>, Vec<String>) {
    let mut env = HashMap::with_capacity(values.len());
    let mut skipped = Vec::new();
    let mut keys: Vec<&String> = values.keys().collect();
    keys.sort_unstable();
    for key in keys {
        let mut name = key.to_uppercase();
        if let Some(sep) = separator {
            name = name.replace(sep, "_");
        }
        if !is_valid_env_identifier(&name) {
            skipped.push(key.clone());
            continue;
        }
        let env_name = format!("{prefix}{name}");
        if env.contains_key(&env_name) {
            // Two keys uppercased to the same env var (e.g. `foo` vs `FOO`).
            skipped.push(key.clone());
            continue;
        }
        env.insert(env_name, values[key].clone());
    }
    (env, skipped)
}

/// Valid POSIX environment variable name: `[A-Za-z_][A-Za-z0-9_]*`.
fn is_valid_env_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
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

    // ── flatten_secrets ────────────────────────────────────────

    #[test]
    fn flatten_secrets_joins_group_and_key_with_dot() {
        let secrets = HashMap::from([(
            "db".to_string(),
            HashMap::from([
                ("password".to_string(), "s3cret".to_string()),
                ("user".to_string(), "chaos".to_string()),
            ]),
        )]);
        let flat = flatten_secrets(&secrets);
        assert_eq!(flat.get("db.password").unwrap(), "s3cret");
        assert_eq!(flat.get("db.user").unwrap(), "chaos");
        assert_eq!(flat.len(), 2);
    }

    // ── build_config_env / build_secret_env ────────────────────

    #[test]
    fn config_env_uppercases_and_prefixes_keys() {
        let config = HashMap::from([
            ("db_host".to_string(), "db.internal".to_string()),
            ("retries".to_string(), "3".to_string()),
        ]);
        let (env, skipped) = build_config_env(&config);
        assert!(skipped.is_empty());
        assert_eq!(env.get("TUMULT_CONFIG_DB_HOST").unwrap(), "db.internal");
        assert_eq!(env.get("TUMULT_CONFIG_RETRIES").unwrap(), "3");
    }

    #[test]
    fn secret_env_flattens_group_dot_key_to_underscore() {
        let flat = HashMap::from([("db.password".to_string(), "s3cret".to_string())]);
        let (env, skipped) = build_secret_env(&flat);
        assert!(skipped.is_empty());
        assert_eq!(env.get("TUMULT_SECRET_DB_PASSWORD").unwrap(), "s3cret");
    }

    #[test]
    fn invalid_identifier_keys_are_skipped_not_mangled() {
        let config = HashMap::from([
            ("db-host".to_string(), "x".to_string()),
            ("good_key".to_string(), "y".to_string()),
        ]);
        let (env, skipped) = build_config_env(&config);
        assert_eq!(skipped, vec!["db-host".to_string()]);
        assert_eq!(env.len(), 1);
        assert!(env.contains_key("TUMULT_CONFIG_GOOD_KEY"));
    }

    #[test]
    fn case_colliding_keys_skip_the_later_one_deterministically() {
        let config = HashMap::from([
            ("foo".to_string(), "lower".to_string()),
            ("FOO".to_string(), "upper".to_string()),
        ]);
        let (env, skipped) = build_config_env(&config);
        // Sorted iteration: "FOO" < "foo", so the uppercase key wins and the
        // lowercase one is reported as skipped.
        assert_eq!(skipped, vec!["foo".to_string()]);
        assert_eq!(env.get("TUMULT_CONFIG_FOO").unwrap(), "upper");
    }
}
