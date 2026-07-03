use tumult_core::runner::{ActivityExecutor, ActivityOutcome};
use tumult_core::types::{Activity, HttpMethod, Provider};

// tumult-net dispatch lives in a sibling source file (commands/net.rs) but is
// wired in as a child module so it can reuse this module's private arg helpers.
#[path = "../net.rs"]
mod net;

mod native;
mod process;

use native::execute_native;
use process::execute_process;

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

fn format_http_method(method: &HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
        HttpMethod::Put => "PUT",
        HttpMethod::Delete => "DELETE",
        HttpMethod::Patch => "PATCH",
    }
}
