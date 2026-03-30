//! Kubernetes chaos actions.
//!
//! Actions that mutate cluster state: delete pods, drain nodes,
//! scale deployments, apply network policies.

use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{Node, Pod};
use k8s_openapi::api::networking::v1::NetworkPolicy;
use kube::api::{Api, DeleteParams, Patch, PatchParams};
use kube::Client;

use crate::error::KubeError;

/// Delete a pod by name. Optionally set a grace period.
pub async fn delete_pod(
    client: Client,
    namespace: &str,
    name: &str,
    grace_period_seconds: Option<u32>,
) -> Result<String, KubeError> {
    let pods: Api<Pod> = Api::namespaced(client, namespace);
    let mut dp = DeleteParams::default();
    if let Some(grace) = grace_period_seconds {
        dp = dp.grace_period(grace);
    }
    pods.delete(name, &dp).await?;
    Ok(format!("pod {}/{} deleted", namespace, name))
}

/// Scale a deployment to a target number of replicas.
pub async fn scale_deployment(
    client: Client,
    namespace: &str,
    name: &str,
    replicas: i32,
) -> Result<String, KubeError> {
    let deployments: Api<Deployment> = Api::namespaced(client, namespace);

    let patch = serde_json::json!({
        "spec": {
            "replicas": replicas
        }
    });

    deployments
        .patch(name, &PatchParams::apply("tumult"), &Patch::Merge(&patch))
        .await?;

    Ok(format!(
        "deployment {}/{} scaled to {} replicas",
        namespace, name, replicas
    ))
}

/// Cordon a node (mark as unschedulable).
pub async fn cordon_node(client: Client, name: &str) -> Result<String, KubeError> {
    let nodes: Api<Node> = Api::all(client);

    let patch = serde_json::json!({
        "spec": {
            "unschedulable": true
        }
    });

    nodes
        .patch(name, &PatchParams::apply("tumult"), &Patch::Merge(&patch))
        .await?;

    Ok(format!("node {} cordoned", name))
}

/// Uncordon a node (mark as schedulable).
pub async fn uncordon_node(client: Client, name: &str) -> Result<String, KubeError> {
    let nodes: Api<Node> = Api::all(client);

    let patch = serde_json::json!({
        "spec": {
            "unschedulable": false
        }
    });

    nodes
        .patch(name, &PatchParams::apply("tumult"), &Patch::Merge(&patch))
        .await?;

    Ok(format!("node {} uncordoned", name))
}

/// Result of a node drain operation.
#[derive(Debug, Clone)]
pub struct DrainResult {
    pub node: String,
    pub evicted: Vec<String>,
    pub failed: Vec<(String, String)>,
    pub skipped_daemonsets: usize,
}

impl std::fmt::Display for DrainResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "node {} drained: {} evicted, {} failed, {} daemonset pods skipped",
            self.node,
            self.evicted.len(),
            self.failed.len(),
            self.skipped_daemonsets
        )
    }
}

/// Drain a node: cordon it, then delete all non-DaemonSet pods on it.
pub async fn drain_node(
    client: Client,
    name: &str,
    grace_period_seconds: Option<u32>,
) -> Result<DrainResult, KubeError> {
    cordon_node(client.clone(), name).await?;

    let pods: Api<Pod> = Api::all(client.clone());
    let pod_list = pods
        .list(&kube::api::ListParams::default().fields(&format!("spec.nodeName={}", name)))
        .await?;

    let mut evicted = Vec::new();
    let mut failed = Vec::new();
    let mut skipped_daemonsets = 0;
    let mut dp = DeleteParams::default();
    if let Some(grace) = grace_period_seconds {
        dp = dp.grace_period(grace);
    }

    for pod in pod_list {
        let pod_name = pod.metadata.name.unwrap_or_default();
        let pod_ns = pod.metadata.namespace.unwrap_or_else(|| "default".into());

        if let Some(refs) = &pod.metadata.owner_references {
            if refs.iter().any(|r| r.kind == "DaemonSet") {
                skipped_daemonsets += 1;
                continue;
            }
        }

        let ns_pods: Api<Pod> = Api::namespaced(client.clone(), &pod_ns);
        match ns_pods.delete(&pod_name, &dp).await {
            Ok(_) => evicted.push(format!("{}/{}", pod_ns, pod_name)),
            Err(e) => failed.push((format!("{}/{}", pod_ns, pod_name), e.to_string())),
        }
    }

    Ok(DrainResult {
        node: name.to_string(),
        evicted,
        failed,
        skipped_daemonsets,
    })
}

/// Apply a network policy to a namespace.
pub async fn apply_network_policy(
    client: Client,
    namespace: &str,
    policy: NetworkPolicy,
) -> Result<String, KubeError> {
    let policies: Api<NetworkPolicy> = Api::namespaced(client, namespace);
    let name = policy
        .metadata
        .name
        .clone()
        .unwrap_or_else(|| "tumult-policy".into());
    policies
        .patch(&name, &PatchParams::apply("tumult"), &Patch::Apply(&policy))
        .await?;
    Ok(format!("network policy {}/{} applied", namespace, name))
}

/// Delete a network policy from a namespace.
pub async fn delete_network_policy(
    client: Client,
    namespace: &str,
    name: &str,
) -> Result<String, KubeError> {
    let policies: Api<NetworkPolicy> = Api::namespaced(client, namespace);
    policies.delete(name, &DeleteParams::default()).await?;
    Ok(format!("network policy {}/{} deleted", namespace, name))
}

#[cfg(test)]
mod tests {
    // K8s actions require a live cluster — tests are integration-only.
    // Unit tests validate error type construction.
    use super::*;

    #[test]
    fn delete_params_with_grace_period() {
        let dp = DeleteParams::default().grace_period(30);
        assert_eq!(dp.grace_period_seconds, Some(30));
    }

    #[test]
    fn error_formats_invalid_config() {
        let err = KubeError::InvalidConfig("replicas must be >= 0".into());
        assert!(err.to_string().contains("replicas"));
    }
}
