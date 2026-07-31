//! Tumult Exec — the provider-dispatch activity executor shared by the
//! tumult CLI and `tumultd`.
//!
//! [`ProviderExecutor`] implements [`tumult_core::runner::ActivityExecutor`]
//! by dispatching each activity to its declared provider: external processes
//! (`process`), filesystem-discovered script plugins (`script`), and
//! in-process Rust plugins (`native`). The native composition root built by
//! [`native_registry`] registers every native provider crate, so both the CLI
//! and the daemon execute experiments with the identical plugin set.

use std::collections::HashMap;

use tumult_core::runner::{ActivityExecutor, ActivityOutcome};
use tumult_core::types::{Activity, Provider};

mod native;
mod process;
mod script;

use native::execute_native;
use process::execute_process;
use script::execute_script_provider;

pub use native::registry as native_registry;

// ── Provider-based executor ───────────────────────────────────

/// Executes activities by dispatching to the appropriate provider.
///
/// Supports Process, Script, and Native (Rust) providers. Script plugins
/// resolve through the filesystem discovery search paths and run via
/// `tumult_plugin::executor::execute_script`; native plugins dispatch
/// through the `tumult_plugin::native::NativeExecutorRegistry` composition
/// root in the native executor, via async execution on the Tokio runtime.
///
/// `injected_env` carries the experiment's resolved `configuration:` and
/// `secrets:` values as pre-built `TUMULT_CONFIG_*` / `TUMULT_SECRET_*`
/// pairs (see [`tumult_core::engine::build_config_env`]). They reach
/// `process` activities as environment variables and `script` activities as
/// extra arguments (which the script executor exports as the same env
/// vars). Entries declared on the activity itself always win over injected
/// ones; `native` providers receive no injection.
pub struct ProviderExecutor {
    injected_env: HashMap<String, String>,
}

impl ProviderExecutor {
    /// Executor with no configuration/secret injection (tests).
    #[must_use]
    pub fn new() -> Self {
        Self {
            injected_env: HashMap::new(),
        }
    }

    /// Executor injecting the given `TUMULT_CONFIG_*` / `TUMULT_SECRET_*`
    /// pairs into process and script provider subprocesses.
    #[must_use]
    pub fn with_injected_env(injected_env: HashMap<String, String>) -> Self {
        Self { injected_env }
    }

    /// Merge the injected env into a process activity's declared environment;
    /// declared entries win.
    fn merged_process_env(&self, env: &HashMap<String, String>) -> HashMap<String, String> {
        let mut merged = self.injected_env.clone();
        merged.extend(env.iter().map(|(k, v)| (k.clone(), v.clone())));
        merged
    }

    /// Merge the injected env into a script activity's declared arguments as
    /// `config_*` / `secret_*` argument keys (exported by the script executor
    /// as the same `TUMULT_*` env vars); declared arguments win.
    fn merged_script_arguments(
        &self,
        arguments: &HashMap<String, serde_json::Value>,
    ) -> HashMap<String, serde_json::Value> {
        let mut merged: HashMap<String, serde_json::Value> = self
            .injected_env
            .iter()
            .filter_map(|(k, v)| {
                k.strip_prefix("TUMULT_")
                    .map(|rest| (rest.to_lowercase(), serde_json::Value::String(v.clone())))
            })
            .collect();
        merged.extend(arguments.iter().map(|(k, v)| (k.clone(), v.clone())));
        merged
    }
}

impl Default for ProviderExecutor {
    fn default() -> Self {
        Self::new()
    }
}

/// Debug prints only the injected variable NAMES (sorted), never values —
/// the map carries resolved secrets.
impl std::fmt::Debug for ProviderExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut names: Vec<&str> = self.injected_env.keys().map(String::as_str).collect();
        names.sort_unstable();
        f.debug_struct("ProviderExecutor")
            .field("injected_env_names", &names)
            .finish()
    }
}

impl ActivityExecutor for ProviderExecutor {
    fn execute(&self, activity: &Activity) -> ActivityOutcome {
        match &activity.provider {
            Provider::Process {
                path,
                arguments,
                env,
                timeout_s,
            } => execute_process(
                path,
                arguments,
                &self.merged_process_env(env),
                timeout_s.as_ref(),
            ),
            Provider::Script {
                plugin,
                function,
                arguments,
                timeout_s,
            } => execute_script_provider(
                plugin,
                function,
                &self.merged_script_arguments(arguments),
                timeout_s.as_ref(),
            ),
            Provider::Native {
                plugin,
                function,
                arguments,
            } => execute_native(plugin, function, arguments),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn injected() -> HashMap<String, String> {
        HashMap::from([
            (
                "TUMULT_CONFIG_DB_HOST".to_string(),
                "db.internal".to_string(),
            ),
            ("TUMULT_SECRET_API_TOKEN".to_string(), "s3cret".to_string()),
        ])
    }

    #[test]
    fn process_env_merges_injected_with_declared_winning() {
        let executor = ProviderExecutor::with_injected_env(injected());
        let declared =
            HashMap::from([("TUMULT_CONFIG_DB_HOST".to_string(), "declared".to_string())]);
        let merged = executor.merged_process_env(&declared);
        assert_eq!(merged.get("TUMULT_CONFIG_DB_HOST").unwrap(), "declared");
        assert_eq!(merged.get("TUMULT_SECRET_API_TOKEN").unwrap(), "s3cret");
    }

    #[test]
    fn script_arguments_receive_injected_env_as_lowercase_keys() {
        let executor = ProviderExecutor::with_injected_env(injected());
        let merged = executor.merged_script_arguments(&HashMap::new());
        // The script executor prefixes TUMULT_ and uppercases, so these keys
        // round-trip back to the exact injected env var names.
        assert_eq!(
            merged.get("config_db_host").unwrap(),
            &serde_json::Value::String("db.internal".to_string())
        );
        assert_eq!(
            merged.get("secret_api_token").unwrap(),
            &serde_json::Value::String("s3cret".to_string())
        );
    }

    #[test]
    fn script_declared_arguments_win_over_injected() {
        let executor = ProviderExecutor::with_injected_env(injected());
        let declared = HashMap::from([(
            "config_db_host".to_string(),
            serde_json::Value::String("declared".to_string()),
        )]);
        let merged = executor.merged_script_arguments(&declared);
        assert_eq!(
            merged.get("config_db_host").unwrap(),
            &serde_json::Value::String("declared".to_string())
        );
    }

    #[test]
    fn debug_output_names_keys_but_never_values() {
        let executor = ProviderExecutor::with_injected_env(injected());
        let debug = format!("{executor:?}");
        assert!(debug.contains("TUMULT_SECRET_API_TOKEN"));
        assert!(!debug.contains("s3cret"), "secret value leaked: {debug}");
        assert!(
            !debug.contains("db.internal"),
            "config value leaked: {debug}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn injected_env_reaches_process_subprocess() {
        let executor = ProviderExecutor::with_injected_env(injected());
        let activity = Activity {
            name: "injection-test".into(),
            activity_type: tumult_core::types::ActivityType::Action,
            provider: Provider::Process {
                path: "sh".into(),
                arguments: vec!["-c".into(), "echo \"$TUMULT_CONFIG_DB_HOST\"".into()],
                env: HashMap::new(),
                timeout_s: Some(5.0),
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
            label_selector: None,
        };
        let outcome = executor.execute(&activity);
        assert!(outcome.success, "{:?}", outcome.error);
        assert_eq!(outcome.output.as_deref(), Some("db.internal"));
    }
}
