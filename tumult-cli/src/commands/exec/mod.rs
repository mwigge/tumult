use tumult_core::runner::{ActivityExecutor, ActivityOutcome};
use tumult_core::types::{Activity, Provider};

mod native;
mod process;

use native::execute_native;
use process::execute_process;

pub(crate) use native::registry as native_registry;

// ── Provider-based executor ───────────────────────────────────

/// Executes activities by dispatching to the appropriate provider.
///
/// Supports Process and Native (Rust) providers. Native plugins dispatch
/// through the `tumult_plugin::native::NativeExecutorRegistry` composition
/// root in [`native`], via async execution on the Tokio runtime.
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
            Provider::Native {
                plugin,
                function,
                arguments,
            } => execute_native(plugin, function, arguments),
        }
    }
}
