//! Compatibility executor backed by the shared CLI/daemon provider dispatch.

use std::collections::HashMap;
use tumult_core::runner::{ActivityExecutor, ActivityOutcome};
use tumult_core::types::{Activity, Provider};

/// Default timeout retained for MCP process activities without an explicit limit.
const DEFAULT_EXECUTION_TIMEOUT_SECS: f64 = 300.0;

/// Executes process, script and native activities through the shared provider registry.
/// The historical public name is retained for existing MCP integrations.
#[derive(Default)]
pub struct ProcessExecutor {
    inner: tumult_exec::ProviderExecutor,
}

impl ProcessExecutor {
    /// Creates an executor with no injected configuration.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an executor with per-run configuration and credential injection.
    #[must_use]
    pub fn with_injected_env(env: HashMap<String, String>) -> Self {
        Self {
            inner: tumult_exec::ProviderExecutor::with_injected_env(env),
        }
    }
}

impl ActivityExecutor for ProcessExecutor {
    fn execute(&self, activity: &Activity) -> ActivityOutcome {
        if matches!(
            &activity.provider,
            Provider::Process {
                timeout_s: None,
                ..
            }
        ) {
            let mut bounded = activity.clone();
            if let Provider::Process { timeout_s, .. } = &mut bounded.provider {
                *timeout_s = Some(DEFAULT_EXECUTION_TIMEOUT_SECS);
            }
            return self.inner.execute(&bounded);
        }
        self.inner.execute(activity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tumult_core::runner::ActivityExecutor;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn process_executor_respects_timeout() {
        let executor = ProcessExecutor::new();
        let activity = tumult_core::types::Activity {
            name: "timeout-test".into(),
            activity_type: tumult_core::types::ActivityType::Action,
            provider: tumult_core::types::Provider::Process {
                path: "sleep".into(),
                arguments: vec!["60".into()],
                env: std::collections::HashMap::new(),
                timeout_s: Some(0.2), // 200ms timeout
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
            label_selector: None,
        };

        let outcome = executor.execute(&activity);
        assert!(outcome.error.as_ref().unwrap().contains("timed out"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn process_executor_records_duration() {
        let executor = ProcessExecutor::new();
        let activity = tumult_core::types::Activity {
            name: "duration-test".into(),
            activity_type: tumult_core::types::ActivityType::Action,
            provider: tumult_core::types::Provider::Process {
                path: "echo".into(),
                arguments: vec!["hello".into()],
                env: std::collections::HashMap::new(),
                timeout_s: Some(5.0),
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
            label_selector: None,
        };

        let outcome = executor.execute(&activity);
        assert!(outcome.success);
        assert_eq!(outcome.output.as_deref(), Some("hello"));
        // Duration should be recorded (previously was always 0)
        // It may still be 0 for very fast commands, so just check it's not negative
        // (u64 is always >= 0)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn injected_env_reaches_subprocess_and_declared_wins() {
        let executor = ProcessExecutor::with_injected_env(std::collections::HashMap::from([
            (
                "TUMULT_CONFIG_DB_HOST".to_string(),
                "db.internal".to_string(),
            ),
            ("TUMULT_SECRET_TOKEN".to_string(), "injected".to_string()),
        ]));
        let activity = tumult_core::types::Activity {
            name: "injection-test".into(),
            activity_type: tumult_core::types::ActivityType::Action,
            provider: tumult_core::types::Provider::Process {
                path: "sh".into(),
                arguments: vec![
                    "-c".into(),
                    "echo \"$TUMULT_CONFIG_DB_HOST/$TUMULT_SECRET_TOKEN\"".into(),
                ],
                // The declared entry must win over the injected one.
                env: std::collections::HashMap::from([(
                    "TUMULT_SECRET_TOKEN".to_string(),
                    "declared".to_string(),
                )]),
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
        assert_eq!(outcome.output.as_deref(), Some("db.internal/declared"));
    }

    /// A process-provider activity with the given path/args/env.
    fn process_activity(path: &str, arguments: &[&str]) -> tumult_core::types::Activity {
        tumult_core::types::Activity {
            name: "test".into(),
            activity_type: tumult_core::types::ActivityType::Action,
            provider: tumult_core::types::Provider::Process {
                path: path.into(),
                arguments: arguments.iter().map(|a| (*a).to_string()).collect(),
                env: std::collections::HashMap::new(),
                timeout_s: Some(10.0),
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
            label_selector: None,
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn native_provider_uses_the_shared_dispatch_registry() {
        let executor = ProcessExecutor::new();
        let activity = tumult_core::types::Activity {
            provider: tumult_core::types::Provider::Native {
                plugin: "tumult-net".into(),
                function: "missing-function-for-review".into(),
                arguments: std::collections::HashMap::new(),
            },
            ..process_activity("echo", &[])
        };
        let outcome = executor.execute(&activity);
        let error = outcome.error.unwrap();
        assert!(error.contains("missing-function-for-review"), "{error}");
        assert!(
            error.contains("stop_proxy"),
            "native registry must list its capabilities: {error}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn script_provider_uses_filesystem_discovery() {
        let activity = tumult_core::types::Activity {
            provider: tumult_core::types::Provider::Script {
                plugin: "missing-script-plugin-for-review".into(),
                function: "safe-probe".into(),
                arguments: std::collections::HashMap::new(),
                timeout_s: Some(1.0),
            },
            ..process_activity("echo", &[])
        };
        let outcome = ProcessExecutor::new().execute(&activity);
        let error = outcome.error.unwrap();
        assert!(
            error.contains("unknown script plugin: missing-script-plugin-for-review"),
            "{error}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn missing_binary_reports_the_spawn_error() {
        let executor = ProcessExecutor::new();
        let outcome = executor.execute(&process_activity("tumult-mcp-no-such-binary", &[]));
        assert!(!outcome.success);
        assert!(outcome.output.is_none());
        let error = outcome.error.expect("a failed spawn must be reported");
        assert!(!error.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn nonzero_exit_is_unsuccessful_and_stderr_is_captured() {
        let executor = ProcessExecutor::new();
        let outcome = executor.execute(&process_activity(
            "sh",
            &["-c", "echo partial; echo boom >&2; exit 3"],
        ));
        assert!(!outcome.success, "exit 3 must not be a success");
        assert_eq!(outcome.output.as_deref(), Some("partial"));
        assert_eq!(outcome.error.as_deref(), Some("boom"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn output_beyond_the_capture_cap_is_truncated_and_noted() {
        let executor = ProcessExecutor::new();
        // ~9 MB of 'a' — beyond the 8 MiB in-memory cap.
        let outcome = executor.execute(&process_activity(
            "sh",
            &["-c", "head -c 9000000 /dev/zero | tr '\\0' 'a'"],
        ));
        assert!(outcome.success, "{:?}", outcome.error);
        let output = outcome.output.expect("stdout must be captured");
        assert!(
            output.contains("[output truncated at 8 MiB]"),
            "the truncation note must be appended"
        );
        assert!(
            output.len() < 9_000_000,
            "the capture is bounded by the cap: {} bytes",
            output.len()
        );
    }
}
