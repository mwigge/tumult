//! Experiment parsing and template variable substitution.

use std::collections::HashMap;

use crate::types::Experiment;

use super::EngineError;

/// Parse an experiment from a TOON string.
///
/// # Errors
///
/// Returns [`EngineError::ParseError`] if the TOON string is malformed or cannot be decoded.
pub fn parse_experiment(toon: &str) -> Result<Experiment, EngineError> {
    toon_format::decode_default(toon).map_err(|e| EngineError::ParseError(e.to_string()))
}

/// Apply template variable substitution to an experiment.
///
/// Replaces all `${key}` occurrences in every string field of the experiment
/// with the corresponding value from `vars`.  The substitution is performed
/// on the serialized TOON representation so that all nested string values are
/// covered without visiting individual fields.
///
/// A `$${key}` sequence is an escape hatch: it renders as a literal `${key}`
/// with no substitution, so shell-style text (e.g. `${HOME}` inside a
/// `sh -c` argument) survives templating untouched.
///
/// # Errors
///
/// Returns [`EngineError::UndefinedVars`] naming every `${key}` placeholder
/// (deduplicated, in order of first appearance) that is not present in `vars`.
/// Returns [`EngineError::ParseError`] if the substituted document cannot be
/// decoded back into an `Experiment`.
pub fn apply_vars<S: ::std::hash::BuildHasher>(
    experiment: &Experiment,
    vars: &HashMap<String, String, S>,
) -> Result<Experiment, EngineError> {
    apply_template_vars(experiment, vars, &HashMap::new(), &HashMap::new())
}

/// Like [`apply_vars`], additionally resolving namespaced placeholders from
/// the experiment's resolved `configuration:` and `secrets:` sections:
///
/// * `${config.<name>}` resolves from `config` (the resolved configuration
///   values keyed by configuration name),
/// * `${secrets.<group>.<key>}` resolves from `secrets`, flattened as
///   `"group.key" -> value` (see
///   [`crate::engine::flatten_secrets`]).
///
/// `vars` (`--var` entries) take precedence: a `vars` key that exactly
/// matches the placeholder name (e.g. `config.env`) wins over the resolved
/// configuration or secret value.
///
/// # Errors
///
/// Same as [`apply_vars`].
pub fn apply_template_vars<S, S1, S2>(
    experiment: &Experiment,
    vars: &HashMap<String, String, S>,
    config: &HashMap<String, String, S1>,
    secrets: &HashMap<String, String, S2>,
) -> Result<Experiment, EngineError>
where
    S: ::std::hash::BuildHasher,
    S1: ::std::hash::BuildHasher,
    S2: ::std::hash::BuildHasher,
{
    // Serialize to TOON then do string substitution so every nested string
    // field is covered in one pass.
    let toon = toon_format::encode_default(experiment)
        .map_err(|e| EngineError::ParseError(e.to_string()))?;
    let substituted = substitute_vars(&toon, vars, config, secrets)?;
    toon_format::decode_default(&substituted).map_err(|e| EngineError::ParseError(e.to_string()))
}

/// Substitute `${key}` placeholders in `text` using `vars` plus the
/// namespaced `config`/`secrets` maps. `$${key}` renders as a literal
/// `${key}`.
///
/// # Errors
///
/// Returns [`EngineError::UndefinedVars`] listing every placeholder whose key
/// is present in none of the maps.
fn substitute_vars<S, S1, S2>(
    text: &str,
    vars: &HashMap<String, String, S>,
    config: &HashMap<String, String, S1>,
    secrets: &HashMap<String, String, S2>,
) -> Result<String, EngineError>
where
    S: ::std::hash::BuildHasher,
    S1: ::std::hash::BuildHasher,
    S2: ::std::hash::BuildHasher,
{
    let mut result = String::with_capacity(text.len());
    let mut missing: Vec<String> = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '$' {
            result.push(ch);
            continue;
        }
        if chars.peek() == Some(&'$') {
            // Consume the second '$'. `$${name}` is the escape hatch: copy
            // `${name}` through verbatim, with no substitution and no
            // missing-variable error.
            chars.next();
            if chars.peek() == Some(&'{') {
                result.push_str("${");
                chars.next();
                for inner in chars.by_ref() {
                    result.push(inner);
                    if inner == '}' {
                        break;
                    }
                }
            } else {
                result.push_str("$$");
            }
            continue;
        }
        if chars.peek() == Some(&'{') {
            // Consume '{'
            chars.next();
            let mut name = String::new();
            for inner in chars.by_ref() {
                if inner == '}' {
                    break;
                }
                name.push(inner);
            }
            match lookup_var(&name, vars, config, secrets) {
                Some(value) => {
                    // Escape the value for TOON string context (quotes,
                    // backslashes, newlines) so it can't break parsing or
                    // inject structure into the surrounding document.
                    result.push_str(&toon_format::escape_string(value));
                }
                None => {
                    if !missing.contains(&name) {
                        missing.push(name);
                    }
                }
            }
        } else {
            result.push(ch);
        }
    }
    if missing.is_empty() {
        Ok(result)
    } else {
        let names = missing
            .iter()
            .map(|n| format!("${{{n}}}"))
            .collect::<Vec<_>>()
            .join(", ");
        Err(EngineError::UndefinedVars { names })
    }
}

/// Resolve a placeholder name: bare names come from `vars` (`--var`);
/// `config.<name>` and `secrets.<group>.<key>` come from the namespaced maps.
/// `vars` wins even for namespaced names, so a `--var` can override a
/// resolved configuration or secret value.
fn lookup_var<'a, S, S1, S2>(
    name: &str,
    vars: &'a HashMap<String, String, S>,
    config: &'a HashMap<String, String, S1>,
    secrets: &'a HashMap<String, String, S2>,
) -> Option<&'a String>
where
    S: ::std::hash::BuildHasher,
    S1: ::std::hash::BuildHasher,
    S2: ::std::hash::BuildHasher,
{
    if let Some(value) = vars.get(name) {
        return Some(value);
    }
    if let Some(key) = name.strip_prefix("config.") {
        return config.get(key);
    }
    if let Some(key) = name.strip_prefix("secrets.") {
        return secrets.get(key);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;
    use std::collections::HashMap;

    // ── parse_experiment ───────────────────────────────────────

    #[test]
    fn parse_invalid_toon_returns_error() {
        let result = parse_experiment("not valid toon {{{");
        assert!(result.is_err());
    }

    // ── apply_vars ────────────────────────────────────────────

    fn template_experiment(title: &str) -> Experiment {
        Experiment {
            version: "v1".into(),
            title: title.into(),
            method: vec![Activity {
                name: "action".into(),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn apply_vars_substitutes_title() {
        let exp = template_experiment("Deploy ${env} canary");
        let vars = HashMap::from([("env".into(), "production".into())]);
        let result = apply_vars(&exp, &vars).unwrap();
        assert_eq!(result.title, "Deploy production canary");
    }

    #[test]
    fn apply_vars_substitutes_activity_name() {
        let exp = Experiment {
            version: "v1".into(),
            title: "test".into(),
            method: vec![Activity {
                name: "kill-pod-${namespace}".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let vars = HashMap::from([("namespace".into(), "payments".into())]);
        let result = apply_vars(&exp, &vars).unwrap();
        assert_eq!(result.method[0].name, "kill-pod-payments");
    }

    #[test]
    fn apply_vars_multiple_substitutions() {
        let exp = template_experiment("${cluster} ${env} experiment");
        let vars = HashMap::from([
            ("cluster".into(), "eu-west-1".into()),
            ("env".into(), "staging".into()),
        ]);
        let result = apply_vars(&exp, &vars).unwrap();
        assert_eq!(result.title, "eu-west-1 staging experiment");
    }

    #[test]
    fn apply_vars_empty_vars_passes_through() {
        let exp = template_experiment("no variables here");
        let result = apply_vars(&exp, &HashMap::new()).unwrap();
        assert_eq!(result.title, "no variables here");
    }

    #[test]
    fn apply_vars_undefined_var_returns_error() {
        let exp = template_experiment("${undefined_key} title");
        let err = apply_vars(&exp, &HashMap::new()).unwrap_err();
        assert!(
            err.to_string().contains("undefined_key"),
            "error should name the undefined variable; got: {err}"
        );
    }

    #[test]
    fn apply_vars_repeated_same_var() {
        let exp = template_experiment("${env}-${env}");
        let vars = HashMap::from([("env".into(), "prod".into())]);
        let result = apply_vars(&exp, &vars).unwrap();
        assert_eq!(result.title, "prod-prod");
    }

    #[test]
    fn apply_vars_escapes_quotes_and_newlines() {
        let exp = template_experiment("Deploy ${env} canary");
        let vars = HashMap::from([("env".into(), "prod\"; injected: true\nmore".into())]);
        let result = apply_vars(&exp, &vars).unwrap();
        assert_eq!(result.title, "Deploy prod\"; injected: true\nmore canary");
        // Only the title was substituted; no extra fields were injected.
        assert!(result.description.is_none());
        assert_eq!(result.method.len(), 1);
    }

    // ── escape hatch ($${...}) ────────────────────────────────

    #[test]
    fn escaped_placeholder_renders_literal() {
        let exp = template_experiment("home is $${HOME}");
        let result = apply_vars(&exp, &HashMap::new()).unwrap();
        assert_eq!(result.title, "home is ${HOME}");
    }

    #[test]
    fn escaped_placeholder_needs_no_variable_even_when_vars_present() {
        let exp = template_experiment("run $${HOME} on ${env}");
        let vars = HashMap::from([("env".into(), "prod".into())]);
        let result = apply_vars(&exp, &vars).unwrap();
        assert_eq!(result.title, "run ${HOME} on prod");
    }

    #[test]
    fn double_dollar_without_brace_passes_through() {
        let exp = template_experiment("costs $$5 today");
        let result = apply_vars(&exp, &HashMap::new()).unwrap();
        assert_eq!(result.title, "costs $$5 today");
    }

    // ── missing-variable aggregation ──────────────────────────

    #[test]
    fn missing_vars_error_lists_all_missing_names() {
        let mut exp = template_experiment("${alpha} and ${beta}");
        exp.method[0].name = "step-${gamma}".into();
        let err = apply_vars(&exp, &HashMap::new()).unwrap_err();
        let msg = err.to_string();
        for name in ["${alpha}", "${beta}", "${gamma}"] {
            assert!(msg.contains(name), "error should name {name}; got: {msg}");
        }
    }

    #[test]
    fn missing_vars_error_deduplicates_repeated_names() {
        let exp = template_experiment("${env}-${env}");
        let err = apply_vars(&exp, &HashMap::new()).unwrap_err();
        let msg = err.to_string();
        assert_eq!(msg.matches("${env}").count(), 1, "got: {msg}");
    }

    // ── config/secrets namespaces ─────────────────────────────

    #[test]
    fn template_vars_resolve_config_and_secrets_namespaces() {
        let exp = template_experiment("db=${config.db_host} pw=${secrets.db.password}");
        let config = HashMap::from([("db_host".into(), "db.internal".into())]);
        let secrets = HashMap::from([("db.password".into(), "s3cret".into())]);
        let result = apply_template_vars(&exp, &HashMap::new(), &config, &secrets).unwrap();
        assert_eq!(result.title, "db=db.internal pw=s3cret");
    }

    #[test]
    fn vars_take_precedence_over_config_and_secrets() {
        let exp = template_experiment("${config.env} ${secrets.db.password} ${plain}");
        let vars = HashMap::from([
            ("config.env".into(), "override".into()),
            ("plain".into(), "value".into()),
        ]);
        let config = HashMap::from([("env".into(), "resolved".into())]);
        let secrets = HashMap::from([("db.password".into(), "s3cret".into())]);
        let result = apply_template_vars(&exp, &vars, &config, &secrets).unwrap();
        assert_eq!(result.title, "override s3cret value");
    }

    #[test]
    fn missing_config_placeholder_is_named_with_its_namespace() {
        let exp = template_experiment("${config.nope}");
        let err = apply_template_vars(&exp, &HashMap::new(), &HashMap::new(), &HashMap::new())
            .unwrap_err();
        assert!(
            err.to_string().contains("${config.nope}"),
            "error should keep the namespaced name; got: {err}"
        );
    }
}
