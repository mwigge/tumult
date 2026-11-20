//! The line that says a run started is logged under the root span of that
//! run. The log exporter stamps a record with the trace that is current
//! when the line is logged, so only then does the line link to its run.

use std::sync::{Arc, Mutex};

use opentelemetry::trace::TraceContextExt;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};
use tumult_core::controls::ControlRegistry;
use tumult_core::runner::{
    run_experiment_with_sampling, ActivityExecutor, ActivityOutcome, RunConfig, SamplingConfig,
};
use tumult_core::types::*;

/// Keeps the message of every event with the trace that was current when
/// it was logged.
#[derive(Default)]
struct TraceOfEachEvent {
    seen: Mutex<Vec<(String, String)>>,
}

#[derive(Default)]
struct Message(String);

impl Visit for Message {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

impl Subscriber for TraceOfEachEvent {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _: &Id, _: &Record<'_>) {}

    fn record_follows_from(&self, _: &Id, _: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut message = Message::default();
        event.record(&mut message);
        let context = opentelemetry::Context::current();
        let span = context.span().span_context().clone();
        let trace = if span.is_valid() {
            span.trace_id().to_string()
        } else {
            String::new()
        };
        self.seen
            .lock()
            .expect("the events seen")
            .push((message.0, trace));
    }

    fn enter(&self, _: &Id) {}

    fn exit(&self, _: &Id) {}
}

/// A target where every step works.
struct Healthy;

impl ActivityExecutor for Healthy {
    fn execute(&self, _: &Activity) -> ActivityOutcome {
        ActivityOutcome {
            success: true,
            output: Some("ok".into()),
            error: None,
            duration_ms: 1,
        }
    }
}

fn one_action() -> Experiment {
    Experiment {
        title: "started log".into(),
        method: vec![Activity {
            name: "fault".into(),
            activity_type: ActivityType::Action,
            provider: Provider::Native {
                plugin: "test".into(),
                function: "noop".into(),
                arguments: std::collections::HashMap::new(),
            },
            ..Default::default()
        }],
        ..Default::default()
    }
}

#[test]
fn the_lines_that_a_run_started_and_ended_are_logged_under_the_trace_of_that_run() {
    tumult_test_utils::install_recording_tracer();
    let events = Arc::new(TraceOfEachEvent::default());
    let executor: Arc<dyn ActivityExecutor> = Arc::new(Healthy);

    let journal = tracing::subscriber::with_default(Arc::clone(&events), || {
        run_experiment_with_sampling(
            &one_action(),
            &executor,
            &Arc::new(ControlRegistry::new()),
            &RunConfig::default(),
            &SamplingConfig::default(),
        )
        .expect("run")
    });

    let trace_of_the_run = journal.method_results[0].trace_id.0.clone();
    assert!(!trace_of_the_run.is_empty());
    let seen = events.seen.lock().expect("the events seen");
    for line in ["experiment.started", "experiment.completed"] {
        let traces: Vec<&str> = seen
            .iter()
            .filter(|(message, _)| message == line)
            .map(|(_, trace)| trace.as_str())
            .collect();
        assert_eq!(traces, [trace_of_the_run.as_str()], "{line}");
    }
}
