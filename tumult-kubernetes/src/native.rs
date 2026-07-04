//! Native dispatch — [`NativeExecutor`] implementation for `tumult-kubernetes`.
//!
//! Routes `tumult-kubernetes.<function>` calls to [`crate::actions`] and
//! [`crate::probes`]. A `kube` client is built from the ambient credentials
//! (in-cluster service account, `KUBECONFIG`, or `~/.kube/config`) per call.

use tumult_plugin::native::{arg_num, arg_str, NativeArgs, NativeError, NativeExecutor};

use crate::{actions, probes};

/// Functions `tumult-kubernetes` provides to the experiment runner.
const FUNCTIONS: &[&str] = &[
    "delete_pod",
    "scale_deployment",
    "cordon_node",
    "uncordon_node",
    "pod_is_ready",
    "deployment_is_ready",
    "all_pods_ready",
    "node_status",
];

/// [`NativeExecutor`] for the `tumult-kubernetes` plugin.
pub struct KubernetesExecutor;

#[async_trait::async_trait(?Send)]
impl NativeExecutor for KubernetesExecutor {
    fn name(&self) -> &'static str {
        "tumult-kubernetes"
    }

    fn functions(&self) -> &'static [&'static str] {
        FUNCTIONS
    }

    async fn execute(&self, function: &str, args: &NativeArgs) -> Result<String, NativeError> {
        // Validate the function name before building a client, so typos fail
        // fast with the available-function list instead of a connection error.
        if !FUNCTIONS.contains(&function) {
            return Err(NativeError::unknown_function(
                self.name(),
                function,
                FUNCTIONS,
            ));
        }

        let client = kube::Client::try_default()
            .await
            .map_err(|e| NativeError::execution_context("kubernetes client init failed", e))?;

        match function {
            "delete_pod" => {
                let ns = arg_str(args, "namespace")?;
                let name = arg_str(args, "name")?;
                let grace = arg_num::<u32>(args, "grace_period_seconds");
                actions::delete_pod(client, ns, name, grace)
                    .await
                    .map_err(NativeError::execution)
            }
            "scale_deployment" => {
                let ns = arg_str(args, "namespace")?;
                let name = arg_str(args, "name")?;
                let replicas = arg_num::<i32>(args, "replicas").ok_or_else(|| {
                    NativeError::MissingArgument {
                        argument: "replicas".to_string(),
                    }
                })?;
                actions::scale_deployment(client, ns, name, replicas)
                    .await
                    .map_err(NativeError::execution)
            }
            "cordon_node" => {
                let name = arg_str(args, "name")?;
                actions::cordon_node(client, name)
                    .await
                    .map_err(NativeError::execution)
            }
            "uncordon_node" => {
                let name = arg_str(args, "name")?;
                actions::uncordon_node(client, name)
                    .await
                    .map_err(NativeError::execution)
            }
            "pod_is_ready" => {
                let ns = arg_str(args, "namespace")?;
                let name = arg_str(args, "name")?;
                let ready = probes::pod_is_ready(client, ns, name)
                    .await
                    .map_err(NativeError::execution)?;
                Ok(format!("{ready}"))
            }
            "deployment_is_ready" => {
                let ns = arg_str(args, "namespace")?;
                let name = arg_str(args, "name")?;
                let status = probes::deployment_is_ready(client, ns, name)
                    .await
                    .map_err(NativeError::execution)?;
                serde_json::to_string(&status).map_err(NativeError::execution)
            }
            "all_pods_ready" => {
                let ns = arg_str(args, "namespace")?;
                let selector = arg_str(args, "label_selector")?;
                let (total, ready) = probes::all_pods_ready(client, ns, selector)
                    .await
                    .map_err(NativeError::execution)?;
                Ok(format!("{{\"total\":{total},\"ready\":{ready}}}"))
            }
            "node_status" => {
                let name = arg_str(args, "name")?;
                let status = probes::node_status(client, name)
                    .await
                    .map_err(NativeError::execution)?;
                serde_json::to_string(&status).map_err(NativeError::execution)
            }
            _ => Err(NativeError::unknown_function(
                self.name(),
                function,
                FUNCTIONS,
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_names_plugin_and_functions() {
        let executor = KubernetesExecutor;
        assert_eq!(executor.name(), "tumult-kubernetes");
        assert!(executor.functions().contains(&"delete_pod"));
        assert!(executor.functions().contains(&"node_status"));
    }

    #[tokio::test]
    async fn unknown_function_is_rejected_before_client_init() {
        let err = KubernetesExecutor
            .execute("explode_cluster", &NativeArgs::new())
            .await
            .unwrap_err();
        assert!(
            matches!(err, NativeError::UnknownFunction { .. }),
            "expected UnknownFunction, got: {err:?}"
        );
        assert!(err.to_string().contains("delete_pod"));
    }
}
