//! Immutable target bindings for restart cleanup. Secret values are resolved
//! at execution/recovery time; only their original references are persisted.
use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tumult_core::engine::{parse_experiment, resolve_config};
use tumult_core::types::Experiment;

use super::queue::prepare_definition;

type PreparedRun = (Experiment, HashMap<String, String>);

#[derive(Serialize, Deserialize)]
pub(super) struct RecoveryPlan {
    definition: Experiment,
    vars: HashMap<String, String>,
    config_hash: String,
}

impl RecoveryPlan {
    pub(super) fn capture(toon: &str, vars: &HashMap<String, String>) -> Result<Self, String> {
        let definition = parse_experiment(toon).map_err(|_| preparation_error("parse"))?;
        let resolved =
            resolve_config(&definition.configuration).map_err(|_| preparation_error("config"))?;
        Ok(Self {
            definition,
            vars: vars.clone(),
            config_hash: configuration_hash(&resolved)?,
        })
    }

    pub(super) fn prepare(&self) -> Result<PreparedRun, String> {
        let resolved = resolve_config(&self.definition.configuration)
            .map_err(|_| preparation_error("config"))?;
        if configuration_hash(&resolved)? != self.config_hash {
            return Err(
                "configuration changed since execution; restore original bindings before cleanup"
                    .into(),
            );
        }
        prepare_definition(&self.definition, &self.vars, &resolved).map_err(|diagnostic| {
            // Resolved validation/parser diagnostics can quote credential values.
            // These errors are persisted to public run state and audit records.
            let stage = ["parse", "config", "secrets", "template", "validate"]
                .into_iter()
                .find(|stage| diagnostic.starts_with(&format!("{stage}:")))
                .unwrap_or("prepare");
            preparation_error(stage)
        })
    }
}

fn preparation_error(stage: &str) -> String {
    format!("{stage}: execution inputs could not be prepared; check the definition and configured sources")
}

fn configuration_hash(config: &HashMap<String, String>) -> Result<String, String> {
    let sorted: std::collections::BTreeMap<_, _> = config.iter().collect();
    let bytes = serde_json::to_vec(&sorted).map_err(|e| e.to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotated_secret_validation_error_is_safe_to_persist() {
        let secret = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(secret.path(), "valid_plugin").unwrap();
        let toon = format!(
            "title: safe failure\nsecrets:\n  service:\n    token:\n      type: file\n      path: {}\nmethod[1]:\n  - name: resolved plugin\n    activity_type: action\n    provider:\n      type: script\n      plugin: ${{secrets.service.token}}\n      function: run\n",
            secret.path().display()
        );
        let plan = RecoveryPlan::capture(&toon, &HashMap::new()).unwrap();
        plan.prepare().unwrap();
        let credential = "synthetic rotated secret with whitespace";
        std::fs::write(secret.path(), credential).unwrap();
        let error = plan.prepare().unwrap_err();
        assert!(
            !error.contains(credential),
            "persistable error leaked credential: {error}"
        );
        assert!(
            error.starts_with("validate:"),
            "safe stage is retained: {error}"
        );
    }

    #[test]
    fn snapshot_preserves_bindings_and_secret_references() {
        let secret = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(secret.path(), "old-credential").unwrap();
        let toon = format!(
            r#"
title: recovery binding
configuration:
  namespace:
    type: inline
    value: original
secrets:
  service:
    token:
      type: file
      path: {}
method[1]:
  - name: inject
    activity_type: action
    provider:
      type: process
      path: echo
      arguments[1]: ${{target}}
rollbacks[1]:
  - name: cleanup
    activity_type: action
    provider:
      type: process
      path: echo
      arguments[3]: ${{target}},${{config.namespace}},${{secrets.service.token}}
"#,
            secret.path().display()
        );
        let plan = RecoveryPlan::capture(
            &toon,
            &HashMap::from([("target".into(), "original-target".into())]),
        )
        .unwrap();
        let encoded = serde_json::to_string(&plan).unwrap();
        assert!(!encoded.contains("old-credential"));
        std::fs::write(secret.path(), "rotated-credential").unwrap();
        let restored: RecoveryPlan = serde_json::from_str(&encoded).unwrap();
        let (experiment, env) = restored.prepare().unwrap();
        let tumult_core::types::Provider::Process { arguments, .. } =
            &experiment.rollbacks[0].provider
        else {
            panic!("process rollback")
        };
        assert_eq!(
            arguments,
            &["original-target", "original", "rotated-credential"]
        );
        assert_eq!(env["TUMULT_SECRET_SERVICE_TOKEN"], "rotated-credential");
    }
    #[test]
    fn environment_configuration_is_hashed_and_drift_refuses_cleanup() {
        const KEY: &str = "TUMULT_TEST_RECOVERY_CONFIG_SECRET";
        std::env::set_var(KEY, "original-sensitive-value");
        let toon = format!("title: binding\nconfiguration:\n  target:\n    type: env\n    key: {KEY}\nmethod[1]:\n  - name: action\n    activity_type: action\n    provider:\n      type: process\n      path: echo\n      arguments[1]: ${{config.target}}\n");
        let plan = RecoveryPlan::capture(&toon, &HashMap::new()).unwrap();
        assert!(!serde_json::to_string(&plan)
            .unwrap()
            .contains("original-sensitive-value"));
        std::env::set_var(KEY, "changed-destination");
        assert!(plan
            .prepare()
            .unwrap_err()
            .contains("configuration changed"));
        std::env::set_var(KEY, "original-sensitive-value");
        assert!(plan.prepare().is_ok());
        std::env::remove_var(KEY);
    }
}
