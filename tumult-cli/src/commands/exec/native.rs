use std::sync::OnceLock;

use tumult_core::runner::ActivityOutcome;
use tumult_core::sync_bridge::sync_await;
use tumult_plugin::native::NativeExecutorRegistry;

/// The CLI's composition root for native plugin dispatch and discovery.
///
/// Each native crate implements [`tumult_plugin::native::NativeExecutor`]
/// for its own functions; registering the trait object here is the only CLI
/// change needed to expose a new plugin. Lookup and function validation are
/// handled by the registry, which returns typed errors listing what is
/// available. `tumult discover` reads the same registry, so registered
/// plugins are discoverable automatically.
pub(crate) fn registry() -> &'static NativeExecutorRegistry {
    static REGISTRY: OnceLock<NativeExecutorRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut registry = NativeExecutorRegistry::new();
        registry.register(Box::new(tumult_kubernetes::KubernetesExecutor));
        registry.register(Box::new(tumult_ssh::SshExecutor));
        registry.register(Box::new(tumult_net::NetExecutor));
        registry.register(Box::new(tumult_cloud::CloudExecutor));
        registry
    })
}

/// Dispatch a native plugin call to the appropriate Rust implementation.
///
/// Runs the async executor on the current Tokio runtime and converts the
/// typed [`tumult_plugin::native::NativeError`] to a string exactly once,
/// at this `ActivityOutcome` boundary.
///
/// # Panics
///
/// Panics if called from outside a Tokio multi-threaded runtime context; see
/// [`sync_await`].
pub(super) fn execute_native(
    plugin: &str,
    function: &str,
    arguments: &std::collections::HashMap<String, serde_json::Value>,
) -> ActivityOutcome {
    let start = std::time::Instant::now();

    let result = sync_await(registry().dispatch(plugin, function, arguments));

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
            error: Some(e.to_string()),
            duration_ms,
        },
    }
}
