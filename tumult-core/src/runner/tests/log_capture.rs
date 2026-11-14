//! The log events a piece of code emits, read back as data.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use tracing::field::{Field, Visit};
use tracing::span;

/// One event: its name (the message) and its fields as text.
#[derive(Debug, Default, Clone)]
pub(crate) struct Logged {
    pub(crate) event: String,
    pub(crate) fields: BTreeMap<String, String>,
}

impl Logged {
    pub(crate) fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(String::as_str)
    }
}

impl Visit for Logged {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let text = format!("{value:?}");
        if field.name() == "message" {
            self.event = text;
            return;
        }
        self.fields.insert(field.name().to_string(), text);
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }
}

struct Capture(Arc<Mutex<Vec<Logged>>>);

impl tracing::Subscriber for Capture {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _: &span::Attributes<'_>) -> span::Id {
        span::Id::from_u64(1)
    }

    fn record(&self, _: &span::Id, _: &span::Record<'_>) {}

    fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut logged = Logged::default();
        event.record(&mut logged);
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(logged);
    }

    fn enter(&self, _: &span::Id) {}

    fn exit(&self, _: &span::Id) {}
}

/// Everything `run` logs on this thread.
pub(crate) fn logged(run: impl FnOnce()) -> Vec<Logged> {
    let events = Arc::new(Mutex::new(Vec::new()));
    tracing::subscriber::with_default(Capture(Arc::clone(&events)), run);
    let events = events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    events.clone()
}
