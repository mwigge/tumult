//! Native dispatch — [`NativeExecutor`] implementation for `tumult-kubernetes`.
//!
//! Routes `tumult-kubernetes.<function>` calls to [`crate::actions`] and
//! [`crate::probes`]. A `kube` client is built from the ambient credentials
//! (in-cluster service account, `KUBECONFIG`, or `~/.kube/config`) per call.

use tumult_plugin::native::{arg_num, arg_str, NativeArgs, NativeError, NativeExecutor};

use crate::inject::{self, StressKind};
use crate::{actions, probes};

/// Functions `tumult-kubernetes` provides to the experiment runner.
const FUNCTIONS: &[&str] = &[
    "delete_pod",
    "scale_deployment",
    "cordon_node",
    "uncordon_node",
    "drain_node",
    "apply_network_policy",
    "delete_network_policy",
    "pod_network_latency",
    "pod_stress",
    "pod_is_ready",
    "deployment_is_ready",
    "all_pods_ready",
    "node_status",
    "service_has_endpoints",
    "count_pods_in_phase",
];

/// Read an optional string argument, returning `None` when absent or non-string.
fn opt_str<'a>(args: &'a NativeArgs, key: &str) -> Option<&'a str> {
    args.get(key).and_then(serde_json::Value::as_str)
}

/// Resolve the target pod for an in-pod fault: use the explicit `pod` argument,
/// else resolve the first pod matching `label_selector` (reporting how many
/// pods matched, so a wide selector's arbitrary victim is visible).
async fn resolve_pod(
    client: kube::Client,
    args: &NativeArgs,
    namespace: &str,
) -> Result<inject::ResolvedPod, NativeError> {
    let pod = opt_str(args, "pod");
    let selector = opt_str(args, "label_selector");
    if pod.is_none() && selector.is_none() {
        return Err(NativeError::MissingArgument {
            argument: "pod (or label_selector)".to_string(),
        });
    }
    inject::resolve_target_pod(client, namespace, pod, selector)
        .await
        .map_err(NativeError::execution)
}

/// Parse the stress kind from arguments. Exactly one of `cpu_workers` or
/// `mem_bytes` must be present.
fn stress_kind(args: &NativeArgs) -> Result<StressKind, NativeError> {
    let cpu = arg_num::<u32>(args, "cpu_workers");
    let mem = args.get("mem_bytes").and_then(serde_json::Value::as_u64);
    match (cpu, mem) {
        (Some(_), Some(_)) => Err(NativeError::invalid_argument(
            "cpu_workers",
            "set exactly one of `cpu_workers` or `mem_bytes`, not both",
        )),
        (Some(workers), None) => Ok(StressKind::Cpu { workers }),
        (None, Some(bytes)) => Ok(StressKind::Memory { bytes }),
        (None, None) => Err(NativeError::MissingArgument {
            argument: "cpu_workers (or mem_bytes)".to_string(),
        }),
    }
}

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

    #[allow(clippy::too_many_lines)] // Flat dispatch over all registered functions; one match arm per function
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
            "drain_node" => {
                let name = arg_str(args, "name")?;
                let grace = arg_num::<u32>(args, "grace_period_seconds");
                let result = actions::drain_node(client, name, grace)
                    .await
                    .map_err(NativeError::execution)?;
                Ok(result.to_string())
            }
            "apply_network_policy" => {
                let ns = arg_str(args, "namespace")?;
                let policy_value =
                    args.get("policy")
                        .cloned()
                        .ok_or_else(|| NativeError::MissingArgument {
                            argument: "policy".to_string(),
                        })?;
                let policy = serde_json::from_value(policy_value).map_err(|e| {
                    NativeError::invalid_argument("policy", format!("invalid NetworkPolicy: {e}"))
                })?;
                actions::apply_network_policy(client, ns, policy)
                    .await
                    .map_err(NativeError::execution)
            }
            "delete_network_policy" => {
                let ns = arg_str(args, "namespace")?;
                let name = arg_str(args, "name")?;
                actions::delete_network_policy(client, ns, name)
                    .await
                    .map_err(NativeError::execution)
            }
            "pod_network_latency" => {
                let ns = arg_str(args, "namespace")?;
                let pod = resolve_pod(client.clone(), args, ns).await?;
                let delay_ms = arg_num::<u32>(args, "delay_ms").ok_or_else(|| {
                    NativeError::MissingArgument {
                        argument: "delay_ms".to_string(),
                    }
                })?;
                let jitter_ms = arg_num::<u32>(args, "jitter_ms").unwrap_or(0);
                let duration_s = arg_num::<u32>(args, "duration_s").unwrap_or(30);
                let iface = opt_str(args, "iface").unwrap_or(inject::DEFAULT_IFACE);
                let image = opt_str(args, "image").unwrap_or(inject::DEFAULT_NETEM_IMAGE);
                let out = inject::pod_network_latency(
                    client, ns, &pod.name, delay_ms, jitter_ms, duration_s, iface, image,
                )
                .await
                .map_err(NativeError::execution)?;
                Ok(pod.annotate(&out))
            }
            "pod_stress" => {
                let ns = arg_str(args, "namespace")?;
                let pod = resolve_pod(client.clone(), args, ns).await?;
                let duration_s = arg_num::<u32>(args, "duration_s").unwrap_or(30);
                let kind = stress_kind(args)?;
                let target_container = opt_str(args, "target_container");
                let image = opt_str(args, "image").unwrap_or(inject::DEFAULT_STRESS_IMAGE);
                let out = inject::pod_stress(
                    client,
                    ns,
                    &pod.name,
                    kind,
                    duration_s,
                    target_container,
                    image,
                )
                .await
                .map_err(NativeError::execution)?;
                Ok(pod.annotate(&out))
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
            "service_has_endpoints" => {
                let ns = arg_str(args, "namespace")?;
                let name = arg_str(args, "name")?;
                let has = probes::service_has_endpoints(client, ns, name)
                    .await
                    .map_err(NativeError::execution)?;
                Ok(format!("{has}"))
            }
            "count_pods_in_phase" => {
                let ns = arg_str(args, "namespace")?;
                let selector = arg_str(args, "label_selector")?;
                let phase = arg_str(args, "phase")?;
                let count = probes::count_pods_in_phase(client, ns, selector, phase)
                    .await
                    .map_err(NativeError::execution)?;
                Ok(format!("{count}"))
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
        assert!(executor.functions().contains(&"pod_network_latency"));
        assert!(executor.functions().contains(&"pod_stress"));
    }

    #[test]
    fn registry_covers_every_implemented_function() {
        let executor = KubernetesExecutor;
        let expected = [
            "delete_pod",
            "scale_deployment",
            "cordon_node",
            "uncordon_node",
            "drain_node",
            "apply_network_policy",
            "delete_network_policy",
            "pod_network_latency",
            "pod_stress",
            "pod_is_ready",
            "deployment_is_ready",
            "all_pods_ready",
            "node_status",
            "service_has_endpoints",
            "count_pods_in_phase",
        ];
        assert_eq!(executor.functions().len(), expected.len());
        for function in expected {
            assert!(
                executor.functions().contains(&function),
                "{function} is implemented but not registered"
            );
        }
    }

    #[test]
    fn stress_kind_requires_exactly_one_of_cpu_or_mem() {
        let mut args = NativeArgs::new();
        assert!(
            matches!(stress_kind(&args), Err(NativeError::MissingArgument { .. })),
            "no stress arg must be a missing-argument error"
        );

        args.insert("cpu_workers".into(), serde_json::json!(4));
        assert!(matches!(
            stress_kind(&args),
            Ok(StressKind::Cpu { workers: 4 })
        ));

        args.insert("mem_bytes".into(), serde_json::json!(1024));
        assert!(
            matches!(stress_kind(&args), Err(NativeError::InvalidArgument { .. })),
            "both cpu and mem set must be rejected"
        );

        args.remove("cpu_workers");
        assert!(matches!(
            stress_kind(&args),
            Ok(StressKind::Memory { bytes: 1024 })
        ));
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
