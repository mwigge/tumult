//! Experiment and activity builders for runner tests.

use std::collections::HashMap;

use indexmap::IndexMap;
use tumult_core::types::{Activity, ActivityType, Experiment, Provider};

// ── Activity helpers ──────────────────────────────────────────

/// Builds a foreground action `Activity` backed by a native mock plugin.
#[must_use]
pub fn action(name: &str) -> Activity {
    Activity {
        name: name.into(),
        activity_type: ActivityType::Action,
        provider: Provider::Native {
            plugin: "mock".into(),
            function: "noop".into(),
            arguments: HashMap::new(),
        },
        tolerance: None,
        pause_before_s: None,
        pause_after_s: None,
        background: false,
        label_selector: None,
    }
}

/// Builds a background action `Activity` backed by a native mock plugin.
#[must_use]
pub fn background_action(name: &str) -> Activity {
    Activity {
        background: true,
        ..action(name)
    }
}

// ── Experiment helpers ────────────────────────────────────────

/// Builds a minimal `Experiment` with no hypothesis, no rollbacks, and the
/// supplied `method` activities.
#[must_use]
pub fn minimal_experiment(method: Vec<Activity>) -> Experiment {
    Experiment {
        version: "v1".into(),
        title: "minimal test experiment".into(),
        description: None,
        tags: vec![],
        configuration: IndexMap::new(),
        secrets: IndexMap::new(),
        controls: vec![],
        steady_state_hypothesis: None,
        method,
        rollbacks: vec![],
        estimate: None,
        baseline: None,
        load: None,
        regulatory: None,
        ..Experiment::default()
    }
}
