//! Execution targets, providers, tolerances, and config/secret sources.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::enums::ContainerRuntime;

// ── Execution Target ───────────────────────────────────────────

/// Where a provider executes: locally, over SSH, in a container, or in a
/// Kubernetes pod. Serialized with a `type` tag (snake_case).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecutionTarget {
    /// Execute on the local host.
    Local,
    /// Execute on a remote host over SSH.
    Ssh {
        host: String,
        port: u16,
        user: String,
        /// Private key for authentication; `None` uses the SSH default
        /// (agent or default key paths).
        key_path: Option<PathBuf>,
    },
    /// Execute inside a container via its runtime.
    Container {
        runtime: ContainerRuntime,
        container_id: String,
        /// Optional label selector for filtering containers by Docker/Podman labels.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label_selector: Option<HashMap<String, String>>,
    },
    /// Execute inside a Kubernetes pod.
    KubeExec {
        namespace: String,
        pod: String,
        /// Container within the pod; `None` targets the pod's default container.
        container: Option<String>,
        /// Optional label selector for targeting pods by Kubernetes labels.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label_selector: Option<HashMap<String, String>>,
    },
}

// ── Provider ───────────────────────────────────────────────────

/// How an activity is executed: native plugin, script plugin, or local
/// process. Serialized with a `type` tag (snake_case).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Provider {
    /// Dispatch to a native (in-process) plugin function.
    Native {
        plugin: String,
        function: String,
        #[serde(default)]
        arguments: HashMap<String, serde_json::Value>,
    },
    /// Dispatch a script plugin action (or probe) discovered from the plugin
    /// search paths. `function` names an entry in the plugin manifest's
    /// `actions`/`probes`; `arguments` reach the script as `TUMULT_*`
    /// environment variables (`dns_domain` → `TUMULT_DNS_DOMAIN`).
    Script {
        plugin: String,
        function: String,
        #[serde(default)]
        arguments: HashMap<String, serde_json::Value>,
        #[serde(default)]
        timeout_s: Option<f64>,
    },
    /// Execute a local process; success is judged by exit code unless a
    /// tolerance is set on the activity.
    Process {
        path: String,
        #[serde(default)]
        arguments: Vec<String>,
        #[serde(default)]
        env: HashMap<String, String>,
        /// Execution timeout in seconds; `None` means no timeout.
        timeout_s: Option<f64>,
    },
}

// ── Tolerance ──────────────────────────────────────────────────

/// Expected-output check for a probe or guard. Serialized with a `type` tag
/// (snake_case).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Tolerance {
    /// The actual value must equal `value` (compared as JSON).
    Exact { value: serde_json::Value },
    /// The actual value, parsed as a number, must fall within `[from, to]`.
    Range { from: f64, to: f64 },
    /// The actual value's text form must match `pattern`.
    Regex { pattern: String },
}

// ── Config and Secrets ─────────────────────────────────────────

/// Source of a configuration value: an environment variable or an inline
/// literal. Resolved by the engine at load time. Serialized with a `type`
/// tag (snake_case).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConfigValue {
    /// Resolved from the environment variable named `key`.
    Env { key: String },
    /// A literal value used as-is.
    Inline { value: String },
}

/// Reference to a secret. The secret value itself is never stored in the
/// experiment — only where to find it. The engine resolves the reference at
/// load time and injects the value into provider subprocesses as a
/// `TUMULT_SECRET_*` environment variable; resolved values are never written
/// to the journal. Serialized with a `type` tag (snake_case).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SecretValue {
    /// Read from the environment variable named `key`.
    Env { key: String },
    /// Read from the file at `path`.
    File { path: PathBuf },
}

#[cfg(test)]
mod tests {
    use crate::types::test_support::toon_round_trip;
    use crate::types::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[test]
    fn execution_target_local_round_trips() {
        let target = ExecutionTarget::Local;
        let decoded: ExecutionTarget = toon_round_trip(&target);
        assert_eq!(decoded, ExecutionTarget::Local);
    }

    #[test]
    fn execution_target_ssh_round_trips() {
        let target = ExecutionTarget::Ssh {
            host: "db-primary.example.com".into(),
            port: 22,
            user: "ops".into(),
            key_path: Some(PathBuf::from("/home/ops/.ssh/id_ed25519")),
        };
        let decoded: ExecutionTarget = toon_round_trip(&target);
        assert_eq!(decoded, target);
    }

    #[test]
    fn execution_target_container_round_trips() {
        let target = ExecutionTarget::Container {
            runtime: ContainerRuntime::Docker,
            container_id: "abc123def456".into(),
            label_selector: None,
        };
        let decoded: ExecutionTarget = toon_round_trip(&target);
        assert_eq!(decoded, target);
    }

    #[test]
    fn execution_target_kube_exec_round_trips() {
        let target = ExecutionTarget::KubeExec {
            namespace: "production".into(),
            pod: "api-server-7b8c9d-xk2p1".into(),
            container: Some("app".into()),
            label_selector: None,
        };
        let decoded: ExecutionTarget = toon_round_trip(&target);
        assert_eq!(decoded, target);
    }

    #[test]
    fn provider_native_round_trips() {
        let provider = Provider::Native {
            plugin: "tumult-db".into(),
            function: "terminate_connections".into(),
            arguments: HashMap::from([
                ("host".into(), serde_json::Value::String("localhost".into())),
                ("port".into(), serde_json::Value::Number(5432.into())),
            ]),
        };
        let decoded: Provider = toon_round_trip(&provider);
        assert_eq!(decoded, provider);
    }

    #[test]
    fn provider_process_round_trips() {
        let provider = Provider::Process {
            path: "scripts/kill-broker.sh".into(),
            arguments: vec!["--broker-id".into(), "2".into()],
            env: HashMap::from([("CLUSTER".into(), "prod".into())]),
            timeout_s: Some(30.0),
        };
        let decoded: Provider = toon_round_trip(&provider);
        assert_eq!(decoded, provider);
    }

    #[test]
    fn provider_script_round_trips() {
        let provider = Provider::Script {
            plugin: "tumult-network".into(),
            function: "redirect-dns".into(),
            arguments: HashMap::from([(
                "dns_domain".into(),
                serde_json::Value::String("example.com".into()),
            )]),
            timeout_s: Some(10.0),
        };
        let decoded: Provider = toon_round_trip(&provider);
        assert_eq!(decoded, provider);
    }

    #[test]
    fn provider_script_defaults_arguments_and_timeout() {
        let provider: Provider = serde_json::from_str(
            r#"{"type": "script", "plugin": "tumult-network", "function": "reset-tc"}"#,
        )
        .unwrap();
        assert_eq!(
            provider,
            Provider::Script {
                plugin: "tumult-network".into(),
                function: "reset-tc".into(),
                arguments: HashMap::new(),
                timeout_s: None,
            }
        );
    }

    #[test]
    fn tolerance_exact_round_trips() {
        let t = Tolerance::Exact {
            value: serde_json::Value::Number(200.into()),
        };
        let decoded: Tolerance = toon_round_trip(&t);
        assert_eq!(decoded, t);
    }

    #[test]
    fn tolerance_range_round_trips() {
        let t = Tolerance::Range {
            from: 0.0,
            to: 500.0,
        };
        let decoded: Tolerance = toon_round_trip(&t);
        assert_eq!(decoded, t);
    }

    #[test]
    fn tolerance_regex_round_trips() {
        let t = Tolerance::Regex {
            pattern: "^OK.*".into(),
        };
        let decoded: Tolerance = toon_round_trip(&t);
        assert_eq!(decoded, t);
    }

    #[test]
    fn execution_target_kube_exec_with_label_selector_round_trips() {
        let mut selector = HashMap::new();
        selector.insert("app".to_string(), "worker".to_string());

        let target = ExecutionTarget::KubeExec {
            namespace: "production".into(),
            pod: String::new(),
            container: None,
            label_selector: Some(selector),
        };
        let decoded: ExecutionTarget = toon_round_trip(&target);
        assert_eq!(decoded, target);
        let ExecutionTarget::KubeExec { label_selector, .. } = &decoded else {
            panic!("expected KubeExec");
        };
        assert_eq!(
            label_selector.as_ref().unwrap().get("app").unwrap(),
            "worker"
        );
    }

    #[test]
    fn execution_target_container_with_label_selector_round_trips() {
        let mut selector = HashMap::new();
        selector.insert(
            "com.docker.compose.service".to_string(),
            "redis".to_string(),
        );

        let target = ExecutionTarget::Container {
            runtime: ContainerRuntime::Docker,
            container_id: String::new(),
            label_selector: Some(selector),
        };
        let decoded: ExecutionTarget = toon_round_trip(&target);
        assert_eq!(decoded, target);
        let ExecutionTarget::Container { label_selector, .. } = &decoded else {
            panic!("expected Container");
        };
        assert_eq!(
            label_selector
                .as_ref()
                .unwrap()
                .get("com.docker.compose.service")
                .unwrap(),
            "redis"
        );
    }
}
