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
/// # Errors
///
/// Returns [`EngineError::UndefinedVar`] if the experiment contains a `${key}`
/// placeholder for a variable that is not present in `vars`.
/// Returns [`EngineError::ParseError`] if the substituted document cannot be
/// decoded back into an `Experiment`.
pub fn apply_vars<S: ::std::hash::BuildHasher>(
    experiment: &Experiment,
    vars: &HashMap<String, String, S>,
) -> Result<Experiment, EngineError> {
    // Serialize to TOON then do string substitution so every nested string
    // field is covered in one pass.
    let toon = toon_format::encode_default(experiment)
        .map_err(|e| EngineError::ParseError(e.to_string()))?;
    let substituted = substitute_vars(&toon, vars)?;
    toon_format::decode_default(&substituted).map_err(|e| EngineError::ParseError(e.to_string()))
}

/// Substitute `${key}` placeholders in `text` using the provided `vars` map.
///
/// # Errors
///
/// Returns [`EngineError::UndefinedVar`] for any placeholder whose key is not
/// present in `vars`.
fn substitute_vars<S: ::std::hash::BuildHasher>(
    text: &str,
    vars: &HashMap<String, String, S>,
) -> Result<String, EngineError> {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '$' && chars.peek() == Some(&'{') {
            // Consume '{'
            chars.next();
            let mut name = String::new();
            for inner in chars.by_ref() {
                if inner == '}' {
                    break;
                }
                name.push(inner);
            }
            let value = vars
                .get(&name)
                .ok_or_else(|| EngineError::UndefinedVar { name: name.clone() })?;
            // Escape the value for TOON string context (quotes, backslashes,
            // newlines) so it can't break parsing or inject structure into
            // the surrounding document.
            result.push_str(&toon_format::escape_string(value));
        } else {
            result.push(ch);
        }
    }
    Ok(result)
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
}
