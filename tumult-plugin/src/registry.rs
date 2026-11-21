//! Plugin registry: the script plugin manifests that discovery found.
//!
//! It lists the plugins and their actions by name. Native plugins are
//! listed by the [`NativeExecutorRegistry`](crate::native::NativeExecutorRegistry)
//! that runs them.

use std::collections::HashMap;

use crate::manifest::ScriptPluginManifest;
use crate::traits::ActionDescriptor;

/// Central registry for the discovered script plugins.
pub struct PluginRegistry {
    scripts: HashMap<String, ScriptPluginManifest>,
}

impl PluginRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            scripts: HashMap::new(),
        }
    }

    pub fn register_script(&mut self, manifest: ScriptPluginManifest) {
        let name = manifest.name.clone();
        self.scripts.insert(name, manifest);
    }

    #[must_use]
    pub fn list_plugins(&self) -> Vec<String> {
        let mut names: Vec<String> = self.scripts.keys().cloned().collect();
        names.sort();
        names
    }

    #[must_use]
    pub fn list_all_actions(&self) -> Vec<(String, ActionDescriptor)> {
        let mut result = Vec::new();
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
    use std::path::PathBuf;

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
    fn new_and_default_registries_start_empty() {
        for registry in [PluginRegistry::new(), PluginRegistry::default()] {
            assert!(registry.list_plugins().is_empty());
            assert!(registry.list_all_actions().is_empty());
        }
    }

    #[test]
    fn list_all_actions_includes_script_entries_with_empty_arguments() {
        let mut registry = PluginRegistry::new();
        registry.register_script(script_manifest());

        let actions = registry.list_all_actions();
        assert_eq!(actions.len(), 1);
        let (plugin, descriptor) = actions
            .iter()
            .find(|(plugin, _)| plugin == "mock-script")
            .expect("script action must be listed");
        assert_eq!(descriptor.name, "restart");
        assert_eq!(descriptor.description, "Restart the thing");
        // Script manifests declare no per-action arguments.
        assert!(descriptor.arguments.is_empty());
        assert_eq!(plugin, "mock-script");
        assert_eq!(registry.list_plugins(), vec!["mock-script".to_string()]);
    }
}
