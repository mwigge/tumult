//! The MCP server's composition root for native plugins.
//!
//! Mirrors the CLI's registry in `tumult-cli/src/commands/exec/native.rs`:
//! each binary registers the same executors at its own composition root, so
//! neither binary depends on the other and `tumult-plugin` stays
//! dependency-light. Adding a native plugin means registering it here and in
//! the CLI — `tumult_discover` reads this registry directly.

use std::sync::OnceLock;

use tumult_plugin::native::NativeExecutorRegistry;

/// Native plugins visible to the `tumult_discover` tool.
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

#[cfg(test)]
mod tests {
    #[test]
    fn registry_exposes_all_native_plugins_for_discovery() {
        let registry = super::registry();
        let names = registry.plugin_names();
        for expected in [
            "tumult-cloud",
            "tumult-kubernetes",
            "tumult-net",
            "tumult-ssh",
        ] {
            assert!(names.contains(&expected), "missing {expected}: {names:?}");
            assert!(registry.get(expected).is_some());
        }
        // The composition root is a singleton: repeated calls return the same registry.
        assert!(std::ptr::eq(registry, super::registry()));
    }
}
