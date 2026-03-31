//! Controls — lifecycle hooks for cross-cutting concerns.
//!
//! Controls hook into the experiment lifecycle at defined points:
//! before/after experiment, before/after method, before/after each activity.
//! They are used for logging, tracing, safeguards, and custom integrations.

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

/// A control handler that receives lifecycle events.
pub trait ControlHandler: Send + Sync {
    fn name(&self) -> &str;
    fn on_event(&self, event: &LifecycleEvent);
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
    pub fn emit(&self, event: &LifecycleEvent) {
        for handler in &self.handlers {
            handler.on_event(event);
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
