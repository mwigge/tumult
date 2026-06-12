use tumult_core::runner::{ActivityExecutor, ActivityOutcome};
use tumult_core::types::{Activity, HttpMethod, Provider};

// ── Provider-based executor ───────────────────────────────────

/// Executes activities by dispatching to the appropriate provider.
///
/// Supports Process, HTTP, and Native (Rust) providers.
/// Native plugins dispatch to `tumult-kubernetes` and `tumult-ssh`
/// functions via async execution on the Tokio runtime.
pub struct ProviderExecutor;

impl ActivityExecutor for ProviderExecutor {
    fn execute(&self, activity: &Activity) -> ActivityOutcome {
        match &activity.provider {
            Provider::Process {
                path,
                arguments,
                env,
                timeout_s,
            } => execute_process(path, arguments, env, timeout_s.as_ref()),
            Provider::Http {
                method,
                url,
                headers: _,
                body: _,
                timeout_s: _,
            } => {
                tracing::error!(
                    method = format_http_method(method),
                    url = %url,
                    "HTTP provider not yet implemented"
                );
                ActivityOutcome {
                    success: false,
                    output: None,
                    error: Some(format!(
                        "HTTP provider not yet implemented: {} {}",
                        format_http_method(method),
                        url
                    )),
                    duration_ms: 0,
                }
            }
            Provider::Native {
                plugin,
                function,
                arguments,
            } => execute_native(plugin, function, arguments),
        }
    }
}

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
fn execute_native(
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
        _ => Err(format!("unknown native plugin: {plugin}")),
    }
}

/// Helper: extract a string argument or return an error.
fn arg_str<'a>(
    args: &'a std::collections::HashMap<String, serde_json::Value>,
    key: &str,
) -> Result<&'a str, String> {
    args.get(key)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("missing or invalid argument: {key}"))
}

/// Helper: extract an optional numeric argument, converting via `i64`.
fn arg_num<T: TryFrom<i64>>(
    args: &std::collections::HashMap<String, serde_json::Value>,
    key: &str,
) -> Option<T> {
    args.get(key)?.as_i64()?.try_into().ok()
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

fn format_http_method(method: &HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
        HttpMethod::Put => "PUT",
        HttpMethod::Delete => "DELETE",
        HttpMethod::Patch => "PATCH",
    }
}

/// Execute an external process with optional timeout, using async I/O when a
/// Tokio runtime is available or falling back to `std::process::Command`.
///
/// # Panics
///
/// Panics if a Tokio runtime is present but it uses the `current_thread`
/// scheduler. `tokio::task::block_in_place` requires the `multi_thread`
/// scheduler and will panic otherwise.
fn execute_process(
    path: &str,
    arguments: &[String],
    env: &std::collections::HashMap<String, String>,
    timeout_s: Option<&f64>,
) -> ActivityOutcome {
    // Background activities run on std::thread::scope threads without a Tokio
    // runtime.  Detect this and fall back to std::process::Command.
    if tokio::runtime::Handle::try_current().is_err() {
        return execute_process_sync(path, arguments, env, timeout_s);
    }

    let start = std::time::Instant::now();

    let path = path.to_string();
    let arguments = arguments.to_vec();
    let env = env.clone();
    let timeout_dur = timeout_s.map(|s| std::time::Duration::from_secs_f64(*s));

    tokio::task::block_in_place(move || {
        tokio::runtime::Handle::current().block_on(async {
            let mut cmd = tokio::process::Command::new(&path);
            cmd.args(&arguments);
            cmd.stdout(std::process::Stdio::piped());
            cmd.stderr(std::process::Stdio::piped());
            for (k, v) in &env {
                cmd.env(k, v);
            }

            let mut child = match cmd.spawn() {
                Ok(child) => child,
                Err(e) => {
                    return ActivityOutcome {
                        success: false,
                        output: None,
                        error: Some(format!("failed to execute '{path}': {e}")),
                        // u128 → u64: elapsed ms; truncation only possible after ~584M years.
                        #[allow(clippy::cast_possible_truncation)]
                        duration_ms: start.elapsed().as_millis() as u64,
                    };
                }
            };

            let result = if let Some(dur) = timeout_dur {
                match tokio::time::timeout(dur, child.wait()).await {
                    Ok(Ok(status)) => {
                        let stdout = {
                            let mut buf = Vec::new();
                            if let Some(mut out) = child.stdout.take() {
                                use tokio::io::AsyncReadExt;
                                let _ = out.read_to_end(&mut buf).await;
                            }
                            buf
                        };
                        let stderr = {
                            let mut buf = Vec::new();
                            if let Some(mut err) = child.stderr.take() {
                                use tokio::io::AsyncReadExt;
                                let _ = err.read_to_end(&mut buf).await;
                            }
                            buf
                        };
                        Ok(std::process::Output {
                            status,
                            stdout,
                            stderr,
                        })
                    }
                    Ok(Err(e)) => Err(e.to_string()),
                    Err(_elapsed) => {
                        let _ = child.kill().await;
                        Err("timed out".to_string())
                    }
                }
            } else {
                child.wait_with_output().await.map_err(|e| e.to_string())
            };

            // u128 → u64: elapsed ms; truncation only possible after ~584M years.
            #[allow(clippy::cast_possible_truncation)]
            let duration_ms = start.elapsed().as_millis() as u64;

            match result {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

                    ActivityOutcome {
                        success: output.status.success(),
                        output: if stdout.is_empty() {
                            None
                        } else {
                            Some(stdout)
                        },
                        error: if stderr.is_empty() {
                            None
                        } else {
                            Some(stderr)
                        },
                        duration_ms,
                    }
                }
                Err(reason) => ActivityOutcome {
                    success: false,
                    output: None,
                    error: Some(format!("process '{path}' {reason}")),
                    duration_ms,
                },
            }
        })
    })
}

/// Synchronous process execution for background threads (no Tokio runtime).
fn execute_process_sync(
    path: &str,
    arguments: &[String],
    env: &std::collections::HashMap<String, String>,
    timeout_s: Option<&f64>,
) -> ActivityOutcome {
    let start = std::time::Instant::now();

    let mut cmd = std::process::Command::new(path);
    cmd.args(arguments);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            return ActivityOutcome {
                success: false,
                output: None,
                error: Some(format!("failed to execute '{path}': {e}")),
                #[allow(clippy::cast_possible_truncation)]
                duration_ms: start.elapsed().as_millis() as u64,
            };
        }
    };

    let result = if let Some(&secs) = timeout_s {
        let dur = std::time::Duration::from_secs_f64(secs);
        let deadline = std::time::Instant::now() + dur;
        const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);
        loop {
            match child.try_wait() {
                Ok(Some(_status)) => break child.wait_with_output().map_err(|e| e.to_string()),
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        // Timeout — kill and reap the child so it doesn't keep
                        // running (or become a zombie) after we return.
                        let _ = child.kill();
                        let _ = child.wait();
                        break Err(format!("process '{path}' timed out"));
                    }
                    std::thread::sleep(
                        POLL_INTERVAL
                            .min(deadline.saturating_duration_since(std::time::Instant::now())),
                    );
                }
                Err(e) => break Err(e.to_string()),
            }
        }
    } else {
        child.wait_with_output().map_err(|e| e.to_string())
    };

    #[allow(clippy::cast_possible_truncation)]
    let duration_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let success = output.status.success();
            ActivityOutcome {
                success,
                output: if stdout.is_empty() {
                    None
                } else {
                    Some(stdout)
                },
                error: if success {
                    if stderr.is_empty() {
                        None
                    } else {
                        Some(stderr)
                    }
                } else {
                    Some(if stderr.is_empty() {
                        format!("process '{path}' exited with {}", output.status)
                    } else {
                        stderr
                    })
                },
                duration_ms,
            }
        }
        Err(reason) => ActivityOutcome {
            success: false,
            output: None,
            error: Some(reason),
            duration_ms,
        },
    }
}
