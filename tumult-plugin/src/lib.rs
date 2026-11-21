//! Tumult Plugin: native executors, registry, and manifest loader.
//!
//! Provides the [`NativeExecutor`] trait for native Rust plugins and the
//! script plugin manifest parser for community plugins.
//!
//! # Overview
//!
//! Tumult uses a dual plugin model:
//!
//! - **Native plugins** implement [`NativeExecutor`] in Rust. They are
//!   compiled into the binary and dispatched through a
//!   [`NativeExecutorRegistry`].
//! - **Script plugins** are directories containing a `plugin.toon` manifest
//!   and shell-script actions/probes. They are discovered at runtime by
//!   [`discovery`] and registered with [`PluginRegistry::register_script`].
//!
//! # Key types
//!
//! | Type | Purpose |
//! |--------------------------|---------------------------------------------|
//! | [`NativeExecutor`] | Trait that native plugins implement |
//! | [`NativeExecutorRegistry`]| Dispatch table for native executors |
//! | [`PluginRegistry`] | Lookup of the script plugins and their actions |
//! | [`ScriptPluginManifest`] | Deserialized `plugin.toon` descriptor |
//! | [`ActionDescriptor`] | Metadata for a single chaos action |
//! | [`ProbeDescriptor`] | Metadata for a single steady-state probe |
//!
//! # Plugin discovery
//!
//! The [`discovery`] module scans configured plugin directories for
//! `plugin.toon` manifests and auto-registers every valid script plugin.

pub mod discovery;
pub mod executor;
pub mod manifest;
pub mod native;
pub mod registry;
// Script execution telemetry is emitted only by the POSIX executor path.
#[cfg(not(windows))]
pub(crate) mod telemetry;
pub mod traits;

pub use manifest::ScriptPluginManifest;
pub use native::{NativeArgs, NativeError, NativeExecutor, NativeExecutorRegistry};
pub use registry::PluginRegistry;
pub use traits::{ActionDescriptor, ProbeDescriptor};

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ── ScriptPluginManifest ───────────────────────────────────

    #[test]
    fn script_manifest_deserializes_from_json() {
        let json = r#"{
            "name": "tumult-kafka",
            "version": "0.2.0",
            "description": "Kafka chaos actions and probes",
            "actions": [
                {"name": "kill-broker", "script": "actions/kill-broker.sh", "description": "Kill a Kafka broker"},
                {"name": "partition-topic", "script": "actions/partition-topic.sh", "description": "Partition a topic"}
            ],
            "probes": [
                {"name": "consumer-lag", "script": "probes/consumer-lag.sh", "description": "Check consumer lag"}
            ]
        }"#;
        let manifest: ScriptPluginManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.name, "tumult-kafka");
        assert_eq!(manifest.version, "0.2.0");
        assert_eq!(manifest.actions.len(), 2);
        assert_eq!(manifest.probes.len(), 1);
        assert_eq!(manifest.actions[0].name, "kill-broker");
        assert_eq!(
            manifest.actions[0].script,
            PathBuf::from("actions/kill-broker.sh")
        );
    }

    #[test]
    fn script_manifest_round_trips_through_toon() {
        let manifest = ScriptPluginManifest {
            name: "tumult-stress".into(),
            version: "0.1.0".into(),
            description: "CPU and memory stress testing".into(),
            actions: vec![manifest::ScriptAction {
                name: "cpu-stress".into(),
                script: PathBuf::from("actions/cpu-stress.sh"),
                description: "Stress CPU cores".into(),
            }],
            probes: vec![manifest::ScriptProbe {
                name: "cpu-utilization".into(),
                script: PathBuf::from("probes/cpu-util.sh"),
                description: "Current CPU usage".into(),
            }],
        };
        let encoded = toon_format::encode_default(&manifest).unwrap();
        let decoded: ScriptPluginManifest = toon_format::decode_default(&encoded).unwrap();
        assert_eq!(decoded, manifest);
    }

    // ── PluginRegistry ─────────────────────────────────────────

    #[test]
    fn registry_registers_script_plugin() {
        let mut registry = PluginRegistry::new();
        let manifest = ScriptPluginManifest {
            name: "tumult-kafka".into(),
            version: "0.1.0".into(),
            description: "Kafka chaos".into(),
            actions: vec![],
            probes: vec![],
        };
        registry.register_script(manifest);
        let plugins = registry.list_plugins();
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0], "tumult-kafka");
    }
}
