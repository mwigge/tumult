//! Script plugin manifest types.
//!
//! Community plugins are directories with a `plugin.toon` manifest
//! declaring available actions and probes as executable scripts.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A script-based action declared in a plugin manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptAction {
    pub name: String,
    pub script: PathBuf,
    pub description: String,
}

/// A script-based probe declared in a plugin manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptProbe {
    pub name: String,
    pub script: PathBuf,
    pub description: String,
}

/// Manifest for a script-based community plugin.
///
/// Loaded from `plugin.toon` in the plugin directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptPluginManifest {
    pub name: String,
    pub version: String,
    pub description: String,
    pub actions: Vec<ScriptAction>,
    pub probes: Vec<ScriptProbe>,
}
