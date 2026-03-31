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
