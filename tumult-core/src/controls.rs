//! Controls — lifecycle hooks for cross-cutting concerns.
//!
//! Controls hook into the experiment lifecycle at defined points:
//! before/after experiment, before/after method, before/after each activity.
//! They are used for logging, tracing, safeguards, and custom integrations.

use std::sync::Arc;

use crate::runner::ActivityExecutor;
use crate::types::{Activity, ActivityType, Control, Provider};

/// Lifecycle event that a control can observe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleEvent {
    BeforeExperiment,
    AfterExperiment,
    BeforeMethod,
    AfterMethod,
    BeforeActivity { name: String },
    AfterActivity { name: String },
    BeforeRollback,
    AfterRollback,
    BeforeHypothesis,
    AfterHypothesis,
}

impl LifecycleEvent {
    /// Stable machine name for the event (`before_experiment`,
    /// `after_activity`, …). Declared controls receive it as
    /// `TUMULT_CONTROL_EVENT` so the hook can decide whether to act on a
    /// given event.
    #[must_use]
    pub fn event_name(&self) -> &'static str {
        match self {
            Self::BeforeExperiment => "before_experiment",
            Self::AfterExperiment => "after_experiment",
            Self::BeforeMethod => "before_method",
            Self::AfterMethod => "after_method",
            Self::BeforeActivity { .. } => "before_activity",
            Self::AfterActivity { .. } => "after_activity",
            Self::BeforeRollback => "before_rollback",
            Self::AfterRollback => "after_rollback",
            Self::BeforeHypothesis => "before_hypothesis",
            Self::AfterHypothesis => "after_hypothesis",
        }
    }

    /// The activity name carried by `BeforeActivity`/`AfterActivity` events;
    /// `None` for all other events. Declared controls receive it as
    /// `TUMULT_CONTROL_ACTIVITY`.
    #[must_use]
    pub fn activity_name(&self) -> Option<&str> {
        match self {
            Self::BeforeActivity { name } | Self::AfterActivity { name } => Some(name),
            _ => None,
        }
    }
}

/// A control handler that receives lifecycle events.
pub trait ControlHandler: Send + Sync {
    fn name(&self) -> &str;
    fn on_event(&self, event: &LifecycleEvent);
}

/// A control declared in the experiment's `controls:` section, executed by
/// the run at every lifecycle event.
///
/// The schema carries no event scoping, so the control's provider is invoked
/// once per event with the event identity injected, and the hook decides
/// whether to act:
///
/// * `process` providers receive `TUMULT_CONTROL_EVENT` (plus
///   `TUMULT_CONTROL_ACTIVITY` for activity events) as environment variables;
/// * `script` providers receive them as `control_event` / `control_activity`
///   arguments, which the script executor exports as the same `TUMULT_*`
///   environment variables;
/// * `native` providers receive them as `control_event` / `control_activity`
///   entries in the arguments map.
///
/// Entries already present in the declared provider win over the injected
/// ones. A failing control is logged and never aborts the run — the registry
/// contains panics, and a non-success outcome is reported via `tracing`.
pub struct ProviderControl {
    control: Control,
    executor: Arc<dyn ActivityExecutor>,
}

impl ProviderControl {
    #[must_use]
    pub fn new(control: Control, executor: Arc<dyn ActivityExecutor>) -> Self {
        Self { control, executor }
    }

    /// Build the activity-shaped invocation for one event: the declared
    /// provider plus the injected event identity.
    fn invocation(&self, event: &LifecycleEvent) -> Activity {
        let provider = match &self.control.provider {
            Provider::Process {
                path,
                arguments,
                env,
                timeout_s,
            } => {
                let mut env = env.clone();
                env.entry("TUMULT_CONTROL_EVENT".to_string())
                    .or_insert_with(|| event.event_name().to_string());
                if let Some(name) = event.activity_name() {
                    env.entry("TUMULT_CONTROL_ACTIVITY".to_string())
                        .or_insert_with(|| name.to_string());
                }
                Provider::Process {
                    path: path.clone(),
                    arguments: arguments.clone(),
                    env,
                    timeout_s: *timeout_s,
                }
            }
            Provider::Script {
                plugin,
                function,
                arguments,
                timeout_s,
            } => {
                let mut arguments = arguments.clone();
                arguments
                    .entry("control_event".to_string())
                    .or_insert_with(|| event.event_name().into());
                if let Some(name) = event.activity_name() {
                    arguments
                        .entry("control_activity".to_string())
                        .or_insert_with(|| name.into());
                }
                Provider::Script {
                    plugin: plugin.clone(),
                    function: function.clone(),
                    arguments,
                    timeout_s: *timeout_s,
                }
            }
            Provider::Native {
                plugin,
                function,
                arguments,
            } => {
                let mut arguments = arguments.clone();
                arguments
                    .entry("control_event".to_string())
                    .or_insert_with(|| event.event_name().into());
                if let Some(name) = event.activity_name() {
                    arguments
                        .entry("control_activity".to_string())
                        .or_insert_with(|| name.into());
                }
                Provider::Native {
                    plugin: plugin.clone(),
                    function: function.clone(),
                    arguments,
                }
            }
        };
        Activity {
            name: format!("control:{}", self.control.name),
            activity_type: ActivityType::Action,
            provider,
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
            label_selector: None,
        }
    }
}

impl ControlHandler for ProviderControl {
    fn name(&self) -> &str {
        &self.control.name
    }

    fn on_event(&self, event: &LifecycleEvent) {
        let outcome = self.executor.execute(&self.invocation(event));
        if !outcome.success {
            tracing::error!(
                control = %self.control.name,
                event = %event.event_name(),
                error = outcome.error.as_deref().unwrap_or("unknown error"),
                "declared control failed; continuing run"
            );
        }
    }
}

/// Extract a human-readable message from a caught panic payload.
pub(crate) fn panic_message(panic: &(dyn std::any::Any + Send + 'static)) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

/// Registry of control handlers.
pub struct ControlRegistry {
    handlers: Vec<Box<dyn ControlHandler>>,
}

impl ControlRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            handlers: Vec::new(),
        }
    }

    pub fn register(&mut self, handler: Box<dyn ControlHandler>) {
        self.handlers.push(handler);
    }

    /// Emit an event to all registered handlers.
    ///
    /// A panicking handler is contained at this boundary: the panic is
    /// caught, logged, and the remaining handlers still receive the event —
    /// a broken control must never abort a run or skip later handlers.
    pub fn emit(&self, event: &LifecycleEvent) {
        for handler in &self.handlers {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                handler.on_event(event);
            }));
            if let Err(panic) = result {
                tracing::error!(
                    control = handler.name(),
                    event = ?event,
                    panic = %panic_message(&*panic),
                    "control handler panicked; continuing with remaining handlers"
                );
            }
        }
    }

    #[must_use]
    pub fn handler_count(&self) -> usize {
        self.handlers.len()
    }

    #[must_use]
    pub fn handler_names(&self) -> Vec<&str> {
        self.handlers.iter().map(|h| h.name()).collect()
    }
}

impl Default for ControlRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct CountingHandler {
        name: String,
        count: Arc<AtomicUsize>,
    }

    impl ControlHandler for CountingHandler {
        fn name(&self) -> &str {
            &self.name
        }
        fn on_event(&self, _event: &LifecycleEvent) {
            self.count.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn empty_registry_has_no_handlers() {
        let registry = ControlRegistry::new();
        assert_eq!(registry.handler_count(), 0);
    }

    #[test]
    fn register_and_count_handlers() {
        let mut registry = ControlRegistry::new();
        let count = Arc::new(AtomicUsize::new(0));
        registry.register(Box::new(CountingHandler {
            name: "logger".into(),
            count: count.clone(),
        }));
        registry.register(Box::new(CountingHandler {
            name: "tracer".into(),
            count: count.clone(),
        }));
        assert_eq!(registry.handler_count(), 2);
        assert_eq!(registry.handler_names(), vec!["logger", "tracer"]);
    }

    #[test]
    fn emit_calls_all_handlers() {
        let mut registry = ControlRegistry::new();
        let count1 = Arc::new(AtomicUsize::new(0));
        let count2 = Arc::new(AtomicUsize::new(0));
        registry.register(Box::new(CountingHandler {
            name: "h1".into(),
            count: count1.clone(),
        }));
        registry.register(Box::new(CountingHandler {
            name: "h2".into(),
            count: count2.clone(),
        }));

        registry.emit(&LifecycleEvent::BeforeExperiment);
        assert_eq!(count1.load(Ordering::Relaxed), 1);
        assert_eq!(count2.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn emit_multiple_events() {
        let mut registry = ControlRegistry::new();
        let count = Arc::new(AtomicUsize::new(0));
        registry.register(Box::new(CountingHandler {
            name: "counter".into(),
            count: count.clone(),
        }));

        registry.emit(&LifecycleEvent::BeforeExperiment);
        registry.emit(&LifecycleEvent::BeforeMethod);
        registry.emit(&LifecycleEvent::BeforeActivity {
            name: "kill-pod".into(),
        });
        registry.emit(&LifecycleEvent::AfterActivity {
            name: "kill-pod".into(),
        });
        registry.emit(&LifecycleEvent::AfterMethod);
        registry.emit(&LifecycleEvent::AfterExperiment);

        assert_eq!(count.load(Ordering::Relaxed), 6);
    }

    #[test]
    fn emit_to_empty_registry_does_not_panic() {
        let registry = ControlRegistry::new();
        registry.emit(&LifecycleEvent::BeforeExperiment);
    }

    #[test]
    fn panicking_handler_is_contained_and_remaining_handlers_still_receive_events() {
        struct PanicHandler;
        impl ControlHandler for PanicHandler {
            fn name(&self) -> &'static str {
                "panicker"
            }
            fn on_event(&self, _event: &LifecycleEvent) {
                panic!("deliberate control panic");
            }
        }

        let mut registry = ControlRegistry::new();
        registry.register(Box::new(PanicHandler));
        let count = Arc::new(AtomicUsize::new(0));
        registry.register(Box::new(CountingHandler {
            name: "counter".into(),
            count: count.clone(),
        }));

        registry.emit(&LifecycleEvent::BeforeExperiment);

        assert_eq!(
            count.load(Ordering::Relaxed),
            1,
            "a panicking handler must not skip or abort the remaining handlers"
        );
    }

    // ── ProviderControl (declared experiment controls) ─────────

    use crate::runner::ActivityOutcome;
    use crate::types::Control;
    use std::sync::Mutex;

    /// Executor that records every activity it is asked to run.
    struct RecordingExecutor {
        activities: Arc<Mutex<Vec<crate::types::Activity>>>,
        success: bool,
    }

    impl crate::runner::ActivityExecutor for RecordingExecutor {
        fn execute(&self, activity: &crate::types::Activity) -> ActivityOutcome {
            self.activities.lock().unwrap().push(activity.clone());
            ActivityOutcome {
                success: self.success,
                output: None,
                error: if self.success {
                    None
                } else {
                    Some("boom".into())
                },
                duration_ms: 1,
            }
        }
    }

    fn process_control(env: std::collections::HashMap<String, String>) -> Control {
        Control {
            name: "notify".into(),
            provider: crate::types::Provider::Process {
                path: "notify.sh".into(),
                arguments: vec![],
                env,
                timeout_s: Some(5.0),
            },
        }
    }

    #[test]
    fn declared_control_executes_on_every_event_with_event_identity() {
        let activities = Arc::new(Mutex::new(Vec::new()));
        let executor: Arc<dyn crate::runner::ActivityExecutor> = Arc::new(RecordingExecutor {
            activities: activities.clone(),
            success: true,
        });

        let mut registry = ControlRegistry::new();
        registry.register(Box::new(ProviderControl::new(
            process_control(std::collections::HashMap::new()),
            executor,
        )));

        registry.emit(&LifecycleEvent::BeforeExperiment);
        registry.emit(&LifecycleEvent::BeforeActivity {
            name: "kill-pod".into(),
        });

        let recorded = activities.lock().unwrap();
        assert_eq!(recorded.len(), 2);
        let crate::types::Provider::Process { env, .. } = &recorded[0].provider else {
            panic!("expected process provider");
        };
        assert_eq!(
            env.get("TUMULT_CONTROL_EVENT").unwrap(),
            "before_experiment"
        );
        assert!(!env.contains_key("TUMULT_CONTROL_ACTIVITY"));
        let crate::types::Provider::Process { env, .. } = &recorded[1].provider else {
            panic!("expected process provider");
        };
        assert_eq!(env.get("TUMULT_CONTROL_EVENT").unwrap(), "before_activity");
        assert_eq!(env.get("TUMULT_CONTROL_ACTIVITY").unwrap(), "kill-pod");
    }

    #[test]
    fn declared_env_entries_win_over_injected_event_identity() {
        let activities = Arc::new(Mutex::new(Vec::new()));
        let executor: Arc<dyn crate::runner::ActivityExecutor> = Arc::new(RecordingExecutor {
            activities: activities.clone(),
            success: true,
        });
        let control = process_control(std::collections::HashMap::from([(
            "TUMULT_CONTROL_EVENT".to_string(),
            "declared".to_string(),
        )]));

        let mut registry = ControlRegistry::new();
        registry.register(Box::new(ProviderControl::new(control, executor)));
        registry.emit(&LifecycleEvent::AfterExperiment);

        let recorded = activities.lock().unwrap();
        let crate::types::Provider::Process { env, .. } = &recorded[0].provider else {
            panic!("expected process provider");
        };
        assert_eq!(env.get("TUMULT_CONTROL_EVENT").unwrap(), "declared");
    }

    #[test]
    fn failing_control_does_not_panic_or_skip_other_handlers() {
        let executor: Arc<dyn crate::runner::ActivityExecutor> = Arc::new(RecordingExecutor {
            activities: Arc::new(Mutex::new(Vec::new())),
            success: false,
        });
        let mut registry = ControlRegistry::new();
        registry.register(Box::new(ProviderControl::new(
            process_control(std::collections::HashMap::new()),
            executor,
        )));
        let count = Arc::new(AtomicUsize::new(0));
        registry.register(Box::new(CountingHandler {
            name: "counter".into(),
            count: count.clone(),
        }));

        registry.emit(&LifecycleEvent::BeforeMethod);
        assert_eq!(count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn event_names_and_activity_names_are_stable() {
        assert_eq!(
            LifecycleEvent::BeforeExperiment.event_name(),
            "before_experiment"
        );
        assert_eq!(
            LifecycleEvent::AfterActivity { name: "a".into() }.event_name(),
            "after_activity"
        );
        assert_eq!(
            LifecycleEvent::AfterActivity { name: "a".into() }.activity_name(),
            Some("a")
        );
        assert_eq!(LifecycleEvent::BeforeMethod.activity_name(), None);
    }

    #[test]
    fn lifecycle_events_are_distinct() {
        let events = [
            LifecycleEvent::BeforeExperiment,
            LifecycleEvent::AfterExperiment,
            LifecycleEvent::BeforeMethod,
            LifecycleEvent::AfterMethod,
            LifecycleEvent::BeforeRollback,
            LifecycleEvent::AfterRollback,
            LifecycleEvent::BeforeHypothesis,
            LifecycleEvent::AfterHypothesis,
        ];
        // All events should be different from each other
        for (i, a) in events.iter().enumerate() {
            for (j, b) in events.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b);
                }
            }
        }
    }
}
