//! Response mapping: the outcome data types the panel renders and the pure
//! parsers that project each `tools/call` result into them. Every parser is
//! unit-tested against canned JSON — no live server required.

mod outcomes;
mod structured;
mod text;

pub use outcomes::*;
pub use structured::*;
pub use text::*;

use serde_json::Value;

/// Read a string field from a JSON object, defaulting to empty.
fn str_field(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Find the first line beginning with `label` and parse the remainder as a
/// count (e.g. `Plugins: 12` with label `Plugins:` → `12`).
fn labeled_count(text: &str, label: &str) -> Option<usize> {
    text.lines().find_map(|line| {
        line.trim()
            .strip_prefix(label)
            .and_then(|rest| rest.trim().parse::<usize>().ok())
    })
}

/// Parse the whitespace-delimited number immediately preceding `suffix`
/// (e.g. `… 3 method steps …` with suffix `method step` → `3`).
fn number_before(text: &str, suffix: &str) -> Option<usize> {
    let idx = text.find(suffix)?;
    text[..idx]
        .split_whitespace()
        .next_back()
        .and_then(|tok| tok.parse::<usize>().ok())
}

/// First text block from a `content` array, if any.
fn content_text(result: &Value) -> Option<String> {
    result
        .get("content")
        .and_then(Value::as_array)
        .and_then(|c| c.iter().find_map(|b| b.get("text").and_then(Value::as_str)))
        .map(ToString::to_string)
}
