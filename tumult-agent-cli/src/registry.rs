//! Registry of built-in agent CLI adapters.

use crate::adapter::{AgentCliAdapter, CliProbe};
use crate::claude::ClaudeCodeAdapter;
use crate::codex::CodexAdapter;
use crate::error::AgentCliError;

/// A named collection of [`AgentCliAdapter`]s.
///
/// A lookup table from a stable
/// provider name (e.g. `claude-code`) to its adapter.
pub struct AdapterRegistry {
    adapters: Vec<Box<dyn AgentCliAdapter + Send + Sync>>,
}

impl AdapterRegistry {
    /// The built-in adapters: Claude Code and Codex.
    #[must_use]
    pub fn builtin() -> Self {
        Self {
            adapters: vec![
                Box::new(ClaudeCodeAdapter::new()),
                Box::new(CodexAdapter::new()),
            ],
        }
    }

    /// Look up an adapter by [`AgentCliAdapter::name`].
    ///
    /// # Errors
    ///
    /// Returns [`AgentCliError::UnknownAdapter`] (listing the registered
    /// names) when no adapter matches.
    pub fn get(&self, name: &str) -> Result<&(dyn AgentCliAdapter + Send + Sync), AgentCliError> {
        self.adapters
            .iter()
            .map(AsRef::as_ref)
            .find(|adapter| adapter.name() == name)
            .ok_or_else(|| AgentCliError::UnknownAdapter {
                name: name.to_string(),
                available: self.names().join(", "),
            })
    }

    /// Names of all registered adapters, in registration order.
    #[must_use]
    pub fn names(&self) -> Vec<&'static str> {
        self.adapters.iter().map(|adapter| adapter.name()).collect()
    }

    /// Run [`AgentCliAdapter::detect`] on every registered adapter.
    ///
    /// Each probe may spawn a short `--version` subprocess, so this is
    /// intended for setup / doctor flows rather than hot paths.
    #[must_use]
    pub fn detect_all(&self) -> Vec<(&'static str, CliProbe)> {
        self.adapters
            .iter()
            .map(|adapter| (adapter.name(), adapter.detect()))
            .collect()
    }
}

impl Default for AdapterRegistry {
    fn default() -> Self {
        Self::builtin()
    }
}

impl std::fmt::Debug for AdapterRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdapterRegistry")
            .field("adapters", &self.names())
            .finish()
    }
}
