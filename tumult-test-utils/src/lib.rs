//! Shared test helpers for tumult workspace integration tests.
//!
//! This crate is a **test-only** utility published exclusively as a
//! `[dev-dependency]`. It provides mock executor implementations and
//! experiment builder helpers used across multiple integration test suites.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use indexmap::IndexMap;
use tumult_core::controls::{ControlHandler, LifecycleEvent};
use tumult_core::runner::{ActivityExecutor, ActivityOutcome};
use tumult_core::types::{
    Activity, ActivityType, Experiment, HttpMethod, Hypothesis, Provider, Tolerance,
};

// ── MockPlugin ────────────────────────────────────────────────

/// A configurable mock executor that simulates plugin behaviour for tests.
///
/// Responses can be configured per activity name; a default response is used
/// for any activity that has not been explicitly registered.
pub struct MockPlugin {
    /// Per-name responses: `(success, optional_output)`.
    responses: HashMap<String, (bool, Option<String>)>,
    /// Default success flag for unregistered activities.
    default_success: bool,
    /// Default output for unregistered activities.
    default_output: Option<String>,
    /// Tracks the order in which activities were executed.
    execution_log: Arc<Mutex<Vec<String>>>,
}

impl MockPlugin {
    /// Creates a new `MockPlugin` that succeeds with output `"200"` by default.
    #[must_use]
    pub fn new() -> Self {
        Self {
            responses: HashMap::new(),
            default_success: true,
            default_output: Some("200".into()),
            execution_log: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Registers a specific response for an activity named `name`.
    #[must_use]
    pub fn on(mut self, name: &str, success: bool, output: Option<&str>) -> Self {
        self.responses
            .insert(name.into(), (success, output.map(String::from)));
        self
    }

    /// Overrides the default output returned for unregistered activities.
    #[must_use]
    pub fn default_output(mut self, output: &str) -> Self {
        self.default_output = Some(output.into());
        self
    }

    /// Configures the mock to fail by default for unregistered activities.
    #[must_use]
    pub fn default_fail(mut self) -> Self {
        self.default_success = false;
        self.default_output = None;
        self
    }

    /// Returns a cloneable handle to the shared execution log.
    ///
    /// Clone the handle before moving `MockPlugin` into an `Arc` to observe
    /// which activities ran after experiment execution completes.
    ///
    /// # Panics
    ///
    /// Lock access on the returned handle panics if the mutex is poisoned.
    #[must_use]
    pub fn execution_log_handle(&self) -> Arc<Mutex<Vec<String>>> {
        Arc::clone(&self.execution_log)
    }

    /// Returns a snapshot of the execution log (activity names in order).
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    #[must_use]
    pub fn log(&self) -> Vec<String> {
        self.execution_log
            .lock()
            .expect("execution_log mutex poisoned")
            .clone()
    }
}

impl Default for MockPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl ActivityExecutor for MockPlugin {
    fn execute(&self, activity: &Activity) -> ActivityOutcome {
        self.execution_log
            .lock()
            .expect("execution_log mutex poisoned")
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

// ── EventLog ──────────────────────────────────────────────────

/// A `ControlHandler` that records all `LifecycleEvent`s for test assertions.
pub struct EventLog {
    events: Arc<Mutex<Vec<LifecycleEvent>>>,
}

impl EventLog {
    /// Creates a new `EventLog`.
    ///
    /// Returns both the handler (to register with a `ControlRegistry`) and a
    /// shared handle to the recorded events.
    #[must_use]
    pub fn new() -> (Self, Arc<Mutex<Vec<LifecycleEvent>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                events: Arc::clone(&events),
            },
            events,
        )
    }
}

impl ControlHandler for EventLog {
    #[allow(clippy::unnecessary_literal_bound)] // trait bound requires &str
    fn name(&self) -> &str {
        "event-log"
    }

    fn on_event(&self, event: &LifecycleEvent) {
        self.events
            .lock()
            .expect("events mutex poisoned")
            .push(event.clone());
    }
}

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

/// Builds a foreground action `Activity` backed by a native mock plugin.
///
/// This is an alias for [`action`] with an explicit name to improve readability
/// in tests that mix foreground and background activities side-by-side.
#[must_use]
pub fn foreground_action(name: &str) -> Activity {
    action(name)
}

/// Builds a probe `Activity` that checks for an exact HTTP response value.
#[must_use]
pub fn probe_with_tolerance(name: &str, expected: serde_json::Value) -> Activity {
    Activity {
        name: name.into(),
        activity_type: ActivityType::Probe,
        provider: Provider::Http {
            method: HttpMethod::Get,
            url: "http://localhost/health".into(),
            headers: HashMap::new(),
            body: None,
            timeout_s: Some(5.0),
        },
        tolerance: Some(Tolerance::Exact { value: expected }),
        pause_before_s: None,
        pause_after_s: None,
        background: false,
        label_selector: None,
    }
}

/// Builds a `Hypothesis` with a title and a set of probes.
#[must_use]
pub fn hypothesis(title: &str, probes: Vec<Activity>) -> Hypothesis {
    Hypothesis {
        title: title.into(),
        probes,
    }
}

// ── Experiment helpers ────────────────────────────────────────

/// Builds a skeleton `Experiment` with a single `inject-fault` action.
///
/// Callers can override individual fields after construction.
#[must_use]
pub fn experiment_builder() -> Experiment {
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
    }
}
