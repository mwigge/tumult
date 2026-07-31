//! Script plugin dispatch: resolve a `type: script` provider through the
//! plugin discovery search paths and run the manifest-declared script via
//! [`tumult_plugin::executor::execute_script`].

use std::collections::HashMap;

use tumult_core::runner::ActivityOutcome;
use tumult_core::sync_bridge::sync_await;
use tumult_plugin::discovery::discover_all_report;
use tumult_plugin::executor::execute_script;

/// Dispatch a script provider call to the discovered plugin's script.
///
/// The plugin manifest is resolved through the normal discovery search
/// paths (`./plugins`, `~/.tumult/plugins`, `TUMULT_PLUGIN_PATH`);
/// `function` names an entry in its `actions` (falling back to `probes`).
/// Values in `arguments` reach the script as `TUMULT_*` environment
/// variables (`dns_domain` → `TUMULT_DNS_DOMAIN`).
///
/// Lookup failures name what is available, mirroring the native registry's
/// `UnknownPlugin`/`UnknownFunction` style, and append any discovery
/// warnings so a skipped path or malformed manifest is visible instead of
/// silently shrinking the available set.
///
/// # Panics
///
/// Panics if called from outside a Tokio multi-threaded runtime context; see
/// [`sync_await`].
pub(super) fn execute_script_provider(
    plugin: &str,
    function: &str,
    arguments: &HashMap<String, serde_json::Value>,
    timeout_s: Option<&f64>,
) -> ActivityOutcome {
    let start = std::time::Instant::now();

    let report = discover_all_report();
    let warnings = if report.warnings.is_empty() {
        String::new()
    } else {
        format!("; discovery warnings: {}", report.warnings.join("; "))
    };

    let Some(discovered) = report.plugins.iter().find(|p| p.manifest.name == plugin) else {
        let available = report
            .plugins
            .iter()
            .map(|p| p.manifest.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return ActivityOutcome {
            success: false,
            output: None,
            error: Some(format!(
                "unknown script plugin: {plugin} (available: {available}){warnings}"
            )),
            duration_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
        };
    };

    // Actions first, then probes — a probe activity names a manifest probe.
    let script_rel = discovered
        .manifest
        .actions
        .iter()
        .find(|a| a.name == function)
        .map(|a| &a.script)
        .or_else(|| {
            discovered
                .manifest
                .probes
                .iter()
                .find(|p| p.name == function)
                .map(|p| &p.script)
        });
    let Some(script_rel) = script_rel else {
        let available = discovered
            .manifest
            .actions
            .iter()
            .map(|a| a.name.as_str())
            .chain(discovered.manifest.probes.iter().map(|p| p.name.as_str()))
            .collect::<Vec<_>>()
            .join(", ");
        return ActivityOutcome {
            success: false,
            output: None,
            error: Some(format!(
                "unknown {plugin} action: {function} (available: {available}){warnings}"
            )),
            duration_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
        };
    };

    let script_path = discovered.root.join(script_rel);
    let args: HashMap<String, String> = arguments
        .iter()
        .map(|(k, v)| (k.clone(), argument_value_to_string(v)))
        .collect();
    let timeout = timeout_s.map(|s| std::time::Duration::from_secs_f64(*s));

    let duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
    match sync_await(execute_script(
        &script_path,
        &discovered.root,
        &args,
        timeout,
    )) {
        Ok(result) => {
            let success = result.succeeded();
            let stdout = result.stdout.trim();
            let stderr = result.stderr.trim();
            ActivityOutcome {
                success,
                output: if stdout.is_empty() {
                    None
                } else {
                    Some(stdout.to_string())
                },
                error: if success {
                    if stderr.is_empty() {
                        None
                    } else {
                        Some(stderr.to_string())
                    }
                } else if stderr.is_empty() {
                    let status = result.exit_status.code().map_or_else(
                        || "termination by signal".to_string(),
                        |code| format!("exit code {code}"),
                    );
                    Some(format!(
                        "script '{}' failed ({status})",
                        script_rel.display()
                    ))
                } else {
                    Some(stderr.to_string())
                },
                duration_ms,
            }
        }
        Err(e) => ActivityOutcome {
            success: false,
            output: None,
            error: Some(e.to_string()),
            duration_ms,
        },
    }
}

/// Stringify an argument value for the `TUMULT_*` environment: strings pass
/// through unquoted; numbers, booleans, and composite values use their JSON
/// representation.
fn argument_value_to_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}
