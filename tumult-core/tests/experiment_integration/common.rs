//! Shared test fixtures for the experiment-runner integration tests.
//!
//! Provides mock plugin executors, a lifecycle-event recorder, and builder
//! helpers used across the split test modules.

use std::collections::HashMap;

use indexmap::IndexMap;
use tumult_core::controls::ControlHandler;

// Re-exported so the sibling test modules can `use crate::common::*;`.
pub(crate) use std::sync::Arc;
pub(crate) use tumult_core::controls::{ControlRegistry, LifecycleEvent};
pub(crate) use tumult_core::execution::RollbackStrategy;
pub(crate) use tumult_core::runner::{
    run_experiment, run_experiment_with_sampling, ActivityExecutor, ActivityOutcome, RunConfig,
    SamplingConfig,
};
pub(crate) use tumult_core::types::*;

// ── Mock Plugin Executor ──────────────────────────────────────

/// A configurable mock executor that simulates plugin behavior.
pub(crate) struct MockPlugin {
    /// Map from activity name to (success, output) pairs.
    responses: HashMap<String, (bool, Option<String>)>,
    /// Default response for unknown activities.
    default_success: bool,
    default_output: Option<String>,
    /// Track execution order.
    pub(crate) execution_log: Arc<std::sync::Mutex<Vec<String>>>,
}

impl MockPlugin {
    pub(crate) fn new() -> Self {
        Self {
            responses: HashMap::new(),
            default_success: true,
            default_output: Some("200".into()),
            execution_log: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    pub(crate) fn on(mut self, name: &str, success: bool, output: Option<&str>) -> Self {
        self.responses
            .insert(name.into(), (success, output.map(String::from)));
        self
    }

    pub(crate) fn default_output(mut self, output: &str) -> Self {
        self.default_output = Some(output.into());
        self
    }

    pub(crate) fn default_fail(mut self) -> Self {
        self.default_success = false;
        self.default_output = None;
        self
    }

    #[allow(dead_code)] // Used in integration tests for debugging; kept for test utility
    pub(crate) fn log(&self) -> Vec<String> {
        self.execution_log.lock().unwrap().clone()
    }
}

impl ActivityExecutor for MockPlugin {
    fn execute(&self, activity: &Activity) -> ActivityOutcome {
        self.execution_log
            .lock()
            .unwrap()
            .push(activity.name.clone());

        if let Some((success, output)) = self.responses.get(&activity.name) {
            ActivityOutcome {
                success: *success,
                output: output.clone(),
                error: if *success {
                    None
                } else {
                    Some(format!("{} failed", activity.name))
                },
                duration_ms: 10,
            }
        } else {
            ActivityOutcome {
                success: self.default_success,
                output: self.default_output.clone(),
                error: if self.default_success {
                    None
                } else {
                    Some("default failure".into())
                },
                duration_ms: 10,
            }
        }
    }
}

// ── Event recorder ────────────────────────────────────────────

pub(crate) struct EventLog {
    events: Arc<std::sync::Mutex<Vec<LifecycleEvent>>>,
}

impl EventLog {
    pub(crate) fn new() -> (Self, Arc<std::sync::Mutex<Vec<LifecycleEvent>>>) {
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        (
            Self {
                events: events.clone(),
            },
            events,
        )
    }
}

impl ControlHandler for EventLog {
    // Trait returns &str; literal impl appears static but trait sig stays &str
    // since other impls return non-static field refs.
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "event-log"
    }
    fn on_event(&self, event: &LifecycleEvent) {
        self.events.lock().unwrap().push(event.clone());
    }
}

// ── Test helpers ──────────────────────────────────────────────

pub(crate) fn action(name: &str) -> Activity {
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

pub(crate) fn background_action(name: &str) -> Activity {
    Activity {
        background: true,
        ..action(name)
    }
}

pub(crate) fn probe_with_tolerance(name: &str, expected: serde_json::Value) -> Activity {
    Activity {
        name: name.into(),
        activity_type: ActivityType::Probe,
        provider: Provider::Process {
            path: "scripts/health-check.sh".into(),
            arguments: vec![],
            env: HashMap::new(),
            timeout_s: Some(5.0),
        },
        tolerance: Some(Tolerance::Exact { value: expected }),
        pause_before_s: None,
        pause_after_s: None,
        background: false,
        label_selector: None,
    }
}

pub(crate) fn hypothesis(title: &str, probes: Vec<Activity>) -> Hypothesis {
    Hypothesis {
        title: title.into(),
        probes,
    }
}

/// Sampling config with tight intervals and timeouts so tests that leave
/// probes failing after the method (deviation scenarios) don't wait out the
/// default 30s post-phase recovery window.
pub(crate) fn fast_sampling() -> SamplingConfig {
    SamplingConfig {
        interval: std::time::Duration::from_millis(10),
        max_during_samples: 50,
        recovery_timeout: std::time::Duration::from_millis(80),
    }
}

pub(crate) fn experiment_builder() -> Experiment {
    Experiment {
        version: "v1".into(),
        title: "Integration test experiment".into(),
        description: Some("Tests the full five-phase lifecycle".into()),
        tags: vec!["integration".into(), "test".into()],
        configuration: IndexMap::new(),
        secrets: IndexMap::new(),
        controls: vec![],
        steady_state_hypothesis: None,
        method: vec![action("inject-fault")],
        rollbacks: vec![],
        estimate: None,
        baseline: None,
        load: None,
        regulatory: None,
    }
}
