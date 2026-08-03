//! Plugin registry — discovers and resolves plugins.
//!
//! The registry holds both native Rust plugins and script plugin manifests.
//! It provides lookup by plugin name and action/probe name.

use std::collections::HashMap;

use crate::manifest::ScriptPluginManifest;
use crate::traits::{ActionDescriptor, TumultPlugin};

/// Central registry for all discovered plugins.
pub struct PluginRegistry {
    native: HashMap<String, Box<dyn TumultPlugin>>,
    scripts: HashMap<String, ScriptPluginManifest>,
}

impl PluginRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            native: HashMap::new(),
            scripts: HashMap::new(),
        }
    }

    pub fn register_native(&mut self, plugin: Box<dyn TumultPlugin>) {
        let name = plugin.name().to_string();
        self.native.insert(name, plugin);
    }

    pub fn register_script(&mut self, manifest: ScriptPluginManifest) {
        let name = manifest.name.clone();
        self.scripts.insert(name, manifest);
    }

    #[must_use]
    pub fn list_plugins(&self) -> Vec<String> {
        let mut names: Vec<String> = self.native.keys().cloned().collect();
        names.extend(self.scripts.keys().cloned());
        names.sort();
        names
    }

    #[must_use]
    pub fn has_action(&self, plugin: &str, action: &str) -> bool {
        if let Some(p) = self.native.get(plugin) {
            return p.actions().iter().any(|a| a.name == action);
        }
        if let Some(m) = self.scripts.get(plugin) {
            return m.actions.iter().any(|a| a.name == action);
        }
        false
    }

    #[must_use]
    pub fn has_probe(&self, plugin: &str, probe: &str) -> bool {
        if let Some(p) = self.native.get(plugin) {
            return p.probes().iter().any(|pr| pr.name == probe);
        }
        if let Some(m) = self.scripts.get(plugin) {
            return m.probes.iter().any(|pr| pr.name == probe);
        }
        false
    }

    #[must_use]
    pub fn list_all_actions(&self) -> Vec<(String, ActionDescriptor)> {
        let mut result = Vec::new();
        for (name, plugin) in &self.native {
            for action in plugin.actions() {
                result.push((name.clone(), action.clone()));
            }
        }
        for (name, manifest) in &self.scripts {
            for action in &manifest.actions {
                result.push((
                    name.clone(),
                    ActionDescriptor {
                        name: action.name.clone(),
                        description: action.description.clone(),
                        arguments: vec![],
                    },
                ));
            }
        }
        result
    }
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{ScriptAction, ScriptProbe};
    use crate::traits::{private, ProbeDescriptor};
    use std::path::PathBuf;

    struct MockNative {
        actions: Vec<ActionDescriptor>,
        probes: Vec<ProbeDescriptor>,
    }

    impl private::Sealed for MockNative {}

    impl TumultPlugin for MockNative {
        fn name(&self) -> &'static str {
            "mock-native"
        }
        fn version(&self) -> &'static str {
            "0.1.0"
        }
        fn description(&self) -> &'static str {
            "mock native plugin"
        }
        fn actions(&self) -> &[ActionDescriptor] {
            &self.actions
        }
        fn probes(&self) -> &[ProbeDescriptor] {
            &self.probes
        }
    }

    fn script_manifest() -> ScriptPluginManifest {
        ScriptPluginManifest {
            name: "mock-script".into(),
            version: "0.1.0".into(),
            description: "mock script plugin".into(),
            actions: vec![ScriptAction {
                name: "restart".into(),
                script: PathBuf::from("actions/restart.sh"),
                description: "Restart the thing".into(),
            }],
            probes: vec![ScriptProbe {
                name: "is-up".into(),
                script: PathBuf::from("probes/is-up.sh"),
                description: "Check the thing".into(),
            }],
        }
    }

    #[test]
    fn default_registry_matches_new() {
        let registry = PluginRegistry::default();
        assert!(registry.list_plugins().is_empty());
        assert!(!registry.has_action("anything", "anything"));
    }

    #[test]
    fn script_plugin_probes_are_found_by_name() {
        let mut registry = PluginRegistry::new();
        registry.register_script(script_manifest());
        assert!(registry.has_probe("mock-script", "is-up"));
        assert!(!registry.has_probe("mock-script", "is-down"));
        assert!(!registry.has_probe("unregistered", "is-up"));
    }

    #[test]
    fn list_all_actions_includes_script_entries_with_empty_arguments() {
        let mut registry = PluginRegistry::new();
        registry.register_native(Box::new(MockNative {
            actions: vec![ActionDescriptor {
                name: "kill".into(),
                description: "Kill the thing".into(),
                arguments: vec!["target".into()],
            }],
            probes: vec![ProbeDescriptor {
                name: "alive".into(),
                description: "Liveness".into(),
                arguments: vec![],
            }],
        }));
        registry.register_script(script_manifest());

        let actions = registry.list_all_actions();
        assert_eq!(actions.len(), 2);
        let (plugin, descriptor) = actions
            .iter()
            .find(|(plugin, _)| plugin == "mock-script")
            .expect("script action must be listed");
        assert_eq!(descriptor.name, "restart");
        assert_eq!(descriptor.description, "Restart the thing");
        // Script manifests declare no per-action arguments.
        assert!(descriptor.arguments.is_empty());
        assert_eq!(plugin, "mock-script");

        assert!(registry.has_action("mock-native", "kill"));
        assert!(registry.has_probe("mock-native", "alive"));
        assert_eq!(
            registry.list_plugins(),
            vec!["mock-native".to_string(), "mock-script".to_string()]
        );
    }
}
