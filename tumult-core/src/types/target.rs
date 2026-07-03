//! Execution targets, providers, tolerances, and config/secret sources.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::enums::{ContainerRuntime, HttpMethod};

// ── Execution Target ───────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecutionTarget {
    Local,
    Ssh {
        host: String,
        port: u16,
        user: String,
        key_path: Option<PathBuf>,
    },
    Container {
        runtime: ContainerRuntime,
        container_id: String,
        /// Optional label selector for filtering containers by Docker/Podman labels.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label_selector: Option<HashMap<String, String>>,
    },
    KubeExec {
        namespace: String,
        pod: String,
        container: Option<String>,
        /// Optional label selector for targeting pods by Kubernetes labels.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label_selector: Option<HashMap<String, String>>,
    },
}

// ── Provider ───────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Provider {
    Native {
        plugin: String,
        function: String,
        #[serde(default)]
        arguments: HashMap<String, serde_json::Value>,
    },
    Process {
        path: String,
        #[serde(default)]
        arguments: Vec<String>,
        #[serde(default)]
        env: HashMap<String, String>,
        timeout_s: Option<f64>,
    },
    Http {
        method: HttpMethod,
        url: String,
        #[serde(default)]
        headers: HashMap<String, String>,
        body: Option<String>,
        timeout_s: Option<f64>,
    },
}

// ── Tolerance ──────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Tolerance {
    Exact { value: serde_json::Value },
    Range { from: f64, to: f64 },
    Regex { pattern: String },
}

// ── Config and Secrets ─────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConfigValue {
    Env { key: String },
    Inline { value: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SecretValue {
    Env { key: String },
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
    fn provider_http_round_trips() {
        let provider = Provider::Http {
            method: HttpMethod::Get,
            url: "http://localhost:8080/health".into(),
            headers: HashMap::from([("Accept".into(), "application/json".into())]),
            body: None,
            timeout_s: Some(5.0),
        };
        let decoded: Provider = toon_round_trip(&provider);
        assert_eq!(decoded, provider);
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
