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

/// Output from executing a plugin action or probe.
#[must_use]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Backwards-compatible type alias for action output.
pub type ActionOutput = PluginOutput;

/// Backwards-compatible type alias for probe output.
pub type ProbeOutput = PluginOutput;

pub(crate) mod private {
    pub trait Sealed {}
}

/// Trait that all native Rust plugins must implement.
///
/// This trait is sealed — it cannot be implemented outside this crate.
/// Native plugins must be contributed to the main repository or use
/// the script plugin mechanism instead.
pub trait TumultPlugin: Send + Sync + private::Sealed {
    fn name(&self) -> &'static str;
    fn version(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn actions(&self) -> &[ActionDescriptor];
    fn probes(&self) -> &[ProbeDescriptor];
}
