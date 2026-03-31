//! Kubernetes status probes.
//!
//! Probes that observe cluster state without modifying it.
//! Follows the patterns established by Chaos Toolkit, LitmusChaos,
//! and Chaos Mesh for verifying steady state.

use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{Node, Pod};
use kube::api::{Api, ListParams};
use kube::Client;

use crate::error::KubeError;

/// Pod status summary.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PodStatus {
    pub name: String,
    pub namespace: String,
    pub phase: String,
    pub ready: bool,
    pub restarts: i32,
    pub node: String,
}

/// Deployment status summary.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DeploymentStatus {
    pub name: String,
    pub namespace: String,
    pub desired: i32,
    pub ready: i32,
    pub available: i32,
    pub up_to_date: i32,
}

/// Node condition summary.
#[derive(Debug, Clone, serde::Serialize)]
pub struct NodeStatus {
    pub name: String,
    pub ready: bool,
    pub schedulable: bool,
    pub conditions: Vec<NodeCondition>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct NodeCondition {
    pub condition_type: String,
    pub status: String,
}

/// Check if a specific pod is running and ready.
pub async fn pod_is_ready(client: Client, namespace: &str, name: &str) -> Result<bool, KubeError> {
    let _span = crate::telemetry::begin_pod_probe(namespace, name);
    let pods: Api<Pod> = Api::namespaced(client, namespace);
    let pod = pods.get(name).await?;
    Ok(is_pod_ready(&pod))
}

/// List pods matching a label selector and return their statuses.
/// This follows the Chaos Mesh / LitmusChaos pattern of targeting by labels.
pub async fn pods_by_label(
    client: Client,
    namespace: &str,
    label_selector: &str,
) -> Result<Vec<PodStatus>, KubeError> {
    let _span = crate::telemetry::begin_pods_by_label(namespace, label_selector);
    let pods: Api<Pod> = Api::namespaced(client, namespace);
    let lp = ListParams::default().labels(label_selector);
    let pod_list = pods.list(&lp).await?;

    Ok(pod_list.into_iter().map(pod_to_status).collect())
}

/// Check if all pods matching a label selector are ready.
/// Returns (total, ready) count.
pub async fn all_pods_ready(
    client: Client,
    namespace: &str,
    label_selector: &str,
) -> Result<(usize, usize), KubeError> {
    let statuses = pods_by_label(client, namespace, label_selector).await?;
    let total = statuses.len();
    let ready = statuses.iter().filter(|s| s.ready).count();
    Ok((total, ready))
}

/// Check if a deployment has all replicas available.
pub async fn deployment_is_ready(
    client: Client,
    namespace: &str,
    name: &str,
) -> Result<DeploymentStatus, KubeError> {
    let _span = crate::telemetry::begin_deployment_probe(namespace, name);
    let deployments: Api<Deployment> = Api::namespaced(client, namespace);
    let deployment = deployments.get(name).await?;

    let spec_replicas = deployment
        .spec
        .as_ref()
        .and_then(|s| s.replicas)
        .unwrap_or(1);

    let status = deployment.status.as_ref();
    let ready = status.and_then(|s| s.ready_replicas).unwrap_or(0);
    let available = status.and_then(|s| s.available_replicas).unwrap_or(0);
    let updated = status.and_then(|s| s.updated_replicas).unwrap_or(0);

    Ok(DeploymentStatus {
        name: name.to_string(),
        namespace: namespace.to_string(),
        desired: spec_replicas,
        ready,
        available,
        up_to_date: updated,
    })
}

/// Get node conditions and schedulability status.
pub async fn node_status(client: Client, name: &str) -> Result<NodeStatus, KubeError> {
    let _span = crate::telemetry::begin_node_status(name);
    let nodes: Api<Node> = Api::all(client);
    let node = nodes.get(name).await?;

    let schedulable = !node
        .spec
        .as_ref()
        .and_then(|s| s.unschedulable)
        .unwrap_or(false);

    let conditions: Vec<NodeCondition> = node
        .status
        .as_ref()
        .and_then(|s| s.conditions.as_ref())
        .map(|conds| {
            conds
                .iter()
                .map(|c| NodeCondition {
                    condition_type: c.type_.clone(),
                    status: c.status.clone(),
                })
                .collect()
        })
        .unwrap_or_default();

    let ready = conditions
        .iter()
        .any(|c| c.condition_type == "Ready" && c.status == "True");

    Ok(NodeStatus {
        name: name.to_string(),
        ready,
        schedulable,
        conditions,
    })
}

/// Check if a service has endpoints.
pub async fn service_has_endpoints(
    client: Client,
    namespace: &str,
    name: &str,
) -> Result<bool, KubeError> {
    let endpoints: Api<k8s_openapi::api::core::v1::Endpoints> = Api::namespaced(client, namespace);
    let ep = endpoints.get(name).await?;

    let has_addresses = ep
        .subsets
        .as_ref()
        .map(|subsets| {
            subsets
                .iter()
                .any(|s| s.addresses.as_ref().is_some_and(|a| !a.is_empty()))
        })
        .unwrap_or(false);

    Ok(has_addresses)
}

/// Count pods matching a label selector that are in a given phase.
pub async fn count_pods_in_phase(
    client: Client,
    namespace: &str,
    label_selector: &str,
    phase: &str,
) -> Result<usize, KubeError> {
    let statuses = pods_by_label(client, namespace, label_selector).await?;
    Ok(statuses.iter().filter(|s| s.phase == phase).count())
}

// ── Helpers ───────────────────────────────────────────────────

fn is_pod_ready(pod: &Pod) -> bool {
    pod.status
        .as_ref()
        .and_then(|s| s.conditions.as_ref())
        .map(|conds| {
            conds
                .iter()
                .any(|c| c.type_ == "Ready" && c.status == "True")
        })
        .unwrap_or(false)
}

fn pod_to_status(pod: Pod) -> PodStatus {
    let ready = is_pod_ready(&pod);
    let name = pod.metadata.name.unwrap_or_default();
    let namespace = pod.metadata.namespace.unwrap_or_else(|| "default".into());

    let phase = pod
        .status
        .as_ref()
        .and_then(|s| s.phase.clone())
        .unwrap_or_else(|| "Unknown".into());

    let restarts = pod
        .status
        .as_ref()
        .and_then(|s| s.container_statuses.as_ref())
        .map(|cs| cs.iter().map(|c| c.restart_count).sum())
        .unwrap_or(0);

    let node = pod
        .spec
        .as_ref()
        .and_then(|s| s.node_name.clone())
        .unwrap_or_default();

    PodStatus {
        name,
        namespace,
        phase,
        ready,
        restarts,
        node,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pod_status_serializes_to_json() {
        let status = PodStatus {
            name: "api-xyz".into(),
            namespace: "prod".into(),
            phase: "Running".into(),
            ready: true,
            restarts: 0,
            node: "worker-01".into(),
        };
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("api-xyz"));
        assert!(json.contains("Running"));
    }

    #[test]
    fn deployment_status_serializes() {
        let status = DeploymentStatus {
            name: "web".into(),
            namespace: "prod".into(),
            desired: 3,
            ready: 3,
            available: 3,
            up_to_date: 3,
        };
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"desired\":3"));
    }

    #[test]
    fn node_status_serializes() {
        let status = NodeStatus {
            name: "worker-01".into(),
            ready: true,
            schedulable: true,
            conditions: vec![NodeCondition {
                condition_type: "Ready".into(),
                status: "True".into(),
            }],
        };
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("worker-01"));
        assert!(json.contains("Ready"));
    }
}
