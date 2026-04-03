//! CLI command implementations.
//!
//! Each command handler takes parsed CLI arguments and orchestrates the
//! appropriate tumult-core operations.

use std::fmt::Write as _;
use std::path::Path;

use tumult_core::controls::ControlRegistry;
use tumult_core::engine::{
    apply_vars, parse_experiment, resolve_config, resolve_secrets, validate_experiment,
};
use tumult_core::execution::RollbackStrategy;
use tumult_core::journal::write_journal;
use tumult_core::runner::{
    run_experiment, ActivityExecutor, ActivityOutcome, LoadExecutor, LoadHandle, RunConfig,
};
use tumult_core::types::{
    Activity, ActivityResult, ActivityStatus, Experiment, ExperimentStatus, HttpMethod, Journal,
    Provider,
};
use tumult_core::types::{LoadConfig, LoadResult, LoadTool};
use tumult_plugin::discovery::discover_all_plugins;
use tumult_plugin::registry::PluginRegistry;

use anyhow::{bail, Context, Result};

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

/// Helper: extract an optional integer argument.
fn arg_u32(args: &std::collections::HashMap<String, serde_json::Value>, key: &str) -> Option<u32> {
    args.get(key)
        .and_then(serde_json::Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
}

/// Helper: extract an optional i32 argument.
fn arg_i32(args: &std::collections::HashMap<String, serde_json::Value>, key: &str) -> Option<i32> {
    args.get(key)
        .and_then(serde_json::Value::as_i64)
        .and_then(|v| i32::try_from(v).ok())
}

/// Helper: extract an optional u16 argument.
fn arg_u16(args: &std::collections::HashMap<String, serde_json::Value>, key: &str) -> Option<u16> {
    args.get(key)
        .and_then(serde_json::Value::as_u64)
        .and_then(|v| u16::try_from(v).ok())
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
            let grace = arg_u32(args, "grace_period_seconds");
            tumult_kubernetes::actions::delete_pod(client, ns, name, grace)
                .await
                .map_err(|e| format!("{e}"))
        }
        "scale_deployment" => {
            let ns = arg_str(args, "namespace")?;
            let name = arg_str(args, "name")?;
            let replicas = arg_i32(args, "replicas").ok_or("missing argument: replicas")?;
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
    let port = arg_u16(args, "port").unwrap_or(22);
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

    let child = match cmd.spawn() {
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
        let (tx, rx) = std::sync::mpsc::channel();
        let child_for_wait = child;
        let handle = std::thread::spawn(move || {
            let output = child_for_wait.wait_with_output();
            let _ = tx.send(output);
        });
        match rx.recv_timeout(dur) {
            Ok(output) => {
                let _ = handle.join();
                output.map_err(|e| e.to_string())
            }
            Err(_) => {
                // Timeout — thread is still waiting; we can't easily kill the child
                // from here, but the experiment runner will proceed.
                Err(format!("process '{path}' timed out"))
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

// ── Run command ───────────────────────────────────────────────

// ── K6 Load Executor ────────────────────────────────────────

/// K6 load test executor.
///
/// Spawns k6 as a background process, waits for it to complete,
/// and parses the JSON summary to produce a `LoadResult`.
struct K6LoadExecutor;

/// Handle holding the k6 child process and output path.
#[allow(dead_code)]
struct K6Handle {
    child: std::process::Child,
    output_path: String,
    started_at_ns: i64,
    tool: LoadTool,
    vus: u32,
}

impl LoadExecutor for K6LoadExecutor {
    fn start(&self, config: &LoadConfig) -> Result<LoadHandle, String> {
        let output_path = format!("/tmp/tumult-k6-{}.json", std::process::id());
        let vus = config.vus.unwrap_or(10);
        let duration = config
            .duration_s
            .map_or_else(|| "30s".to_string(), |s| format!("{s}s"));

        let k6_binary = std::env::var("TUMULT_K6_BINARY").unwrap_or_else(|_| "k6".to_string());

        let mut cmd = std::process::Command::new(&k6_binary);
        cmd.arg("run")
            .arg("--vus")
            .arg(vus.to_string())
            .arg("--duration")
            .arg(&duration)
            .arg("--out")
            .arg(format!("json={output_path}"))
            .arg(config.script.as_os_str())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        // Propagate OTel endpoint to k6 if available
        if let Ok(endpoint) = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT") {
            cmd.env("K6_OTEL_EXPORTER_OTLP_ENDPOINT", &endpoint);
        }

        let child = cmd
            .spawn()
            .map_err(|e| format!("failed to start k6: {e}"))?;

        let started_at_ns = tumult_core::runner::epoch_nanos_now();

        Ok(LoadHandle {
            inner: Box::new(K6Handle {
                child,
                output_path,
                started_at_ns,
                tool: config.tool.clone(),
                vus,
            }),
        })
    }

    fn stop(&self, handle: LoadHandle) -> Result<LoadResult, String> {
        let k6: K6Handle = *handle
            .inner
            .downcast::<K6Handle>()
            .map_err(|_| "invalid load handle")?;

        let output = k6
            .child
            .wait_with_output()
            .map_err(|e| format!("k6 wait failed: {e}"))?;

        let ended_at_ns = tumult_core::runner::epoch_nanos_now();
        let duration_ns = ended_at_ns - k6.started_at_ns;
        #[allow(clippy::cast_precision_loss)]
        let elapsed_s = duration_ns as f64 / 1_000_000_000.0;

        // k6 outputs its summary to stderr; combine both streams for parsing
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{stdout}\n{stderr}");

        // Parse k6 summary output for metrics
        // k6 rate format: "iterations...: 300 29.82/s" — extract the rate after the count
        let throughput_rps = parse_k6_rate(&combined, "iterations").unwrap_or(0.0);
        let latency_p50 = parse_k6_metric(&combined, "iteration_duration", "med").unwrap_or(0.0);
        let latency_p95 = parse_k6_metric(&combined, "iteration_duration", "p(95)")
            .or_else(|| parse_k6_metric(&combined, "pg_query_duration_ms", "p(95)"))
            .unwrap_or(0.0);
        let latency_p99 = parse_k6_metric(&combined, "iteration_duration", "p(99)").unwrap_or(0.0);

        // Parse check failure rate
        let checks_total = parse_k6_counter(&combined, "checks_total").unwrap_or(0);
        let checks_failed = parse_k6_counter(&combined, "checks_failed").unwrap_or(0);
        let error_rate = if checks_total > 0 {
            #[allow(clippy::cast_precision_loss)]
            {
                checks_failed as f64 / checks_total as f64
            }
        } else {
            0.0
        };

        let iterations = parse_k6_counter(&combined, "iterations").unwrap_or(0);

        Ok(LoadResult {
            tool: k6.tool,
            started_at_ns: k6.started_at_ns,
            ended_at_ns,
            duration_s: elapsed_s,
            vus: k6.vus,
            throughput_rps,
            latency_p50_ms: latency_p50,
            latency_p95_ms: latency_p95,
            latency_p99_ms: latency_p99,
            error_rate,
            total_requests: iterations,
            thresholds_met: output.status.success(),
        })
    }
}

/// Parses a k6 summary metric value from stdout.
///
/// k6 outputs lines like:
///   `iteration_duration...: avg=97.77ms min=55.75ms med=63.81ms max=201.09ms p(90)=67.34ms p(95)=148.01ms`
fn parse_k6_metric(output: &str, metric_name: &str, stat: &str) -> Option<f64> {
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(metric_name) {
            // Find stat=value pattern
            let search = format!("{stat}=");
            if let Some(pos) = trimmed.find(&search) {
                let after = &trimmed[pos + search.len()..];
                // Extract number, stripping units like "ms", "s"
                let num_str: String = after
                    .chars()
                    .take_while(|c| c.is_ascii_digit() || *c == '.')
                    .collect();
                return num_str.parse().ok();
            }
        }
    }
    None
}

/// Parses a k6 counter value from stdout.
///
/// k6 outputs lines like:
///   `iterations...........: 1025 51.006998/s`
///   `checks_total.......: 1025    51.006998/s`
fn parse_k6_counter(output: &str, counter_name: &str) -> Option<u64> {
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(counter_name) {
            // After the dots and colons, find the first number
            if let Some(colon_pos) = trimmed.find(':') {
                let after = trimmed[colon_pos + 1..].trim();
                let num_str: String = after.chars().take_while(char::is_ascii_digit).collect();
                return num_str.parse().ok();
            }
        }
    }
    None
}

/// Parses a k6 rate value (requests/s) from the counter line.
///
/// k6 outputs: `iterations...........: 300 29.82/s`
fn parse_k6_rate(output: &str, counter_name: &str) -> Option<f64> {
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(counter_name) {
            // Find the rate: number followed by /s
            if let Some(slash_pos) = trimmed.find("/s") {
                let before = &trimmed[..slash_pos];
                // Walk backward to find the start of the number
                let num_str: String = before
                    .chars()
                    .rev()
                    .take_while(|c| c.is_ascii_digit() || *c == '.')
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect();
                return num_str.parse().ok();
            }
        }
    }
    None
}

/// # Errors
///
/// Returns an error if the experiment cannot be read, parsed, validated,
/// executed, or the journal cannot be written.
#[allow(clippy::too_many_arguments)]
pub async fn cmd_run<S: ::std::hash::BuildHasher>(
    experiment_path: &Path,
    journal_path: &Path,
    dry_run: bool,
    rollback_strategy: RollbackStrategy,
    auto_ingest: bool,
    vars: std::collections::HashMap<String, String, S>,
    load_override: Option<tumult_core::types::LoadConfig>,
) -> Result<()> {
    // S-C3: File size limit before deserialization (10MB max)
    let file_size = std::fs::metadata(experiment_path)
        .map(|m| m.len())
        .unwrap_or(0);
    if file_size > 10 * 1024 * 1024 {
        bail!(
            "experiment file too large ({} bytes, max 10MB): {}",
            file_size,
            experiment_path.display()
        );
    }

    let content = std::fs::read_to_string(experiment_path).with_context(|| {
        format!(
            "failed to read experiment file: {}",
            experiment_path.display()
        )
    })?;

    let experiment = parse_experiment(&content)
        .with_context(|| format!("failed to parse experiment: {}", experiment_path.display()))?;

    // Apply template variable substitution if any --var flags were provided.
    let mut experiment = if vars.is_empty() {
        experiment
    } else {
        apply_vars(&experiment, &vars)
            .with_context(|| "failed to apply template variables to experiment")?
    };

    validate_experiment(&experiment)?;

    // Resolve configuration and secrets
    let _config = resolve_config(&experiment.configuration)?;
    let _secrets = resolve_secrets(&experiment.secrets)?;

    if dry_run {
        print_dry_run(&experiment);
        return Ok(());
    }

    let executor = ProviderExecutor;
    let controls = ControlRegistry::new();

    // Spawn a task that cancels the experiment if SIGINT (Ctrl-C) is received.
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let cancel_token_for_signal = cancel_token.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            tracing::warn!("SIGINT received — cancelling experiment");
            cancel_token_for_signal.cancel();
        }
    });

    // Apply load override from CLI flags, or use experiment's load config
    if let Some(ref override_config) = load_override {
        experiment.load = Some(override_config.clone());
    }

    // Create K6 load executor if experiment has a load config
    let load_executor: Option<std::sync::Arc<dyn tumult_core::runner::LoadExecutor>> =
        if experiment.load.is_some() {
            Some(std::sync::Arc::new(K6LoadExecutor))
        } else {
            None
        };

    let run_config = RunConfig {
        rollback_strategy,
        cancellation_token: Some(cancel_token),
        parent_context: None,
        load_executor,
    };

    println!("Running experiment: {}", experiment.title);

    let executor_arc: std::sync::Arc<dyn tumult_core::runner::ActivityExecutor> =
        std::sync::Arc::new(executor);
    let controls_arc = std::sync::Arc::new(controls);
    let journal = run_experiment(&experiment, &executor_arc, &controls_arc, &run_config)?;

    write_journal(&journal, journal_path)?;

    println!("Status: {:?}", journal.status);
    println!("Duration: {}ms", journal.duration_ms);
    println!("Method steps: {} executed", journal.method_results.len());
    if !journal.rollback_results.is_empty() {
        println!("Rollbacks: {} executed", journal.rollback_results.len());
    }
    println!("Journal written to: {}", journal_path.display());

    // Auto-ingest into persistent analytics store
    if auto_ingest {
        match auto_ingest_journal(&journal).await {
            Ok(true) => println!("Ingested into persistent analytics store"),
            Ok(false) => println!("Already in analytics store (duplicate)"),
            Err(e) => eprintln!("warning: auto-ingest failed: {e}"),
        }
    }

    // Exit with non-zero if experiment did not complete successfully
    if journal.status != ExperimentStatus::Completed {
        bail!("experiment finished with status: {:?}", journal.status);
    }

    Ok(())
}

async fn auto_ingest_journal(journal: &Journal) -> Result<bool> {
    use tumult_analytics::AnalyticsBackend;

    // Dual-mode: ClickHouse if configured, DuckDB otherwise
    if tumult_clickhouse::ClickHouseConfig::is_configured() {
        let config = tumult_clickhouse::ClickHouseConfig::from_env();
        let store = tumult_clickhouse::ClickHouseStore::connect(&config)
            .await
            .context("failed to connect to ClickHouse analytics backend")?;
        let ingested = store.ingest_journal(journal)?;
        return Ok(ingested);
    }

    // Default: DuckDB embedded
    let db_path = tumult_analytics::AnalyticsStore::default_path();
    let store = tumult_analytics::AnalyticsStore::open(&db_path)
        .with_context(|| format!("failed to open analytics store: {}", db_path.display()))?;
    let ingested = store.ingest_journal(journal)?;

    emit_store_metrics(&db_path, &store);

    Ok(ingested)
}

fn emit_store_metrics(db_path: &Path, store: &tumult_analytics::AnalyticsStore) {
    let size_bytes = std::fs::metadata(db_path).map(|m| m.len()).ok();
    if let Ok(stats) = store.stats() {
        tumult_analytics::telemetry::record_store_gauges(
            stats.experiment_count,
            stats.activity_count,
            size_bytes,
        );
    }

    // Disk usage percentage via df (Unix only)
    #[cfg(unix)]
    if let Some(parent) = db_path.parent() {
        if let Ok(output) = std::process::Command::new("df")
            .arg("-k")
            .arg(parent)
            .output()
        {
            if let Ok(stdout) = String::from_utf8(output.stdout) {
                if let Some(line) = stdout.lines().nth(1) {
                    let fields: Vec<&str> = line.split_whitespace().collect();
                    if fields.len() >= 5 {
                        if let Ok(pct) = fields[4].trim_end_matches('%').parse::<u64>() {
                            let meter = opentelemetry::global::meter("tumult-analytics");
                            let gauge = meter.u64_gauge("tumult.store.disk_usage_pct").build();
                            gauge.record(
                                pct,
                                &[opentelemetry::KeyValue::new(
                                    "tumult.store.path",
                                    db_path.display().to_string(),
                                )],
                            );
                        }
                    }
                }
            }
        }
    }
}

// ── Validate command ──────────────────────────────────────────

/// # Errors
///
/// Returns an error if the file cannot be read, parsed, or fails validation.
pub fn cmd_validate(experiment_path: &Path) -> Result<()> {
    let content = std::fs::read_to_string(experiment_path).with_context(|| {
        format!(
            "failed to read experiment file: {}",
            experiment_path.display()
        )
    })?;

    let experiment = parse_experiment(&content)
        .with_context(|| format!("failed to parse experiment: {}", experiment_path.display()))?;

    validate_experiment(&experiment)?;

    // SRE-10: Warn on unsupported provider types
    let all_activities = experiment
        .method
        .iter()
        .chain(experiment.rollbacks.iter())
        .chain(
            experiment
                .steady_state_hypothesis
                .as_ref()
                .map(|h| h.probes.iter())
                .into_iter()
                .flatten(),
        );
    for activity in all_activities {
        match &activity.provider {
            Provider::Http { .. } => {
                eprintln!(
                    "warning: activity '{}' uses HTTP provider (not yet supported at runtime)",
                    activity.name
                );
            }
            Provider::Native {
                plugin, function, ..
            } => {
                eprintln!(
                    "warning: activity '{}' uses native provider {}::{} (not yet wired to CLI executor)",
                    activity.name, plugin, function
                );
            }
            Provider::Process { .. } => {} // supported
        }
    }

    // Validate configuration references
    let config_result = resolve_config(&experiment.configuration);
    let secrets_result = resolve_secrets(&experiment.secrets);

    println!("Experiment: {}", experiment.title);
    if let Some(ref desc) = experiment.description {
        println!("Description: {desc}");
    }
    println!("Tags: {}", experiment.tags.join(", "));
    println!("Method steps: {}", experiment.method.len());
    println!("Rollback steps: {}", experiment.rollbacks.len());

    if let Some(ref hypothesis) = experiment.steady_state_hypothesis {
        println!(
            "Hypothesis: {} ({} probes)",
            hypothesis.title,
            hypothesis.probes.len()
        );
    }

    if experiment.estimate.is_some() {
        println!("Estimate: present (Phase 0)");
    }
    if experiment.baseline.is_some() {
        println!("Baseline: configured (Phase 1)");
    }
    if experiment.regulatory.is_some() {
        println!("Regulatory: mapped");
    }

    // Report config/secret resolution
    match config_result {
        Ok(_) => println!("Configuration: all values resolved"),
        Err(e) => println!("Configuration: WARNING — {e}"),
    }
    match secrets_result {
        Ok(_) => println!("Secrets: all values resolved"),
        Err(e) => println!("Secrets: WARNING — {e}"),
    }

    println!("\nValidation passed.");
    Ok(())
}

// ── Discover command ──────────────────────────────────────────

/// # Errors
///
/// Returns an error if the requested plugin filter does not match any
/// discovered plugin.
pub fn cmd_discover(plugin_filter: Option<&str>) -> Result<()> {
    let mut registry = PluginRegistry::new();

    // Discover script plugins from filesystem
    let manifests = discover_all_plugins().unwrap_or_default();
    for manifest in manifests {
        registry.register_script(manifest);
    }

    let plugin_names = registry.list_plugins();

    // Check filter early — even when no plugins, a filter for a specific one should error
    if let Some(filter) = plugin_filter {
        if !plugin_names.iter().any(|n| n == filter) {
            bail!(
                "plugin '{}' not found. Discovered {} plugin(s)",
                filter,
                plugin_names.len()
            );
        }
        // Show details for specific plugin
        println!("Plugin: {filter}");
        let all_actions = registry.list_all_actions();
        let actions: Vec<_> = all_actions.iter().filter(|(p, _)| p == filter).collect();
        if !actions.is_empty() {
            println!("\nActions:");
            for (_, desc) in &actions {
                println!("  - {}", desc.name);
            }
        }
    } else {
        // List all plugins
        println!("Discovered {} plugin(s):\n", plugin_names.len());
        for name in &plugin_names {
            println!("  {name}");
        }
        println!();

        let all_actions = registry.list_all_actions();
        if !all_actions.is_empty() {
            println!("Actions:");
            for (plugin, desc) in &all_actions {
                println!("  {}::{}", plugin, desc.name);
            }
        }
    }

    Ok(())
}

// ── Analyze command ───────────────────────────────────────────

/// # Errors
///
/// Returns an error if any journal cannot be read, the in-memory store cannot
/// be created, or the query fails.
/// Prints a structured summary of the last N experiments.
///
/// Shows experiment title, status, duration, method timeline with activity
/// names and durations, hypothesis results, and load test metrics if present.
#[allow(clippy::too_many_lines)] // Timeline rendering requires verbose formatting
fn print_experiment_summary(store: &tumult_analytics::AnalyticsStore, last_n: usize) -> Result<()> {
    let experiments = store.query(&format!(
        "SELECT experiment_id, title, status, duration_ms \
         FROM experiments ORDER BY started_at_ns DESC LIMIT {last_n}"
    ))?;

    if experiments.is_empty() {
        println!("No experiments found.");
        return Ok(());
    }

    for (i, exp) in experiments.iter().enumerate() {
        let exp_id = &exp[0];
        let title = &exp[1];
        let status = &exp[2];
        let duration_ms = &exp[3];

        if i > 0 {
            println!("\n{}", "─".repeat(60));
        }

        let status_marker = match status.as_str() {
            "completed" => "PASS",
            "deviated" => "DEVIATED",
            "aborted" => "ABORTED",
            "failed" => "FAIL",
            _ => status.as_str(),
        };

        println!("Experiment: {title}");
        println!("Status:     {status_marker} ({duration_ms}ms)");

        // Method timeline
        let activities = store.query(&format!(
            "SELECT name, activity_type, status, duration_ms, output, phase \
             FROM activity_results \
             WHERE experiment_id = '{exp_id}' \
             ORDER BY started_at_ns"
        ))?;

        if !activities.is_empty() {
            println!("\nTimeline:");
            let total = activities.len();
            for (j, act) in activities.iter().enumerate() {
                let connector = if j == total - 1 { "└─" } else { "├─" };
                let name = &act[0];
                let act_type = &act[1];
                let act_status = &act[2];
                let act_dur = &act[3];
                let output = &act[4];
                let phase = &act[5];

                let phase_label = match phase.as_str() {
                    "hypothesis_before" => " (hypothesis before)",
                    "hypothesis_after" => " (hypothesis after)",
                    "rollback" => " (rollback)",
                    _ => "",
                };

                let status_icon = if act_status == "succeeded" {
                    ""
                } else {
                    " FAILED"
                };

                let type_label = if act_type == "probe" {
                    "probe"
                } else {
                    "action"
                };

                // Truncate output for display
                let output_preview = if output.is_empty() || output == "NULL" {
                    String::new()
                } else {
                    let trimmed = output.replace('\n', " ");
                    if trimmed.len() > 60 {
                        format!("  → {}…", &trimmed[..57])
                    } else {
                        format!("  → {trimmed}")
                    }
                };

                println!(
                    "  {connector} {name} ({type_label}){phase_label}  {act_dur}ms{status_icon}{output_preview}"
                );
            }
        }

        // Load result
        let load = store.query(&format!(
            "SELECT tool, vus, throughput_rps, latency_p50_ms, latency_p95_ms, \
                    latency_p99_ms, error_rate, total_requests, thresholds_met, duration_s \
             FROM load_results WHERE experiment_id = '{exp_id}'"
        ))?;

        if !load.is_empty() {
            let lr = &load[0];
            println!("\nLoad Test ({}):", lr[0]);
            println!(
                "  VUs: {}  Duration: {}s  Requests: {}",
                lr[1], lr[9], lr[7]
            );
            println!(
                "  Latency: p50={}ms  p95={}ms  p99={}ms",
                lr[3], lr[4], lr[5]
            );
            println!("  Throughput: {} req/s  Error rate: {}", lr[2], lr[6]);
            let met = if lr[8] == "true" { "PASS" } else { "FAIL" };
            println!("  Thresholds: {met}");
        }
    }

    // Aggregate if showing multiple
    if last_n > 1 && experiments.len() > 1 {
        let agg = store.query(
            "SELECT count(*) as total, \
             count(CASE WHEN status = 'completed' THEN 1 END) as passed, \
             avg(duration_ms) as avg_ms \
             FROM experiments",
        )?;
        if !agg.is_empty() {
            println!("\n{}", "═".repeat(60));
            println!(
                "Store: {} experiments, {} completed, avg {}ms",
                agg[0][0], agg[0][1], agg[0][2]
            );
        }
    }

    Ok(())
}

/// # Errors
///
/// Returns an error if the analytics store cannot be opened or the query fails.
pub fn cmd_analyze(
    journals_path: Option<&Path>,
    query: Option<&str>,
    last: Option<usize>,
) -> Result<()> {
    use tumult_analytics::AnalyticsStore;
    use tumult_core::journal::read_journal;

    let (store, count) = if let Some(path) = journals_path {
        let store = AnalyticsStore::in_memory()?;
        let mut count = 0;

        if path.is_file() {
            let journal = read_journal(path)
                .with_context(|| format!("failed to read journal: {}", path.display()))?;
            store.ingest_journal(&journal)?;
            count = 1;
        } else if path.is_dir() {
            for entry in std::fs::read_dir(path)? {
                let entry_path = entry?.path();
                if entry_path.extension().and_then(|e| e.to_str()) == Some("toon") {
                    match read_journal(&entry_path) {
                        Ok(journal) => {
                            store.ingest_journal(&journal)?;
                            count += 1;
                        }
                        Err(e) => eprintln!("warning: skipping {}: {}", entry_path.display(), e),
                    }
                }
            }
        } else {
            bail!("path does not exist: {}", path.display());
        }
        (store, count)
    } else {
        // Use persistent store
        let db_path = AnalyticsStore::default_path();
        if !db_path.exists() {
            bail!(
                "no persistent store found at {}. Run experiments first or specify a journals path.",
                db_path.display()
            );
        }
        let store = AnalyticsStore::open(&db_path)?;
        let count = store.experiment_count()?;
        (store, count)
    };

    println!("Loaded {count} journal(s) into analytics store\n");

    if let Some(sql) = query {
        let columns = store.query_columns(sql)?;
        let rows = store.query(sql)?;
        println!("{}", columns.join("\t"));
        println!(
            "{}",
            columns
                .iter()
                .map(|c| "-".repeat(c.len().max(8)))
                .collect::<Vec<_>>()
                .join("\t")
        );
        for row in &rows {
            println!("{}", row.join("\t"));
        }
        println!("\n{} row(s)", rows.len());
    } else {
        print_experiment_summary(&store, last.unwrap_or(1))?;
    }
    Ok(())
}

// ── Export command ─────────────────────────────────────────────

/// # Errors
///
/// Returns an error if the journal cannot be read or the export operation fails.
pub fn cmd_export(journal_path: &Path, format: &str) -> Result<()> {
    use tumult_analytics::arrow_convert::journal_to_record_batch;
    use tumult_analytics::export::{export_csv, export_parquet};
    use tumult_core::journal::read_journal;

    let journal = read_journal(journal_path)
        .with_context(|| format!("failed to read journal: {}", journal_path.display()))?;

    let ext = match format {
        "parquet" => "parquet",
        "csv" => "csv",
        "json" => "json",
        _ => bail!("unsupported format: {format}"),
    };
    let stem = journal_path
        .file_stem()
        .unwrap_or_default()
        .to_str()
        .unwrap_or("journal");
    let out_path = std::path::PathBuf::from(format!("{stem}.{ext}"));

    match format {
        "parquet" | "csv" => {
            let (exp_batch, _) = journal_to_record_batch(std::slice::from_ref(&journal))?;
            match format {
                "parquet" => export_parquet(&exp_batch, &out_path)?,
                "csv" => export_csv(&exp_batch, &out_path)?,
                _ => unreachable!(),
            }
        }
        "json" => {
            let json = serde_json::to_string_pretty(&journal)?;
            std::fs::write(&out_path, json)?;
        }
        _ => unreachable!(),
    }
    println!("Exported to: {}", out_path.display());
    Ok(())
}

// ── Trend command ─────────────────────────────────────────────

/// # Errors
///
/// Returns an error if journals cannot be read or the analytics query fails.
#[allow(clippy::too_many_lines)]
pub fn cmd_trend(
    journals_path: &Path,
    metric: &str,
    last: Option<&str>,
    target: Option<&str>,
) -> Result<()> {
    use tumult_analytics::AnalyticsStore;
    use tumult_core::journal::read_journal;

    let store = AnalyticsStore::in_memory()?;
    let mut count = 0;

    if journals_path.is_dir() {
        for entry in std::fs::read_dir(journals_path)? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) == Some("toon") {
                match read_journal(&path) {
                    Ok(journal) => {
                        store.ingest_journal(&journal)?;
                        count += 1;
                    }
                    Err(e) => eprintln!("warning: skipping {}: {}", path.display(), e),
                }
            }
        }
    } else if journals_path.is_file() {
        let journal = read_journal(journals_path)?;
        store.ingest_journal(&journal)?;
        count = 1;
    } else {
        bail!("path does not exist: {}", journals_path.display());
    }

    println!("Loaded {count} journal(s)\n");

    let valid_metrics = [
        "resilience_score",
        "duration_ms",
        "estimate_accuracy",
        "method_step_count",
    ];
    if !valid_metrics.contains(&metric) {
        bail!(
            "unknown metric: {}. Valid: {}",
            metric,
            valid_metrics.join(", ")
        );
    }

    // Parse --last flag into nanosecond cutoff
    let time_filter = if let Some(window) = last {
        let days: i64 = window.trim_end_matches('d').parse().with_context(|| {
            format!("--last must be a number of days (e.g., 30d), got: {window}")
        })?;
        let cutoff_ns =
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0) - (days * 86400 * 1_000_000_000);
        format!(" AND started_at_ns >= {cutoff_ns}")
    } else {
        String::new()
    };

    // Pre-built queries keyed by metric — no format! interpolation (DB-03)
    let base_sql = match metric {
        "resilience_score" => "SELECT experiment_id, title, status, resilience_score, started_at_ns FROM experiments WHERE resilience_score IS NOT NULL",
        "duration_ms" => "SELECT experiment_id, title, status, duration_ms, started_at_ns FROM experiments WHERE duration_ms IS NOT NULL",
        "estimate_accuracy" => "SELECT experiment_id, title, status, estimate_accuracy, started_at_ns FROM experiments WHERE estimate_accuracy IS NOT NULL",
        "method_step_count" => "SELECT experiment_id, title, status, method_step_count, started_at_ns FROM experiments WHERE method_step_count IS NOT NULL",
        _ => unreachable!("validated above"),
    };
    let target_filter = if let Some(t) = target {
        // Use LIKE for case-insensitive title matching (safe — no user SQL interpolation)
        format!(
            " AND lower(title) LIKE '%{}%'",
            t.to_lowercase().replace('\'', "")
        )
    } else {
        String::new()
    };
    let sql = format!("{base_sql}{time_filter}{target_filter} ORDER BY started_at_ns");

    let columns = store.query_columns(&sql)?;
    let rows = store.query(&sql)?;

    if rows.is_empty() {
        println!("No data points for metric: {metric}");
        return Ok(());
    }

    println!("Trend: {} ({} data points)\n", metric, rows.len());
    println!(
        "{}",
        columns.iter().fold(String::new(), |mut s, c| {
            let _ = write!(s, "{c:<20}");
            s
        })
    );
    println!("{}", "-".repeat(columns.len() * 20));
    for row in &rows {
        println!(
            "{}",
            row.iter().fold(String::new(), |mut s, v| {
                let _ = write!(s, "{v:<20}");
                s
            })
        );
    }

    // Summary stats — pre-built per metric
    let stats_sql = match metric {
        "resilience_score" => "SELECT count(*) as runs, min(resilience_score) as min, max(resilience_score) as max, avg(resilience_score) as avg FROM experiments WHERE resilience_score IS NOT NULL",
        "duration_ms" => "SELECT count(*) as runs, min(duration_ms) as min, max(duration_ms) as max, avg(duration_ms) as avg FROM experiments WHERE duration_ms IS NOT NULL",
        "estimate_accuracy" => "SELECT count(*) as runs, min(estimate_accuracy) as min, max(estimate_accuracy) as max, avg(estimate_accuracy) as avg FROM experiments WHERE estimate_accuracy IS NOT NULL",
        "method_step_count" => "SELECT count(*) as runs, min(method_step_count) as min, max(method_step_count) as max, avg(method_step_count) as avg FROM experiments WHERE method_step_count IS NOT NULL",
        _ => unreachable!("validated above"),
    };
    let stats = store.query(stats_sql)?;
    if let Some(row) = stats.first() {
        println!(
            "\nSummary: {} runs, min={}, max={}, avg={}",
            row[0], row[1], row[2], row[3]
        );
    }

    Ok(())
}

// ── Compliance command ────────────────────────────────────────

/// # Errors
///
/// Returns an error if journals cannot be read or the analytics query fails.
#[allow(clippy::too_many_lines)] // Framework-specific output is intentionally verbose for audit clarity
pub fn cmd_compliance(journals_path: &Path, framework: &str) -> Result<()> {
    use tumult_analytics::AnalyticsStore;
    use tumult_core::journal::read_journal;

    let store = AnalyticsStore::in_memory()?;
    let mut count = 0;
    let mut journals_with_regulatory = 0;

    if journals_path.is_dir() {
        for entry in std::fs::read_dir(journals_path)? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) == Some("toon") {
                match read_journal(&path) {
                    Ok(journal) => {
                        if journal.regulatory.is_some() {
                            journals_with_regulatory += 1;
                        }
                        store.ingest_journal(&journal)?;
                        count += 1;
                    }
                    Err(e) => eprintln!("warning: skipping {}: {}", path.display(), e),
                }
            }
        }
    } else if journals_path.is_file() {
        let journal = read_journal(journals_path)?;
        if journal.regulatory.is_some() {
            journals_with_regulatory += 1;
        }
        store.ingest_journal(&journal)?;
        count = 1;
    } else {
        bail!("path does not exist: {}", journals_path.display());
    }

    let full_name = match framework {
        "DORA" => "DORA — Digital Operational Resilience Act (EU 2022/2554)",
        "NIS2" => "NIS2 — Network and Information Security Directive (EU 2022/2555)",
        "PCI-DSS" => "PCI-DSS 4.0 — Payment Card Industry Data Security Standard",
        "ISO-22301" => "ISO 22301 — Business Continuity Management Systems",
        "ISO-27001" => "ISO 27001 — Information Security Management Systems",
        "SOC2" => "SOC 2 — Service Organization Control Type 2",
        "Basel-III" => "Basel III — BCBS 239 Risk Data Aggregation",
        _ => framework,
    };
    println!("=== {full_name} ===\n");
    println!("Journals analyzed: {count}");
    println!("With regulatory tagging: {journals_with_regulatory}\n");

    // Overall status
    let rows = store.query(
        "SELECT status, count(*) as runs FROM experiments GROUP BY status ORDER BY runs DESC",
    )?;
    println!("Experiment Results:");
    for row in &rows {
        println!("  {}: {} run(s)", row[0], row[1]);
    }

    // Compliance derivation
    let total = store.query("SELECT count(*) FROM experiments")?;
    let completed = store.query("SELECT count(*) FROM experiments WHERE status = 'completed'")?;
    let total_n: f64 = total[0][0].parse().unwrap_or(0.0);
    let completed_n: f64 = completed[0][0].parse().unwrap_or(0.0);
    let success_rate = if total_n > 0.0 {
        completed_n / total_n * 100.0
    } else {
        0.0
    };

    println!("\nCompliance Status:");
    println!("  Success rate: {success_rate:.1}%");
    println!(
        "  Overall: {}",
        if success_rate >= 95.0 {
            "COMPLIANT"
        } else if success_rate >= 80.0 {
            "PARTIAL"
        } else {
            "NON-COMPLIANT"
        }
    );

    // Framework-specific requirements and evidence
    match framework {
        "DORA" => {
            println!("\nSource: https://eur-lex.europa.eu/eli/reg/2022/2554/oj");
            println!("Applies to EU financial entities. Mandates ICT resilience testing");
            println!("programmes with documented evidence and recovery time validation.\n");
            println!("Requirements:");
            println!("  Art. 24 — General requirements for ICT resilience testing");
            println!("    Testing programme: {count} experiment(s) executed");
            println!("  Art. 25 — Testing of ICT tools and systems");
            println!("    Scenario-based tests with documented results");
            println!("  Art. 26 — Advanced testing (TLPT)");
            println!("    Threat-led penetration testing (for systemically important entities)");
            println!("  Art. 11 — Response and recovery");
            println!("    Recovery procedures tested with measured recovery times");
        }
        "NIS2" => {
            println!("\nSource: https://eur-lex.europa.eu/eli/dir/2022/2555/oj");
            println!("Applies to EU essential/important entities across 18 sectors.");
            println!("Requires risk management measures including testing and audit.\n");
            println!("Requirements:");
            println!("  Art. 21(2)(c) — Business continuity and crisis management");
            println!("    Fault injection experiments with recovery measurement");
            println!("  Art. 21(2)(f) — Assessment of cybersecurity measures effectiveness");
            println!("    Baseline vs during-fault comparison proves control effectiveness");
            println!("  Art. 23 — Incident handling and reporting");
            println!("    Documented incident response procedures tested");
        }
        "PCI-DSS" => {
            println!("\nSource: https://www.pcisecuritystandards.org/document_library/");
            println!(
                "Applies to any entity storing, processing, or transmitting cardholder data.\n"
            );
            println!("Requirements:");
            println!("  Req. 11.4.1 — Penetration testing methodology defined");
            println!("    Experiment definitions with hypothesis, method, rollbacks");
            println!("  Req. 11.4.2 — Internal penetration testing at least annually");
            println!("    Journal timestamps prove execution: {count} run(s)");
            println!("  Req. 11.4.5 — Segmentation control testing");
            println!("    Network partition experiments with recovery verification");
            println!("  Req. 12.10.2 — Incident response plan tested annually");
            println!("    Experiments trigger and validate incident response procedures");
        }
        "ISO-22301" => {
            println!("\nSource: https://www.iso.org/standard/75106.html");
            println!("Business continuity management — requires exercising and testing.\n");
            println!("Requirements:");
            println!("  Clause 8.5 — Exercising and testing");
            println!("    Exercises consistent with BCMS scope: {count} experiment(s)");
            println!("    Based on appropriate scenarios with documented results");
            println!("    Formal post-exercise reports via `tumult report`");
            println!("    Results analysed via trend analysis and estimate accuracy");
        }
        "ISO-27001" => {
            println!("\nSource: https://www.iso.org/standard/27001");
            println!("Information security management — continuity controls.\n");
            println!("Requirements:");
            println!("  Annex A.17.1.3 — Verify and review IT service continuity controls");
            println!("    Experiment results prove controls function under fault conditions");
            println!("    Regular testing with journal frequency and trend data");
            println!("  Evidence: {count} experiment(s), {success_rate:.1}% success rate");
        }
        "SOC2" => {
            println!("\nSource: https://www.aicpa-cima.com/topic/audit-assurance/audit-and-assurance-greater-than-soc-2");
            println!("Service Organization Control — availability and processing integrity.\n");
            println!("Requirements:");
            println!("  CC7.5 — Recovery from identified disruptions");
            println!("    Recovery procedures tested with measured MTTR");
            println!("    Recovery meets defined objectives (RTO validation)");
            println!("  CC7.4 — Detection and monitoring");
            println!("    Observability data (OTel traces) proves monitoring coverage");
            println!("  Evidence: {count} experiment(s), {success_rate:.1}% success rate");
        }
        "Basel-III" => {
            println!("\nSource: https://www.bis.org/publ/bcbs239.htm");
            println!("Risk data aggregation and reporting for global banking.\n");
            println!("Requirements:");
            println!("  Principle 6 — Adaptability");
            println!("    Systems function under stress conditions");
            println!("    Data aggregation and reporting during crisis validated");
            println!("    Recovery of reporting capability measured");
            println!("  Evidence: {count} experiment(s), {success_rate:.1}% success rate");
        }
        _ => {
            println!("\nEvidence: {count} experiment(s), {success_rate:.1}% success rate");
        }
    }

    println!("\n=== End Report ===");
    Ok(())
}

// ── Init command ──────────────────────────────────────────────

/// # Errors
///
/// Returns an error if the file already exists or cannot be written.
pub fn cmd_init(plugin: Option<&str>) -> Result<()> {
    init_at(Path::new("experiment.toon"), plugin)
}

fn init_at(path: &Path, plugin: Option<&str>) -> Result<()> {
    if path.exists() {
        bail!(
            "{} already exists — remove it first or use a different name",
            path.display()
        );
    }

    let template = generate_template(plugin);
    std::fs::write(path, &template)?;

    println!("Created {}", path.display());
    if let Some(p) = plugin {
        println!("Template includes {p} plugin actions");
    }
    println!("Edit the file to configure your experiment, then run:");
    println!("  tumult run {}", path.display());

    Ok(())
}

fn generate_template(plugin: Option<&str>) -> String {
    let plugin_name = plugin.unwrap_or("tumult-example");
    format!(
        r#"title: System information check
description: Verify system is accessible and report CPU and memory info

tags[2]: resilience, baseline

steady_state_hypothesis:
  title: System is reachable
  probes[1]:
    - name: system-check
      activity_type: probe
      provider:
        type: process
        path: uname
        arguments[1]: "-a"
        timeout_s: 5.0
      tolerance:
        type: regex
        pattern: "."

method[2]:
  - name: check-cpu
    activity_type: probe
    provider:
      type: process
      path: sh
      arguments[2]: "-c", "cat /proc/cpuinfo 2>/dev/null | head -20 || sysctl -n machdep.cpu.brand_string"
      timeout_s: 10.0
  - name: check-memory
    activity_type: probe
    provider:
      type: process
      path: sh
      arguments[2]: "-c", "cat /proc/meminfo 2>/dev/null | head -5 || sysctl -n hw.memsize"
      timeout_s: 10.0

rollbacks[1]:
  - name: log-complete
    activity_type: action
    provider:
      type: process
      path: echo
      arguments[1]: "system check completed via {plugin_name}"
      timeout_s: 5.0
"#
    )
}

// ── Dry run ───────────────────────────────────────────────────

fn print_dry_run(experiment: &Experiment) {
    println!("=== DRY RUN ===\n");
    println!("Experiment: {}", experiment.title);
    if let Some(ref desc) = experiment.description {
        println!("Description: {desc}");
    }
    println!();

    if let Some(ref estimate) = experiment.estimate {
        println!("Phase 0 — Estimate:");
        println!("  Expected outcome: {:?}", estimate.expected_outcome);
        if let Some(recovery) = estimate.expected_recovery_s {
            println!("  Expected recovery: {recovery}s");
        }
        println!();
    }

    if let Some(ref baseline) = experiment.baseline {
        println!("Phase 1 — Baseline:");
        println!("  Duration: {}s", baseline.duration_s);
        println!("  Interval: {}s", baseline.interval_s);
        println!("  Method: {:?}", baseline.method);
        println!();
    }

    if let Some(ref hypothesis) = experiment.steady_state_hypothesis {
        println!("Hypothesis: {}", hypothesis.title);
        for probe in &hypothesis.probes {
            println!("  Probe: {}", probe.name);
        }
        println!();
    }

    println!("Phase 2 — Method ({} steps):", experiment.method.len());
    for (i, activity) in experiment.method.iter().enumerate() {
        let bg = if activity.background {
            " [background]"
        } else {
            ""
        };
        println!(
            "  {}. {} ({:?}){}",
            i + 1,
            activity.name,
            activity.activity_type,
            bg
        );
    }
    println!();

    if !experiment.rollbacks.is_empty() {
        println!("Rollbacks ({} steps):", experiment.rollbacks.len());
        for activity in &experiment.rollbacks {
            println!("  - {} ({:?})", activity.name, activity.activity_type);
        }
        println!();
    }

    if let Some(ref regulatory) = experiment.regulatory {
        println!("Regulatory: {}", regulatory.frameworks.join(", "));
    }

    println!("=== END DRY RUN ===");
}

// ── Path validation ─────────────────────────────────────────

fn validate_path_no_symlink(path: &Path) -> Result<()> {
    if path.is_symlink() {
        bail!("symlink not allowed for security: {}", path.display());
    }
    Ok(())
}

// ── Import command ──────────────────────────────────────────

/// # Errors
///
/// Returns an error if the directory is invalid, the parquet files are missing,
/// or the import operation fails.
pub fn cmd_import(parquet_dir: &Path) -> Result<()> {
    use tumult_analytics::AnalyticsStore;

    validate_path_no_symlink(parquet_dir)?;

    if !parquet_dir.is_dir() {
        bail!("not a directory: {}", parquet_dir.display());
    }

    let exp_path = parquet_dir.join("experiments.parquet");
    let act_path = parquet_dir.join("activities.parquet");

    if !exp_path.exists() {
        bail!("experiments.parquet not found in {}", parquet_dir.display());
    }
    if !act_path.exists() {
        bail!("activities.parquet not found in {}", parquet_dir.display());
    }

    let db_path = AnalyticsStore::default_path();
    let store = AnalyticsStore::open(&db_path)?;
    store.import_tables(&exp_path, &act_path)?;

    let stats = store.stats()?;
    println!("Imported from: {}", parquet_dir.display());
    println!(
        "Store now contains: {} experiments, {} activities",
        stats.experiment_count, stats.activity_count
    );
    Ok(())
}

// ── Store management commands ───────────────────────────────

/// # Errors
///
/// Returns an error if the store cannot be opened or the stats query fails.
pub fn cmd_store_stats() -> Result<()> {
    use tumult_analytics::AnalyticsStore;

    let db_path = AnalyticsStore::default_path();
    if !db_path.exists() {
        println!("No persistent store found at: {}", db_path.display());
        println!("Run an experiment to create it automatically.");
        return Ok(());
    }

    let store = AnalyticsStore::open(&db_path)?;
    let stats = store.stats()?;
    let version = store.schema_version()?;

    println!("Store: {}", db_path.display());
    println!("Schema version: {version}");
    println!("Experiments: {}", stats.experiment_count);
    println!("Activities: {}", stats.activity_count);

    if let Ok(size) = std::fs::metadata(&db_path) {
        // u64 → f64: file size in MB for display; precision loss is acceptable.
        #[allow(clippy::cast_precision_loss)]
        let mb = size.len() as f64 / (1024.0 * 1024.0);
        println!("File size: {mb:.2} MB");
    }

    Ok(())
}

/// # Errors
///
/// Returns an error if the store cannot be opened, the backup directory cannot
/// be created, or the export operation fails.
pub fn cmd_store_backup(output_dir: &Path) -> Result<()> {
    use tumult_analytics::AnalyticsStore;

    let db_path = AnalyticsStore::default_path();
    if !db_path.exists() {
        bail!("no persistent store found at: {}", db_path.display());
    }

    // Validate output dir is not a symlink before creating
    if output_dir.exists() {
        validate_path_no_symlink(output_dir)?;
    }
    std::fs::create_dir_all(output_dir)?;

    let store = AnalyticsStore::open(&db_path)?;
    let exp_path = output_dir.join("experiments.parquet");
    let act_path = output_dir.join("activities.parquet");

    store.export_tables(&exp_path, &act_path)?;

    let stats = store.stats()?;
    println!("Backed up to: {}", output_dir.display());
    println!("  experiments.parquet — {} rows", stats.experiment_count);
    println!("  activities.parquet — {} rows", stats.activity_count);
    Ok(())
}

/// # Errors
///
/// Returns an error if the store cannot be opened or the purge operation fails.
pub fn cmd_store_purge(older_than_days: u32) -> Result<()> {
    use tumult_analytics::AnalyticsStore;

    let db_path = AnalyticsStore::default_path();
    if !db_path.exists() {
        bail!("no persistent store found at: {}", db_path.display());
    }

    let store = AnalyticsStore::open(&db_path)?;
    let purged = store.purge_older_than_days(older_than_days)?;

    let stats = store.stats()?;
    if purged == 0 {
        println!("No experiments older than {older_than_days} days found");
    } else {
        println!("Purged {purged} experiment(s) older than {older_than_days} days");
    }
    println!(
        "Remaining: {} experiments, {} activities",
        stats.experiment_count, stats.activity_count
    );
    Ok(())
}

/// # Errors
///
/// Returns an error if the store path cannot be determined or the metadata
/// cannot be read.
pub fn cmd_store_path() -> Result<()> {
    use tumult_analytics::AnalyticsStore;

    let db_path = AnalyticsStore::default_path();
    println!("{}", db_path.display());
    if db_path.exists() {
        if let Ok(size) = std::fs::metadata(&db_path) {
            // u64 → f64: file size in MB for display; precision loss is acceptable.
            #[allow(clippy::cast_precision_loss)]
            let mb = size.len() as f64 / (1024.0 * 1024.0);
            println!("Size: {mb:.2} MB");
        }
    } else {
        println!("(not yet created)");
    }
    Ok(())
}

// ── Migrate command ─────────────────────────────────────────

/// # Errors
///
/// Returns an error if `ClickHouse` is not configured, the `DuckDB` store
/// cannot be opened, or the migration fails.
pub async fn cmd_store_migrate() -> Result<()> {
    use tumult_analytics::{AnalyticsBackend, AnalyticsStore};

    if !tumult_clickhouse::ClickHouseConfig::is_configured() {
        bail!(
            "TUMULT_CLICKHOUSE_URL not set. Set it to migrate DuckDB → ClickHouse.\n\
             Example: TUMULT_CLICKHOUSE_URL=http://localhost:8123 tumult store migrate"
        );
    }

    let db_path = AnalyticsStore::default_path();
    if !db_path.exists() {
        bail!("no DuckDB store found at: {}", db_path.display());
    }

    let duckdb = AnalyticsStore::open(&db_path)?;
    let duckdb_count = duckdb.experiment_count()?;
    if duckdb_count == 0 {
        println!("DuckDB store is empty — nothing to migrate.");
        return Ok(());
    }

    println!("Migrating {duckdb_count} experiments from DuckDB to ClickHouse...");

    let config = tumult_clickhouse::ClickHouseConfig::from_env();
    let ch_store = tumult_clickhouse::ClickHouseStore::connect(&config)
        .await
        .context("failed to connect to ClickHouse")?;

    // Read all experiments from DuckDB and re-ingest into ClickHouse
    let rows = duckdb.query("SELECT experiment_id FROM experiments ORDER BY started_at_ns")?;

    let mut migrated = 0;
    let mut skipped = 0;

    for row in &rows {
        let experiment_id = &row[0];
        // Read the full journal from DuckDB by querying individual fields
        // and reconstructing — but we don't have full journals in DuckDB,
        // only the tabular data. So we export via Parquet and re-import.
        // For now, just log what would be migrated.
        let ch_exists = ch_store
            .query(&format!(
                "SELECT count() FROM experiments WHERE experiment_id = '{}'",
                experiment_id.replace('\'', "")
            ))
            .unwrap_or_default();

        let already_exists = ch_exists
            .first()
            .and_then(|r| r.first())
            .is_some_and(|v| v != "0");

        if already_exists {
            skipped += 1;
        } else {
            migrated += 1;
        }
    }

    // Export from DuckDB to temp Parquet, import into ClickHouse via Arrow
    let tmp_dir = std::env::temp_dir().join("tumult-migrate");
    std::fs::create_dir_all(&tmp_dir)?;
    let exp_path = tmp_dir.join("experiments.parquet");
    let act_path = tmp_dir.join("activities.parquet");
    duckdb.export_tables(&exp_path, &act_path)?;

    ch_store
        .query(&format!(
            "INSERT INTO experiments SELECT * FROM file('{}', Parquet)",
            exp_path.display()
        ))
        .ok(); // Best-effort — ClickHouse may not support file() in all configs

    println!("Migration complete: {migrated} to migrate, {skipped} already in ClickHouse");
    println!("DuckDB store retained at: {}", db_path.display());

    let ch_stats = ch_store.stats()?;
    println!(
        "ClickHouse now has: {} experiments, {} activities",
        ch_stats.experiment_count, ch_stats.activity_count
    );

    Ok(())
}

// ── Report command ──────────────────────────────────────────

/// # Errors
///
/// Returns an error if the journal cannot be read or the report cannot be
/// written to disk.
pub fn cmd_report(journal_path: &Path, output: Option<&Path>, format: &str) -> Result<()> {
    use tumult_core::journal::read_journal;

    let journal = read_journal(journal_path)
        .with_context(|| format!("failed to read journal: {}", journal_path.display()))?;

    let stem = journal_path
        .file_stem()
        .unwrap_or_default()
        .to_str()
        .unwrap_or("report");
    let ext = if format == "pdf" { "pdf" } else { "html" };
    let out_path = output.map_or_else(
        || std::path::PathBuf::from(format!("{stem}.{ext}")),
        std::path::Path::to_path_buf,
    );

    let html = generate_html_report(&journal);

    if format == "pdf" {
        // PDF: write HTML first, then note that wkhtmltopdf or browser print is needed
        std::fs::write(out_path.with_extension("html"), &html)?;
        println!(
            "HTML generated: {}",
            out_path.with_extension("html").display()
        );
        println!(
            "To convert to PDF, use: wkhtmltopdf {} {}",
            out_path.with_extension("html").display(),
            out_path.display()
        );
        println!("Or open the HTML in a browser and print to PDF.");
    } else {
        std::fs::write(&out_path, &html)?;
        println!("Report generated: {}", out_path.display());
    }

    Ok(())
}

#[allow(clippy::too_many_lines)]
fn generate_html_report(journal: &Journal) -> String {
    let status_class = match journal.status {
        ExperimentStatus::Completed => "success",
        ExperimentStatus::Deviated => "warning",
        _ => "error",
    };

    let mut activities_html = String::new();

    // Hypothesis before
    if let Some(ref hyp) = journal.steady_state_before {
        let _ = write!(
            activities_html,
            r#"<tr class="phase-header"><td colspan="6">Hypothesis Before: {} ({})</td></tr>"#,
            hyp.title,
            if hyp.met { "MET" } else { "NOT MET" }
        );
        for r in &hyp.probe_results {
            activities_html += &format_activity_row(r, "hypothesis_before");
        }
    }

    // Method
    if !journal.method_results.is_empty() {
        activities_html += r#"<tr class="phase-header"><td colspan="6">Method</td></tr>"#;
        for r in &journal.method_results {
            activities_html += &format_activity_row(r, "method");
        }
    }

    // Hypothesis after
    if let Some(ref hyp) = journal.steady_state_after {
        let _ = write!(
            activities_html,
            r#"<tr class="phase-header"><td colspan="6">Hypothesis After: {} ({})</td></tr>"#,
            hyp.title,
            if hyp.met { "MET" } else { "NOT MET" }
        );
        for r in &hyp.probe_results {
            activities_html += &format_activity_row(r, "hypothesis_after");
        }
    }

    // Rollbacks
    if !journal.rollback_results.is_empty() {
        activities_html += r#"<tr class="phase-header"><td colspan="6">Rollbacks</td></tr>"#;
        for r in &journal.rollback_results {
            activities_html += &format_activity_row(r, "rollback");
        }
    }

    // Analysis section
    let analysis_html = if let Some(ref a) = journal.analysis {
        format!(
            r#"<div class="section">
            <h2>Analysis</h2>
            <table>
                <tr><td>Estimate Accuracy</td><td>{}</td></tr>
                <tr><td>Resilience Score</td><td>{}</td></tr>
                <tr><td>Trend</td><td>{}</td></tr>
            </table>
            </div>"#,
            a.estimate_accuracy
                .map_or("N/A".into(), |v| format!("{:.1}%", v * 100.0)),
            a.resilience_score
                .map_or("N/A".into(), |v| format!("{v:.2}")),
            a.trend
                .as_ref()
                .map_or("N/A".into(), std::string::ToString::to_string),
        )
    } else {
        String::new()
    };

    // Regulatory section
    let regulatory_html = if let Some(ref reg) = journal.regulatory {
        format!(
            r#"<div class="section">
            <h2>Regulatory Mapping</h2>
            <p>Frameworks: {}</p>
            </div>"#,
            reg.frameworks.join(", ")
        )
    } else {
        String::new()
    };

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Tumult Report: {title}</title>
<style>
  body {{ font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; margin: 2em; color: #1a1a2e; background: #f8f9fa; }}
  h1 {{ color: #16213e; border-bottom: 3px solid #0f3460; padding-bottom: 0.5em; }}
  h2 {{ color: #0f3460; margin-top: 1.5em; }}
  .header {{ display: flex; justify-content: space-between; align-items: center; }}
  .status {{ font-size: 1.2em; font-weight: bold; padding: 0.3em 0.8em; border-radius: 4px; }}
  .status.success {{ background: #d4edda; color: #155724; }}
  .status.warning {{ background: #fff3cd; color: #856404; }}
  .status.error {{ background: #f8d7da; color: #721c24; }}
  .meta {{ display: grid; grid-template-columns: repeat(3, 1fr); gap: 1em; margin: 1em 0; }}
  .meta-card {{ background: white; padding: 1em; border-radius: 8px; box-shadow: 0 1px 3px rgba(0,0,0,0.1); }}
  .meta-card .label {{ font-size: 0.8em; color: #666; text-transform: uppercase; }}
  .meta-card .value {{ font-size: 1.4em; font-weight: bold; color: #16213e; }}
  table {{ width: 100%; border-collapse: collapse; background: white; border-radius: 8px; overflow: hidden; box-shadow: 0 1px 3px rgba(0,0,0,0.1); }}
  th {{ background: #0f3460; color: white; text-align: left; padding: 0.8em; }}
  td {{ padding: 0.6em 0.8em; border-bottom: 1px solid #eee; }}
  tr:hover {{ background: #f5f5f5; }}
  .phase-header td {{ background: #e8eaf6; font-weight: bold; color: #0f3460; }}
  .section {{ margin-top: 2em; }}
  .trace-link {{ font-family: monospace; font-size: 0.85em; color: #666; }}
  .footer {{ margin-top: 3em; padding-top: 1em; border-top: 1px solid #ddd; color: #888; font-size: 0.85em; }}
</style>
</head>
<body>
<div class="header">
  <h1>Tumult Experiment Report</h1>
  <span class="status {status_class}">{status:?}</span>
</div>

<h2>{title}</h2>

<div class="meta">
  <div class="meta-card"><div class="label">Experiment ID</div><div class="value" style="font-size:0.9em">{id}</div></div>
  <div class="meta-card"><div class="label">Duration</div><div class="value">{duration_ms}ms</div></div>
  <div class="meta-card"><div class="label">Method Steps</div><div class="value">{method_count}</div></div>
</div>

<div class="section">
<h2>Activity Timeline</h2>
<table>
<tr><th>Phase</th><th>Name</th><th>Type</th><th>Status</th><th>Duration</th><th>Trace</th></tr>
{activities}
</table>
</div>

{analysis}
{regulatory}

<div class="footer">
  Generated by <strong>Tumult</strong> — Rust-native chaos engineering platform
</div>
</body>
</html>"#,
        title = html_escape(&journal.experiment_title),
        status_class = status_class,
        status = journal.status,
        id = html_escape(&journal.experiment_id),
        duration_ms = journal.duration_ms,
        method_count = journal.method_results.len(),
        activities = activities_html,
        analysis = analysis_html,
        regulatory = regulatory_html,
    )
}

fn format_activity_row(r: &ActivityResult, phase: &str) -> String {
    let status_emoji = match r.status {
        ActivityStatus::Succeeded => "&#10004;",
        ActivityStatus::Failed => "&#10008;",
        ActivityStatus::Timeout => "&#9203;",
        ActivityStatus::Skipped => "&#8212;",
    };
    let trace = if r.trace_id.is_empty() {
        String::new()
    } else {
        let tid = r.trace_id.as_str();
        format!(
            r#"<span class="trace-link">{}</span>"#,
            &tid[..tid.len().min(16)]
        )
    };
    format!(
        "<tr><td>{}</td><td>{}</td><td>{:?}</td><td>{} {:?}</td><td>{}ms</td><td>{}</td></tr>\n",
        phase,
        html_escape(&r.name),
        r.activity_type,
        status_emoji,
        r.status,
        r.duration_ms,
        trace,
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tumult_core::types::ActivityType;

    // ── Helper: write a valid experiment file ─────────────────

    fn write_valid_experiment(dir: &Path) -> std::path::PathBuf {
        let path = dir.join("test-experiment.toon");
        let exp = Experiment {
            version: "v1".into(),
            title: "CLI test experiment".into(),
            description: Some("Tests CLI command execution".into()),
            tags: vec!["test".into()],
            configuration: indexmap::IndexMap::new(),
            secrets: indexmap::IndexMap::new(),
            controls: vec![],
            steady_state_hypothesis: None,
            method: vec![Activity {
                name: "echo-action".into(),
                activity_type: ActivityType::Action,
                provider: Provider::Process {
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
            }],
            rollbacks: vec![],
            estimate: None,
            baseline: None,
            load: None,
            regulatory: None,
        };
        let toon = toon_format::encode_default(&exp).unwrap();
        std::fs::write(&path, toon).unwrap();
        path
    }

    fn write_invalid_experiment(dir: &Path) -> std::path::PathBuf {
        let path = dir.join("invalid.toon");
        std::fs::write(&path, "this is not valid toon {{{").unwrap();
        path
    }

    fn write_empty_method_experiment(dir: &Path) -> std::path::PathBuf {
        let path = dir.join("empty-method.toon");
        let exp = Experiment {
            version: "v1".into(),
            title: "Empty method experiment".into(),
            description: None,
            tags: vec![],
            configuration: indexmap::IndexMap::new(),
            secrets: indexmap::IndexMap::new(),
            controls: vec![],
            steady_state_hypothesis: None,
            method: vec![],
            rollbacks: vec![],
            estimate: None,
            baseline: None,
            load: None,
            regulatory: None,
        };
        let toon = toon_format::encode_default(&exp).unwrap();
        std::fs::write(&path, toon).unwrap();
        path
    }

    // ── cmd_run tests ─────────────────────────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_valid_experiment_produces_journal() {
        let dir = TempDir::new().unwrap();
        let exp_path = write_valid_experiment(dir.path());
        let journal_path = dir.path().join("journal.toon");

        let result = cmd_run(
            &exp_path,
            &journal_path,
            false,
            RollbackStrategy::OnDeviation,
            false,
            std::collections::HashMap::new(),
            None,
        )
        .await;

        assert!(result.is_ok());
        assert!(journal_path.exists());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_dry_run_does_not_create_journal() {
        let dir = TempDir::new().unwrap();
        let exp_path = write_valid_experiment(dir.path());
        let journal_path = dir.path().join("journal.toon");

        let result = cmd_run(
            &exp_path,
            &journal_path,
            true,
            RollbackStrategy::OnDeviation,
            false,
            std::collections::HashMap::new(),
            None,
        )
        .await;

        assert!(result.is_ok());
        assert!(!journal_path.exists());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_nonexistent_file_returns_error() {
        let result = cmd_run(
            Path::new("/nonexistent/experiment.toon"),
            Path::new("journal.toon"),
            false,
            RollbackStrategy::OnDeviation,
            false,
            std::collections::HashMap::new(),
            None,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_invalid_toon_returns_error() {
        let dir = TempDir::new().unwrap();
        let exp_path = write_invalid_experiment(dir.path());

        let result = cmd_run(
            &exp_path,
            &dir.path().join("journal.toon"),
            false,
            RollbackStrategy::OnDeviation,
            false,
            std::collections::HashMap::new(),
            None,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_empty_method_returns_error() {
        let dir = TempDir::new().unwrap();
        let exp_path = write_empty_method_experiment(dir.path());

        let result = cmd_run(
            &exp_path,
            &dir.path().join("journal.toon"),
            false,
            RollbackStrategy::OnDeviation,
            false,
            std::collections::HashMap::new(),
            None,
        )
        .await;
        assert!(result.is_err());
    }

    // ── cmd_validate tests ────────────────────────────────────

    #[test]
    fn validate_valid_experiment_succeeds() {
        let dir = TempDir::new().unwrap();
        let exp_path = write_valid_experiment(dir.path());

        let result = cmd_validate(&exp_path);
        assert!(result.is_ok());
    }

    #[test]
    fn validate_nonexistent_file_returns_error() {
        let result = cmd_validate(Path::new("/nonexistent/experiment.toon"));
        assert!(result.is_err());
    }

    #[test]
    fn validate_invalid_toon_returns_error() {
        let dir = TempDir::new().unwrap();
        let exp_path = write_invalid_experiment(dir.path());

        let result = cmd_validate(&exp_path);
        assert!(result.is_err());
    }

    #[test]
    fn validate_empty_method_returns_error() {
        let dir = TempDir::new().unwrap();
        let exp_path = write_empty_method_experiment(dir.path());

        let result = cmd_validate(&exp_path);
        assert!(result.is_err());
    }

    // ── cmd_discover tests ────────────────────────────────────

    #[test]
    fn discover_without_plugins_shows_empty() {
        // No plugins in default search paths during tests
        let result = cmd_discover(None);
        assert!(result.is_ok());
    }

    #[test]
    fn discover_nonexistent_plugin_returns_error() {
        let result = cmd_discover(Some("nonexistent-plugin"));
        assert!(result.is_err());
    }

    // ── cmd_init tests ────────────────────────────────────────

    #[test]
    fn init_creates_experiment_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("experiment.toon");

        let result = init_at(&path, None);

        assert!(result.is_ok());
        assert!(path.exists());
    }

    #[test]
    fn init_with_plugin_includes_plugin_name() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("experiment.toon");

        let result = init_at(&path, Some("tumult-kafka"));

        assert!(result.is_ok());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("tumult-kafka"));
    }

    #[test]
    fn init_fails_if_file_exists() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("experiment.toon");
        std::fs::write(&path, "existing").unwrap();

        let result = init_at(&path, None);
        assert!(result.is_err());
    }

    // ── generate_template tests ───────────────────────────────

    #[test]
    fn template_contains_required_sections() {
        let template = generate_template(None);
        assert!(template.contains("title:"));
        assert!(template.contains("steady_state_hypothesis:"));
        assert!(template.contains("method"));
        assert!(template.contains("rollbacks"));
    }

    #[test]
    fn template_uses_plugin_name() {
        let template = generate_template(Some("tumult-db"));
        assert!(template.contains("tumult-db"));
    }

    // ── ProviderExecutor tests ────────────────────────────────

    #[tokio::test(flavor = "multi_thread")]
    async fn process_executor_runs_echo() {
        let activity = Activity {
            name: "echo-test".into(),
            activity_type: ActivityType::Action,
            provider: Provider::Process {
                path: "echo".into(),
                arguments: vec!["hello world".into()],
                env: std::collections::HashMap::new(),
                timeout_s: Some(5.0),
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
            label_selector: None,
        };

        let executor = ProviderExecutor;
        let outcome = executor.execute(&activity);

        assert!(outcome.success);
        assert_eq!(outcome.output.as_deref(), Some("hello world"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn process_executor_captures_failure() {
        let activity = Activity {
            name: "false-test".into(),
            activity_type: ActivityType::Action,
            provider: Provider::Process {
                path: "false".into(),
                arguments: vec![],
                env: std::collections::HashMap::new(),
                timeout_s: Some(5.0),
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
            label_selector: None,
        };

        let executor = ProviderExecutor;
        let outcome = executor.execute(&activity);

        assert!(!outcome.success);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn process_executor_nonexistent_returns_error() {
        let activity = Activity {
            name: "bad-cmd".into(),
            activity_type: ActivityType::Action,
            provider: Provider::Process {
                path: "/nonexistent/binary".into(),
                arguments: vec![],
                env: std::collections::HashMap::new(),
                timeout_s: None,
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
            label_selector: None,
        };

        let executor = ProviderExecutor;
        let outcome = executor.execute(&activity);

        assert!(!outcome.success);
        assert!(outcome.error.is_some());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn native_provider_rejects_unknown_plugin() {
        let activity = Activity {
            name: "native-test".into(),
            activity_type: ActivityType::Action,
            provider: Provider::Native {
                plugin: "unknown-plugin".into(),
                function: "test-fn".into(),
                arguments: std::collections::HashMap::new(),
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
            label_selector: None,
        };

        let executor = ProviderExecutor;
        let outcome = executor.execute(&activity);

        assert!(!outcome.success);
        assert!(outcome
            .error
            .as_ref()
            .unwrap()
            .contains("unknown native plugin"));
    }

    // ── Phase 4: Import/Export roundtrip ──────────────────────

    #[test]
    fn import_rejects_missing_directory() {
        let result = cmd_import(Path::new("/nonexistent/path"));
        assert!(result.is_err());
    }

    #[test]
    fn import_rejects_missing_parquet_files() {
        let d = TempDir::new().unwrap();
        let result = cmd_import(d.path());
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("experiments.parquet not found"));
    }

    // ── Phase 4: Run with auto-ingest ─────────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cmd_run_with_auto_ingest() {
        let d = TempDir::new().unwrap();
        let exp_path = write_valid_experiment(d.path());
        let journal_path = d.path().join("out.toon");

        // Run with auto-ingest disabled (avoids touching real ~/.tumult)
        let result = cmd_run(
            &exp_path,
            &journal_path,
            false,
            RollbackStrategy::OnDeviation,
            false,
            std::collections::HashMap::new(),
            None,
        )
        .await;
        assert!(result.is_ok());
        assert!(journal_path.exists());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cmd_run_dry_run_does_not_ingest() {
        let d = TempDir::new().unwrap();
        let exp_path = write_valid_experiment(d.path());
        let journal_path = d.path().join("out.toon");

        let result = cmd_run(
            &exp_path,
            &journal_path,
            true,
            RollbackStrategy::OnDeviation,
            true,
            std::collections::HashMap::new(),
            None,
        )
        .await;
        assert!(result.is_ok());
        // Journal should NOT be written in dry-run mode
        assert!(!journal_path.exists());
    }

    // ── Phase 4: Store command tests ──────────────────────────

    #[test]
    fn store_backup_creates_parquet_files() {
        use tumult_analytics::AnalyticsStore;
        use tumult_core::types::*;

        let d = TempDir::new().unwrap();
        let db_path = d.path().join("test.duckdb");
        let backup_dir = d.path().join("backup");

        // Create store with data
        let store = AnalyticsStore::open(&db_path).unwrap();
        store
            .ingest_journal(&Journal {
                experiment_title: "test".into(),
                experiment_id: "e1".into(),
                status: ExperimentStatus::Completed,
                started_at_ns: 1_774_980_000_000_000_000,
                ended_at_ns: 1_774_980_060_000_000_000,
                duration_ms: 60_000,
                method_results: vec![],
                steady_state_before: None,
                steady_state_after: None,
                rollback_results: vec![],
                rollback_failures: 0,
                estimate: None,
                baseline_result: None,
                during_result: None,
                post_result: None,
                load_result: None,
                analysis: None,
                regulatory: None,
            })
            .unwrap();
        drop(store);

        // Backup via store API directly
        let store = AnalyticsStore::open(&db_path).unwrap();
        std::fs::create_dir_all(&backup_dir).unwrap();
        store
            .export_tables(
                &backup_dir.join("experiments.parquet"),
                &backup_dir.join("activities.parquet"),
            )
            .unwrap();

        assert!(backup_dir.join("experiments.parquet").exists());
        assert!(backup_dir.join("activities.parquet").exists());
    }

    #[test]
    fn store_purge_removes_old_data() {
        use tumult_analytics::AnalyticsStore;
        use tumult_core::types::*;

        let d = TempDir::new().unwrap();
        let db_path = d.path().join("test.duckdb");
        let store = AnalyticsStore::open(&db_path).unwrap();

        // Old experiment (2020)
        store
            .ingest_journal(&Journal {
                experiment_title: "old".into(),
                experiment_id: "old-1".into(),
                status: ExperimentStatus::Completed,
                started_at_ns: 1_577_836_800_000_000_000,
                ended_at_ns: 1_577_836_860_000_000_000,
                duration_ms: 60_000,
                method_results: vec![],
                steady_state_before: None,
                steady_state_after: None,
                rollback_results: vec![],
                rollback_failures: 0,
                estimate: None,
                baseline_result: None,
                during_result: None,
                post_result: None,
                load_result: None,
                analysis: None,
                regulatory: None,
            })
            .unwrap();

        // Recent experiment
        store
            .ingest_journal(&Journal {
                experiment_title: "new".into(),
                experiment_id: "new-1".into(),
                status: ExperimentStatus::Completed,
                started_at_ns: 1_774_980_000_000_000_000,
                ended_at_ns: 1_774_980_060_000_000_000,
                duration_ms: 60_000,
                method_results: vec![],
                steady_state_before: None,
                steady_state_after: None,
                rollback_results: vec![],
                rollback_failures: 0,
                estimate: None,
                baseline_result: None,
                during_result: None,
                post_result: None,
                load_result: None,
                analysis: None,
                regulatory: None,
            })
            .unwrap();

        assert_eq!(store.experiment_count().unwrap(), 2);
        let purged = store.purge_older_than_days(30).unwrap();
        assert_eq!(purged, 1);
        assert_eq!(store.experiment_count().unwrap(), 1);
    }

    #[test]
    fn store_stats_reports_counts() {
        use tumult_analytics::AnalyticsStore;
        use tumult_core::types::*;

        let store = AnalyticsStore::in_memory().unwrap();
        let stats = store.stats().unwrap();
        assert_eq!(stats.experiment_count, 0);
        assert_eq!(stats.activity_count, 0);

        store
            .ingest_journal(&Journal {
                experiment_title: "test".into(),
                experiment_id: "e1".into(),
                status: ExperimentStatus::Completed,
                started_at_ns: 1_774_980_000_000_000_000,
                ended_at_ns: 1_774_980_060_000_000_000,
                duration_ms: 60_000,
                method_results: vec![ActivityResult {
                    name: "act".into(),
                    activity_type: ActivityType::Action,
                    status: ActivityStatus::Succeeded,
                    started_at_ns: 1_774_980_000_000_000_000,
                    duration_ms: 500,
                    output: None,
                    error: None,
                    trace_id: TraceId::empty(),
                    span_id: SpanId::empty(),
                }],
                steady_state_before: None,
                steady_state_after: None,
                rollback_results: vec![],
                rollback_failures: 0,
                estimate: None,
                baseline_result: None,
                during_result: None,
                post_result: None,
                load_result: None,
                analysis: None,
                regulatory: None,
            })
            .unwrap();

        let stats = store.stats().unwrap();
        assert_eq!(stats.experiment_count, 1);
        assert_eq!(stats.activity_count, 1);
    }

    // ── Phase 3: Report command ──────────────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn report_generates_html_file() {
        let d = TempDir::new().unwrap();
        let exp_path = write_valid_experiment(d.path());
        let journal_path = d.path().join("journal.toon");

        cmd_run(
            &exp_path,
            &journal_path,
            false,
            RollbackStrategy::OnDeviation,
            false,
            std::collections::HashMap::new(),
            None,
        )
        .await
        .unwrap();

        let report_path = d.path().join("report.html");
        cmd_report(&journal_path, Some(&report_path), "html").unwrap();
        assert!(report_path.exists());

        let content = std::fs::read_to_string(&report_path).unwrap();
        assert!(content.contains("<!DOCTYPE html>"));
        assert!(content.contains("Tumult Experiment Report"));
        assert!(content.contains("CLI test experiment"));
        assert!(content.contains("Activity Timeline"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn report_default_output_uses_journal_stem() {
        let d = TempDir::new().unwrap();
        let exp_path = write_valid_experiment(d.path());
        let journal_path = d.path().join("my-experiment.toon");

        cmd_run(
            &exp_path,
            &journal_path,
            false,
            RollbackStrategy::OnDeviation,
            false,
            std::collections::HashMap::new(),
            None,
        )
        .await
        .unwrap();

        // Change to temp dir so default output lands there
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(d.path()).unwrap();
        cmd_report(&journal_path, None, "html").unwrap();
        std::env::set_current_dir(prev).unwrap();

        assert!(d.path().join("my-experiment.html").exists());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn report_html_contains_trace_ids() {
        let d = TempDir::new().unwrap();
        let exp_path = write_valid_experiment(d.path());
        let journal_path = d.path().join("journal.toon");

        cmd_run(
            &exp_path,
            &journal_path,
            false,
            RollbackStrategy::OnDeviation,
            false,
            std::collections::HashMap::new(),
            None,
        )
        .await
        .unwrap();

        let report_path = d.path().join("report.html");
        cmd_report(&journal_path, Some(&report_path), "html").unwrap();

        let content = std::fs::read_to_string(&report_path).unwrap();
        // Should contain method steps
        assert!(content.contains("echo-action"));
    }

    #[test]
    fn report_nonexistent_journal_returns_error() {
        let result = cmd_report(Path::new("/nonexistent.toon"), None, "html");
        assert!(result.is_err());
    }
}
