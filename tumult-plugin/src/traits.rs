//! Plugin trait definitions for native Rust plugins.

use serde::{Deserialize, Serialize};

/// Describes an available action a plugin can perform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionDescriptor {
    pub name: String,
    pub description: String,
    pub arguments: Vec<String>,
}

/// Describes an available probe a plugin can execute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeDescriptor {
    pub name: String,
    pub description: String,
    pub arguments: Vec<String>,
}

/// Output from executing an action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Output from executing a probe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Trait that all native Rust plugins must implement.
///
/// Plugins declare their available actions and probes via descriptors.
/// Execution is handled separately by the engine, which calls into
/// the plugin via the registry.
pub trait TumultPlugin: Send + Sync {
    fn name(&self) -> &str;
    fn version(&self) -> &str;
    fn description(&self) -> &str;
    fn actions(&self) -> Vec<ActionDescriptor>;
    fn probes(&self) -> Vec<ProbeDescriptor>;
}
