//! Descriptors of the actions and probes a plugin offers.

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
