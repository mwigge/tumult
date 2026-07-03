use tumult_core::runner::ActivityOutcome;

use super::{arg_num, arg_str, net};

/// Dispatch a native plugin call to the appropriate Rust function.
///
/// Routes `plugin::function` to `tumult-kubernetes` or `tumult-ssh`
/// implementations. Runs async functions on the current Tokio runtime.
///
/// # Panics
///
/// Panics if called from outside a Tokio multi-threaded runtime context.
/// `tokio::task::block_in_place` requires the `multi_thread` scheduler; it
/// will panic when used with `current_thread` or with no active runtime.
pub(super) fn execute_native(
    plugin: &str,
    function: &str,
    arguments: &std::collections::HashMap<String, serde_json::Value>,
) -> ActivityOutcome {
    let start = std::time::Instant::now();

    let result = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(dispatch_native(plugin, function, arguments))
    });

    let duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);

    match result {
        Ok(output) => ActivityOutcome {
            success: true,
            output: Some(output),
            error: None,
            duration_ms,
        },
        Err(e) => ActivityOutcome {
            success: false,
            output: None,
            error: Some(e),
            duration_ms,
        },
    }
}

/// Async dispatch table for native plugins.
async fn dispatch_native(
    plugin: &str,
    function: &str,
    args: &std::collections::HashMap<String, serde_json::Value>,
) -> Result<String, String> {
    match plugin {
        "tumult-kubernetes" => dispatch_kubernetes(function, args).await,
        "tumult-ssh" => dispatch_ssh(function, args).await,
        "tumult-net" => net::dispatch_net(function, args).await,
        _ => Err(format!("unknown native plugin: {plugin}")),
    }
}

/// Dispatch to tumult-kubernetes functions.
async fn dispatch_kubernetes(
    function: &str,
    args: &std::collections::HashMap<String, serde_json::Value>,
) -> Result<String, String> {
    let client = kube::Client::try_default()
        .await
        .map_err(|e| format!("kubernetes client init failed: {e}"))?;

    match function {
        "delete_pod" => {
            let ns = arg_str(args, "namespace")?;
            let name = arg_str(args, "name")?;
            let grace = arg_num::<u32>(args, "grace_period_seconds");
            tumult_kubernetes::actions::delete_pod(client, ns, name, grace)
                .await
                .map_err(|e| format!("{e}"))
        }
        "scale_deployment" => {
            let ns = arg_str(args, "namespace")?;
            let name = arg_str(args, "name")?;
            let replicas = arg_num::<i32>(args, "replicas").ok_or("missing argument: replicas")?;
            tumult_kubernetes::actions::scale_deployment(client, ns, name, replicas)
                .await
                .map_err(|e| format!("{e}"))
        }
        "cordon_node" => {
            let name = arg_str(args, "name")?;
            tumult_kubernetes::actions::cordon_node(client, name)
                .await
                .map_err(|e| format!("{e}"))
        }
        "uncordon_node" => {
            let name = arg_str(args, "name")?;
            tumult_kubernetes::actions::uncordon_node(client, name)
                .await
                .map_err(|e| format!("{e}"))
        }
        "pod_is_ready" => {
            let ns = arg_str(args, "namespace")?;
            let name = arg_str(args, "name")?;
            let ready = tumult_kubernetes::probes::pod_is_ready(client, ns, name)
                .await
                .map_err(|e| format!("{e}"))?;
            Ok(format!("{ready}"))
        }
        "deployment_is_ready" => {
            let ns = arg_str(args, "namespace")?;
            let name = arg_str(args, "name")?;
            let status = tumult_kubernetes::probes::deployment_is_ready(client, ns, name)
                .await
                .map_err(|e| format!("{e}"))?;
            serde_json::to_string(&status).map_err(|e| format!("{e}"))
        }
        "all_pods_ready" => {
            let ns = arg_str(args, "namespace")?;
            let selector = arg_str(args, "label_selector")?;
            let (total, ready) = tumult_kubernetes::probes::all_pods_ready(client, ns, selector)
                .await
                .map_err(|e| format!("{e}"))?;
            Ok(format!("{{\"total\":{total},\"ready\":{ready}}}"))
        }
        "node_status" => {
            let name = arg_str(args, "name")?;
            let status = tumult_kubernetes::probes::node_status(client, name)
                .await
                .map_err(|e| format!("{e}"))?;
            serde_json::to_string(&status).map_err(|e| format!("{e}"))
        }
        _ => Err(format!("unknown tumult-kubernetes function: {function}")),
    }
}

/// Dispatch to tumult-ssh functions.
async fn dispatch_ssh(
    _function: &str,
    args: &std::collections::HashMap<String, serde_json::Value>,
) -> Result<String, String> {
    let host = arg_str(args, "host")?;
    let port = arg_num::<u16>(args, "port").unwrap_or(22);
    let user = arg_str(args, "user")?;
    let command = arg_str(args, "command")?;

    let key_path = args
        .get("key_file")
        .and_then(serde_json::Value::as_str)
        .map(std::path::PathBuf::from);

    let auth = if let Some(ref path) = key_path {
        tumult_ssh::AuthMethod::Key {
            key_path: path.clone(),
            passphrase: None,
        }
    } else {
        tumult_ssh::AuthMethod::Agent
    };

    let config = tumult_ssh::SshConfig {
        host: host.to_string(),
        port,
        user: user.to_string(),
        auth,
        host_key_policy: tumult_ssh::HostKeyPolicy::AcceptAny,
        connect_timeout: std::time::Duration::from_secs(30),
        command_timeout: Some(std::time::Duration::from_secs(60)),
        known_hosts_path: None,
    };

    let session = tumult_ssh::SshSession::connect(config)
        .await
        .map_err(|e| format!("SSH connect failed: {e}"))?;

    let result = session
        .execute(command)
        .await
        .map_err(|e| format!("SSH execute failed: {e}"))?;

    let _ = session.close().await;

    if result.exit_code == 0 {
        Ok(result.stdout)
    } else {
        Err(format!(
            "SSH command exited {}: {}",
            result.exit_code, result.stderr
        ))
    }
}
