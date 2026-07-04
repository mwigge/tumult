//! Runner test suite — shared mocks/helpers plus grouped test submodules.

use super::*;
use crate::controls::LifecycleEvent;
use crate::types::*;
use indexmap::IndexMap;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

mod background_tests;
mod controls_tests;
mod exec_tests;
mod gameday_tests;
mod halt_tests;
mod load_tests;
mod rollback_tests;
mod sampling_tests;

// -- Mock executor

struct MockExecutor {
    success: bool,
    output: Option<String>,
    call_count: Arc<AtomicUsize>,
}

impl MockExecutor {
    fn always_succeed() -> Self {
        Self {
            success: true,
            output: Some("200".into()),
            call_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn always_fail() -> Self {
        Self {
            success: false,
            output: None,
            call_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn with_output(output: &str) -> Self {
        Self {
            success: true,
            output: Some(output.into()),
            call_count: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl ActivityExecutor for MockExecutor {
    fn execute(&self, _activity: &Activity) -> ActivityOutcome {
        self.call_count.fetch_add(1, Ordering::Relaxed);
        ActivityOutcome {
            success: self.success,
            output: self.output.clone(),
            error: if self.success {
                None
            } else {
                Some("execution failed".into())
            },
            duration_ms: 42,
        }
    }
}

// -- Mock control handler

struct EventRecorder {
    events: Arc<std::sync::Mutex<Vec<LifecycleEvent>>>,
}

impl EventRecorder {
    fn new() -> (Self, Arc<std::sync::Mutex<Vec<LifecycleEvent>>>) {
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        (
            Self {
                events: events.clone(),
            },
            events,
        )
    }
}

impl crate::controls::ControlHandler for EventRecorder {
    // Trait returns &str; literal impls appear static but trait sig cannot change
    // because other impls (e.g. CountingHandler) return non-static field refs.
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "event-recorder"
    }
    fn on_event(&self, event: &LifecycleEvent) {
        self.events.lock().unwrap().push(event.clone());
    }
}

// -- Test helpers

fn test_action(name: &str) -> Activity {
    Activity {
        name: name.into(),
        activity_type: ActivityType::Action,
        provider: Provider::Native {
            plugin: "test".into(),
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

fn test_action_background(name: &str) -> Activity {
    Activity {
        name: name.into(),
        activity_type: ActivityType::Action,
        provider: Provider::Native {
            plugin: "test".into(),
            function: "noop".into(),
            arguments: HashMap::new(),
        },
        tolerance: None,
        pause_before_s: None,
        pause_after_s: None,
        background: true,
        label_selector: None,
    }
}

fn test_probe(name: &str) -> Activity {
    Activity {
        name: name.into(),
        activity_type: ActivityType::Probe,
        provider: Provider::Process {
            path: "scripts/health-check.sh".into(),
            arguments: vec![],
            env: HashMap::new(),
            timeout_s: Some(5.0),
        },
        tolerance: Some(Tolerance::Exact {
            value: serde_json::Value::Number(200.into()),
        }),
        pause_before_s: None,
        pause_after_s: None,
        background: false,
        label_selector: None,
    }
}

fn minimal_experiment() -> Experiment {
    Experiment {
        version: "v1".into(),
        title: "Test experiment".into(),
        description: None,
        tags: vec![],
        configuration: IndexMap::new(),
        secrets: IndexMap::new(),
        controls: vec![],
        steady_state_hypothesis: None,
        method: vec![test_action("action-1")],
        rollbacks: vec![],
        estimate: None,
        baseline: None,
        load: None,
        regulatory: None,
        ..Experiment::default()
    }
}

fn experiment_with_hypothesis() -> Experiment {
    let mut exp = minimal_experiment();
    exp.steady_state_hypothesis = Some(Hypothesis {
        title: "System is healthy".into(),
        probes: vec![test_probe("health-check")],
    });
    exp
}

fn default_config() -> RunConfig {
    RunConfig::default()
}

/// Sampling config with tight intervals and timeouts so tests that leave
/// probes failing after the method (deviation scenarios) don't wait out the
/// default 30s post-phase recovery window.
fn fast_sampling() -> SamplingConfig {
    SamplingConfig {
        interval: std::time::Duration::from_millis(10),
        max_during_samples: 50,
        recovery_timeout: std::time::Duration::from_millis(80),
    }
}
