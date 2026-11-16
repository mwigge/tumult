//! The log events a piece of code emits, read back as data.

use std::collections::BTreeMap;

use serde_json::Value;
use tumult_otel::logging::testing::capture_json;

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

    /// Reads one captured JSON line.
    fn from_line(line: &Value) -> Self {
        let fields = line.as_object().into_iter().flatten();
        Self {
            event: text(&line["message"]),
            fields: fields
                .map(|(name, value)| (name.clone(), text(value)))
                .collect(),
        }
    }
}

/// A JSON value as the text a reader of the log line sees.
fn text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Everything `run` logs on this thread.
pub(crate) fn logged(run: impl FnOnce()) -> Vec<Logged> {
    let captured = capture_json(tracing::Level::TRACE, run);
    captured.events.iter().map(Logged::from_line).collect()
}
