//! Tumult Plugin — Plugin trait definitions, registry, and manifest loader.
//!
//! Provides the [`TumultPlugin`] trait for native Rust plugins and the
//! script plugin manifest parser for community plugins.
//!
//! # Overview
//!
//! Tumult uses a dual plugin model:
//!
//! - **Native plugins** implement the [`TumultPlugin`] trait in Rust.
//!   They are compiled into the binary and registered at startup via
//!   [`PluginRegistry::register_native`].
//! - **Script plugins** are directories containing a `plugin.toon` manifest
//!   and shell-script actions/probes. They are discovered at runtime by
//!   [`discovery`] and registered with [`PluginRegistry::register_script`].
//!
//! # Key types
//!
//! | Type | Purpose |
//! |------------------------|---------------------------------------------|
//! | [`TumultPlugin`] | Trait that native plugins implement |
//! | [`PluginRegistry`] | Central lookup for actions and probes |
//! | [`ScriptPluginManifest`]| Deserialized `plugin.toon` descriptor |
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
pub mod registry;
pub mod telemetry;
pub mod traits;

pub use manifest::ScriptPluginManifest;
pub use registry::PluginRegistry;
pub use traits::{ActionDescriptor, ActionOutput, ProbeDescriptor, ProbeOutput, TumultPlugin};

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ── TumultPlugin trait ─────────────────────────────────────

    #[test]
    fn mock_plugin_implements_trait() {
        let plugin = MockPlugin;
        assert_eq!(plugin.name(), "mock-plugin");
        assert_eq!(plugin.version(), "0.1.0");
        assert!(!plugin.actions().is_empty());
        assert!(!plugin.probes().is_empty());
        assert_eq!(plugin.actions()[0].name, "kill");
        assert_eq!(plugin.probes()[0].name, "health-check");
    }

    // ── ActionDescriptor / ProbeDescriptor ─────────────────────

    #[test]
    fn action_descriptor_round_trips() {
        let desc = ActionDescriptor {
            name: "terminate-connections".into(),
            description: "Kill all active database connections".into(),
            arguments: vec!["database".into(), "max_wait".into()],
        };
        let encoded = toon_format::encode_default(&desc).unwrap();
        let decoded: ActionDescriptor = toon_format::decode_default(&encoded).unwrap();
        assert_eq!(decoded, desc);
    }

    #[test]
    fn probe_descriptor_round_trips() {
        let desc = ProbeDescriptor {
            name: "connection-count".into(),
            description: "Count active database connections".into(),
            arguments: vec!["database".into()],
        };
        let encoded = toon_format::encode_default(&desc).unwrap();
        let decoded: ProbeDescriptor = toon_format::decode_default(&encoded).unwrap();
        assert_eq!(decoded, desc);
    }

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
    fn registry_starts_empty() {
        let registry = PluginRegistry::new();
        assert!(registry.list_plugins().is_empty());
    }

    #[test]
    fn registry_registers_native_plugin() {
        let mut registry = PluginRegistry::new();
        registry.register_native(Box::new(MockPlugin));
        let plugins = registry.list_plugins();
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0], "mock-plugin");
    }

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

    #[test]
    fn registry_finds_action_in_native_plugin() {
        let mut registry = PluginRegistry::new();
        registry.register_native(Box::new(MockPlugin));
        assert!(registry.has_action("mock-plugin", "kill"));
        assert!(!registry.has_action("mock-plugin", "nonexistent"));
        assert!(!registry.has_action("wrong-plugin", "kill"));
    }

    #[test]
    fn registry_finds_probe_in_native_plugin() {
        let mut registry = PluginRegistry::new();
        registry.register_native(Box::new(MockPlugin));
        assert!(registry.has_probe("mock-plugin", "health-check"));
        assert!(!registry.has_probe("mock-plugin", "nonexistent"));
    }

    #[test]
    fn registry_finds_action_in_script_plugin() {
        let mut registry = PluginRegistry::new();
        registry.register_script(ScriptPluginManifest {
            name: "tumult-kafka".into(),
            version: "0.1.0".into(),
            description: "Kafka chaos".into(),
            actions: vec![manifest::ScriptAction {
                name: "kill-broker".into(),
                script: PathBuf::from("actions/kill-broker.sh"),
                description: "Kill broker".into(),
            }],
            probes: vec![],
        });
        assert!(registry.has_action("tumult-kafka", "kill-broker"));
        assert!(!registry.has_action("tumult-kafka", "nonexistent"));
    }

    #[test]
    fn registry_lists_all_actions() {
        let mut registry = PluginRegistry::new();
        registry.register_native(Box::new(MockPlugin));
        registry.register_script(ScriptPluginManifest {
            name: "tumult-kafka".into(),
            version: "0.1.0".into(),
            description: "Kafka chaos".into(),
            actions: vec![manifest::ScriptAction {
                name: "kill-broker".into(),
                script: PathBuf::from("actions/kill-broker.sh"),
                description: "Kill broker".into(),
            }],
            probes: vec![],
        });
        let all = registry.list_all_actions();
        assert_eq!(all.len(), 2);
    }

    // ── Mock plugin for testing ────────────────────────────────

    struct MockPlugin;

    impl crate::traits::private::Sealed for MockPlugin {}
    impl TumultPlugin for MockPlugin {
        fn name(&self) -> &str {
            "mock-plugin"
        }
        fn version(&self) -> &str {
            "0.1.0"
        }
        fn description(&self) -> &str {
            "A mock plugin for testing"
        }
        fn actions(&self) -> Vec<ActionDescriptor> {
            vec![ActionDescriptor {
                name: "kill".into(),
                description: "Kill something".into(),
                arguments: vec!["target".into()],
            }]
        }
        fn probes(&self) -> Vec<ProbeDescriptor> {
            vec![ProbeDescriptor {
                name: "health-check".into(),
                description: "Check health".into(),
                arguments: vec![],
            }]
        }
    }
}
