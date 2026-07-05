//! Reusable JSON Schema fragment builders shared by the output-schema table.

use std::collections::BTreeMap;

use rust_mcp_sdk::schema::ToolOutputSchema;
use serde_json::{json, Map, Value};

/// Schema for a `ChaosGraph` node summary (`{id, kind, label}`).
pub(super) fn graph_node_summary_schema() -> Value {
    json!({
        "type": "object",
        "required": ["id", "kind", "label"],
        "properties": {
            "id": { "type": "string" },
            "kind": {
                "type": "string",
                "enum": ["experiment", "fault", "service", "journal", "deviation", "compliance_article", "coverage_gap", "fault_domain"],
            },
            "label": { "type": "string" },
        },
    })
}

/// Compact schema for `tumult_core::types::Journal` (`snake_case` serde
/// serialization). Nested result payloads that are not stable API surface
/// are documented as opaque nullable objects.
pub(super) fn journal_schema() -> Value {
    json!({
        "type": "object",
        "description": "Tumult experiment journal (tumult_core::types::Journal).",
        "required": [
            "experiment_title", "experiment_id", "status",
            "started_at_ns", "ended_at_ns", "duration_ms",
            "method_results", "rollback_results",
        ],
        "properties": {
            "experiment_title": { "type": "string" },
            "experiment_id": { "type": "string" },
            "status": {
                "type": "string",
                "enum": ["completed", "deviated", "aborted", "failed", "interrupted"],
            },
            "started_at_ns": { "type": "integer" },
            "ended_at_ns": { "type": "integer" },
            "duration_ms": { "type": "integer" },
            "steady_state_before": hypothesis_schema(),
            "steady_state_after": hypothesis_schema(),
            "method_results": { "type": "array", "items": activity_result_schema() },
            "rollback_results": { "type": "array", "items": activity_result_schema() },
            "rollback_failures": { "type": "integer" },
            "estimate": opaque_nullable_object("Pre-run outcome estimate, if declared."),
            "baseline_result": opaque_nullable_object("Baseline phase result, if run."),
            "during_result": opaque_nullable_object("During-phase result, if run."),
            "post_result": opaque_nullable_object("Post-phase result, if run."),
            "load_result": opaque_nullable_object("Load-generation result, if run."),
            "analysis": opaque_nullable_object("Statistical analysis result, if computed."),
            "regulatory": opaque_nullable_object("Regulatory/compliance mapping, if declared."),
        },
    })
}

/// Schema for the compact summary object returned by `tumult_read_journal`.
pub(super) fn journal_summary_schema() -> Value {
    json!({
        "type": "object",
        "required": [
            "experiment_title", "experiment_id", "status", "started_at_ns",
            "duration_ms", "method_count", "rollback_count", "rollback_failures",
        ],
        "properties": {
            "experiment_title": { "type": "string" },
            "experiment_id": { "type": "string" },
            "status": {
                "type": "string",
                "enum": ["completed", "deviated", "aborted", "failed", "interrupted"],
            },
            "started_at_ns": { "type": "integer" },
            "duration_ms": { "type": "integer" },
            "method_count": { "type": "integer" },
            "rollback_count": { "type": "integer" },
            "rollback_failures": { "type": "integer" },
        },
    })
}

/// Schema for `tumult_core::types::ActivityResult`. `output` and `error`
/// are opaque free-form provider text, omitted when absent.
fn activity_result_schema() -> Value {
    json!({
        "type": "object",
        "required": [
            "name", "activity_type", "status",
            "started_at_ns", "duration_ms", "trace_id", "span_id",
        ],
        "properties": {
            "name": { "type": "string" },
            "activity_type": { "type": "string", "enum": ["action", "probe"] },
            "status": { "type": "string", "enum": ["succeeded", "failed", "timeout", "skipped"] },
            "started_at_ns": { "type": "integer" },
            "duration_ms": { "type": "integer" },
            "output": {
                "type": "string",
                "description": "Opaque free-form activity output (raw provider text); omitted when absent.",
            },
            "error": {
                "type": "string",
                "description": "Opaque free-form error text; omitted when absent.",
            },
            "trace_id": { "type": "string" },
            "span_id": { "type": "string" },
        },
    })
}

fn hypothesis_schema() -> Value {
    json!({
        "type": ["object", "null"],
        "description": "Steady-state hypothesis result; null when the experiment declares none.",
        "properties": {
            "title": { "type": "string" },
            "met": { "type": "boolean" },
            "probe_results": { "type": "array", "items": activity_result_schema() },
        },
    })
}

fn opaque_nullable_object(description: &str) -> Value {
    json!({ "type": ["object", "null"], "description": description })
}

/// Build a [`ToolOutputSchema`] from required property names and a JSON
/// object mapping property names to their schemas.
pub(super) fn schema_object(required: &[&str], properties: Value) -> ToolOutputSchema {
    let Value::Object(props) = properties else {
        unreachable!("schema properties are built as a JSON object literal");
    };
    let properties: BTreeMap<String, Map<String, Value>> = props
        .into_iter()
        .map(|(name, schema)| {
            let Value::Object(schema) = schema else {
                unreachable!("each property schema is a JSON object literal");
            };
            (name, schema)
        })
        .collect();
    ToolOutputSchema::new(
        required.iter().map(ToString::to_string).collect(),
        Some(properties),
        None,
    )
}
