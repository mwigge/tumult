//! Output schemas advertised for tools that return structured content.
//!
//! The `#[mcp_tool]` macro always emits `output_schema: None`, so
//! `handle_list_tools_request` patches each generated [`Tool`] with the
//! schema returned by [`output_schema_for`]. Schemas are hand-written,
//! compact JSON Schemas derived from the serde types that produce the
//! structured content (`tumult_core::types::Journal` et al.). Free-form
//! fields (`ActivityResult::output` / `error`) are documented as opaque
//! strings.

use std::collections::BTreeMap;

use rust_mcp_sdk::schema::ToolOutputSchema;
use serde_json::{json, Map, Value};

/// Names of every tool that sets `structured_content` on its results and
/// therefore advertises an output schema. Test-only: the runtime source of
/// truth is [`output_schema_for`].
#[cfg(test)]
pub(crate) const STRUCTURED_TOOLS: &[&str] = &[
    "tumult_run_experiment",
    "tumult_read_journal",
    "tumult_report",
    "tumult_compliance",
    "tumult_trend",
    "tumult_gameday_create",
    "tumult_agents",
    "tumult_recommend",
    "tumult_store_stats",
    "tumult_coverage",
    "tumult_agentic_list_scenarios",
    "tumult_agentic_smoke",
    "tumult_agentic_run_experiment",
    "tumult_list_journals",
    "tumult_list_experiments",
    "tumult_gameday_list",
    "tumult_chaosgraph_query",
    "tumult_chaosgraph_neighbors",
    "tumult_chaosgraph_coverage_gaps",
    "tumult_fault_catalog",
    "tumult_scaffold_experiment",
];

/// Returns the output schema for `tool_name`, or `None` for tools that only
/// return unstructured text.
#[allow(clippy::too_many_lines)] // One schema literal per structured tool; splitting per-tool helpers would not reduce the logical complexity
pub(crate) fn output_schema_for(tool_name: &str) -> Option<ToolOutputSchema> {
    match tool_name {
        "tumult_run_experiment" => Some(schema_object(
            &["journal", "journal_path", "ingestion"],
            json!({
                "journal": journal_schema(),
                "journal_path": {
                    "type": "string",
                    "description": "Filesystem path the journal was written to."
                },
                "ingestion": {
                    "type": "string",
                    "description": "Analytics-store ingestion outcome: 'ingested', 'duplicate', 'skipped', or 'failed: <reason>'."
                },
            }),
        )),
        "tumult_read_journal" => Some(schema_object(
            &["summary"],
            json!({
                "summary": journal_summary_schema(),
                "journal": journal_schema(),
            }),
        )),
        "tumult_report" => Some(schema_object(
            &["format"],
            json!({
                "format": { "type": "string", "enum": ["json", "junit"] },
                "output_path": {
                    "type": "string",
                    "description": "Filesystem path the report was written to; present only when output_path was given.",
                },
                "content": {
                    "type": "string",
                    "description": "Inline report content (capped at 512 KiB); present only when no output_path was given.",
                },
            }),
        )),
        "tumult_compliance" => Some(schema_object(
            &[
                "framework",
                "pass_rate",
                "recovery_compliance",
                "verdict",
                "journals_evaluated",
            ],
            json!({
                "framework": {
                    "type": "string",
                    "description": "Canonical framework report identifier (e.g. 'DORA').",
                },
                "pass_rate": { "type": "number", "description": "Fraction of journals that completed (0.0-1.0)." },
                "recovery_compliance": {
                    "type": ["number", "null"],
                    "description": "Recovery-compliance proxy (0.0-1.0); null when no MTTR or resilience_score data exists (pass-rate-only verdict).",
                },
                "verdict": {
                    "type": "string",
                    "description": "Evidence-strength token (COMPLIANT / PARTIAL / NON-COMPLIANT, with a '(pass-rate only)' suffix when recovery data is absent). Denotes strength of EVIDENCE toward controls, NOT a compliance attestation.",
                },
                "journals_evaluated": { "type": "integer" },
                "disclaimer": {
                    "type": "string",
                    "description": "Scope disclaimer: experiments produce evidence toward controls, not a compliance determination.",
                },
                "source_url": {
                    "type": "string",
                    "description": "Primary official source URL for the framework.",
                },
                "citations": {
                    "type": "array",
                    "description": "Sourced, dated control citations from the registry (single source of truth shared with the CLI).",
                    "items": {
                        "type": "object",
                        "required": ["control_id", "title", "requires", "evidence_type", "strength", "evidence_note", "source_url", "last_verified"],
                        "properties": {
                            "control_id": { "type": "string" },
                            "title": { "type": "string" },
                            "requires": { "type": "string", "description": "What the control actually requires." },
                            "evidence_type": { "type": "string" },
                            "strength": { "type": "string", "description": "Evidence-strength grade: direct / supporting / indirect." },
                            "evidence_note": { "type": "string", "description": "How a Tumult experiment provides evidence toward the control." },
                            "source_url": { "type": "string" },
                            "last_verified": { "type": "string", "description": "ISO date the citation was last checked against the official source." },
                        },
                    },
                },
            }),
        )),
        "tumult_trend" => Some(schema_object(
            &["metric", "points", "target", "verdict"],
            json!({
                "metric": {
                    "type": "string",
                    "enum": ["resilience_score", "duration_ms", "estimate_accuracy", "method_step_count"],
                },
                "points": {
                    "type": "array",
                    "description": "Time-ordered data points.",
                    "items": {
                        "type": "object",
                        "required": ["ts", "value"],
                        "properties": {
                            "ts": { "type": "integer", "description": "started_at_ns of the run." },
                            "value": { "type": "number" },
                        },
                    },
                },
                "target": { "type": ["string", "null"], "description": "Title filter applied, if any." },
                "verdict": {
                    "type": "string",
                    "enum": ["improving", "declining", "stable", "increasing", "decreasing", "insufficient-data"],
                },
            }),
        )),
        "tumult_gameday_create" => Some(schema_object(
            &["path", "experiments"],
            json!({
                "path": { "type": "string", "description": "Filesystem path of the created .gameday.toon file." },
                "experiments": { "type": "integer", "description": "Number of experiment references written." },
            }),
        )),
        "tumult_agents" => Some(schema_object(
            &["adapters"],
            json!({
                "adapters": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "required": ["name", "installed", "version", "logged_in", "detail"],
                        "properties": {
                            "name": { "type": "string" },
                            "installed": { "type": "boolean" },
                            "version": { "type": ["string", "null"] },
                            "logged_in": {
                                "type": ["boolean", "null"],
                                "description": "null when auth state is not cheaply determinable.",
                            },
                            "detail": { "type": "string" },
                        },
                    },
                },
            }),
        )),
        "tumult_recommend" => Some(schema_object(
            &[],
            json!({
                "message": {
                    "type": "string",
                    "description": "Present instead of the data fields when no analytics store exists."
                },
                "source": { "type": "string" },
                "model": { "type": ["string", "null"] },
                "goal": { "type": ["string", "null"] },
                "recommendations": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "required": ["rank", "title", "rationale"],
                        "properties": {
                            "rank": { "type": "integer" },
                            "title": { "type": "string" },
                            "rationale": { "type": "string" },
                            "plugins": { "type": "array", "items": { "type": "string" } },
                            "actions": { "type": "array", "items": { "type": "string" } },
                            "preconditions": { "type": "array", "items": { "type": "string" } },
                            "expected_learning": { "type": ["string", "null"] },
                        },
                    },
                },
                "draft_toon": { "type": ["string", "null"] },
                "draft_valid": { "type": ["boolean", "null"] },
                "draft_validation_error": { "type": ["string", "null"] },
                "notes": { "type": "array", "items": { "type": "string" } },
                "heuristic_context": { "type": "string" },
                "agent": {
                    "type": "object",
                    "description": "Present only when an agent adapter ran.",
                    "required": ["adapter", "recommendations", "experiments_written", "experiments_rejected"],
                    "properties": {
                        "adapter": { "type": "string" },
                        "model": { "type": ["string", "null"] },
                        "recommendations": { "type": "string" },
                        "experiments_written": { "type": "array", "items": { "type": "string" } },
                        "experiments_rejected": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "required": ["error"],
                                "properties": { "error": { "type": "string" } },
                            },
                        },
                    },
                },
            }),
        )),
        "tumult_store_stats" => Some(schema_object(
            &["store", "schema_version", "experiments", "activities"],
            json!({
                "store": { "type": "string" },
                "schema_version": { "type": "integer" },
                "experiments": { "type": "integer" },
                "activities": { "type": "integer" },
                "size_mb": { "type": "number" },
            }),
        )),
        "tumult_coverage" => Some(schema_object(
            &["plugins", "store"],
            json!({
                "plugins": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "required": ["name", "actions_total", "actions_tested", "probes", "status"],
                        "properties": {
                            "name": { "type": "string" },
                            "actions_total": { "type": "integer" },
                            "actions_tested": { "type": "integer" },
                            "probes": { "type": "integer" },
                            "status": { "type": "string", "enum": ["FULL", "PARTIAL", "NONE"] },
                        },
                    },
                },
                "store": {
                    "type": ["object", "null"],
                    "description": "Store summary counts; null when no analytics store exists.",
                    "properties": {
                        "experiments": { "type": "integer" },
                        "activities": { "type": "integer" },
                        "passed": { "type": "integer" },
                        "distinct_experiment_types": { "type": "integer" },
                    },
                },
            }),
        )),
        "tumult_agentic_list_scenarios" => Some(schema_object(
            &[
                "capture_policy",
                "raw_payloads_captured",
                "packs",
                "trajectory_packs",
            ],
            json!({
                "capture_policy": { "type": "string" },
                "raw_payloads_captured": { "type": "boolean" },
                "packs": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "required": ["name", "adapters", "faults", "contracts"],
                        "properties": {
                            "name": { "type": "string" },
                            "adapters": { "type": "array", "items": { "type": "string" } },
                            "faults": { "type": "array", "items": { "type": "string" } },
                            "contracts": { "type": "array", "items": { "type": "string" } },
                        },
                    },
                },
                "trajectory_packs": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "required": [
                            "name",
                            "description",
                            "steps",
                            "injected",
                            "trajectory_contracts",
                            "headline_contract",
                        ],
                        "properties": {
                            "name": { "type": "string" },
                            "description": { "type": "string" },
                            "steps": { "type": "integer" },
                            "injected": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "required": ["fault", "step_index"],
                                    "properties": {
                                        "fault": { "type": "string" },
                                        "step_index": { "type": "integer" },
                                    },
                                },
                            },
                            "trajectory_contracts": {
                                "type": "array",
                                "items": { "type": "string" },
                            },
                            "headline_contract": { "type": "string" },
                        },
                    },
                },
            }),
        )),
        "tumult_agentic_smoke" | "tumult_agentic_run_experiment" => Some(schema_object(
            &[
                "status",
                "adapter",
                "scenario",
                "fault",
                "contract",
                "expected",
                "actual",
                "resilience_score",
                "raw_payloads_captured",
                "next_diagnostic_command",
            ],
            json!({
                "status": { "type": "string", "enum": ["passed", "failed"] },
                "adapter": { "type": "string" },
                "scenario": { "type": "string" },
                "fault": { "type": "string" },
                "contract": { "type": "string" },
                "expected": { "type": "string" },
                "actual": { "type": "string" },
                "resilience_score": { "type": "number" },
                "raw_payloads_captured": { "type": "boolean" },
                "next_diagnostic_command": { "type": "string" },
            }),
        )),
        "tumult_list_journals" => Some(schema_object(
            &["items", "total", "offset", "limit"],
            json!({
                "items": {
                    "type": "array",
                    "description": "Journal file paths on this page (sorted).",
                    "items": { "type": "string" },
                },
                "total": { "type": "integer", "description": "Total matches before pagination." },
                "offset": { "type": "integer" },
                "limit": { "type": "integer" },
            }),
        )),
        "tumult_list_experiments" => Some(schema_object(
            &["items", "total", "offset", "limit"],
            json!({
                "items": {
                    "type": "array",
                    "description": "Experiment entries on this page (sorted by relative path).",
                    "items": {
                        "type": "object",
                        "required": ["name", "path", "title"],
                        "properties": {
                            "name": { "type": "string", "description": "File name." },
                            "path": { "type": "string", "description": "Path relative to the search root." },
                            "title": { "type": "string", "description": "Experiment title field." },
                        },
                    },
                },
                "total": { "type": "integer", "description": "Total matches before pagination." },
                "offset": { "type": "integer" },
                "limit": { "type": "integer" },
            }),
        )),
        "tumult_gameday_list" => Some(schema_object(
            &["items", "total", "offset", "limit"],
            json!({
                "items": {
                    "type": "array",
                    "description": "GameDay entries on this page (sorted by path).",
                    "items": {
                        "type": "object",
                        "required": ["path", "title"],
                        "properties": {
                            "path": { "type": "string" },
                            "title": { "type": "string" },
                        },
                    },
                },
                "total": { "type": "integer", "description": "Total matches before pagination." },
                "offset": { "type": "integer" },
                "limit": { "type": "integer" },
            }),
        )),
        "tumult_chaosgraph_query" => Some(schema_object(
            &["kind", "count", "nodes"],
            json!({
                "kind": { "type": "string", "description": "Node kind queried." },
                "count": { "type": "integer", "description": "Number of matching nodes." },
                "nodes": {
                    "type": "array",
                    "description": "Matching node summaries.",
                    "items": graph_node_summary_schema(),
                },
            }),
        )),
        "tumult_chaosgraph_neighbors" => Some(schema_object(
            &["node_id", "depth", "nodes", "edges"],
            json!({
                "node_id": { "type": "string", "description": "The centre node id." },
                "depth": { "type": "integer", "description": "Neighbourhood radius expanded." },
                "nodes": {
                    "type": "array",
                    "description": "Every node in the ego sub-graph (including the centre).",
                    "items": graph_node_summary_schema(),
                },
                "edges": {
                    "type": "array",
                    "description": "(src)-[rel]->(dst) tuples among those nodes.",
                    "items": {
                        "type": "object",
                        "required": ["src", "rel", "dst"],
                        "properties": {
                            "src": { "type": "string" },
                            "rel": {
                                "type": "string",
                                "enum": ["targets", "injects", "yielded", "observed_on", "exhibited", "evidences", "maps_to_compliance", "gap_in"],
                            },
                            "dst": { "type": "string" },
                        },
                    },
                },
            }),
        )),
        "tumult_chaosgraph_coverage_gaps" => Some(schema_object(
            &["count", "gaps"],
            json!({
                "count": { "type": "integer", "description": "Number of untested actions (after any domain filter)." },
                "gaps": {
                    "type": "array",
                    "description": "Untested plugin actions.",
                    "items": {
                        "type": "object",
                        "required": ["id", "plugin", "action", "domain"],
                        "properties": {
                            "id": { "type": "string", "description": "Coverage-gap node id (gap:<plugin>::<action>)." },
                            "plugin": { "type": "string" },
                            "action": { "type": "string" },
                            "domain": { "type": "string", "description": "FaultDomain node id (domain:<plugin>)." },
                        },
                    },
                },
                "framework": {
                    "type": "string",
                    "description": "Framework report id; present only when a framework filter was given.",
                },
                "unevidenced_articles": {
                    "type": "array",
                    "description": "Framework articles with no evidences edge yet; present only when a framework filter was given.",
                    "items": {
                        "type": "object",
                        "required": ["id", "control_id", "strength"],
                        "properties": {
                            "id": { "type": "string" },
                            "control_id": { "type": "string" },
                            "strength": { "type": "string" },
                        },
                    },
                },
            }),
        )),
        "tumult_fault_catalog" => Some(schema_object(
            &["action_count", "domains"],
            json!({
                "action_count": { "type": "integer", "description": "Total actions and probes across all domains." },
                "domains": {
                    "type": "array",
                    "description": "Fault domains, each with their actions and probes.",
                    "items": {
                        "type": "object",
                        "required": ["domain", "label", "actions"],
                        "properties": {
                            "domain": { "type": "string" },
                            "label": { "type": "string" },
                            "actions": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "required": ["plugin", "name", "description", "kind", "args"],
                                    "properties": {
                                        "plugin": { "type": "string" },
                                        "name": { "type": "string" },
                                        "description": { "type": "string" },
                                        "kind": { "type": "string", "enum": ["action", "probe"] },
                                        "args": {
                                            "type": "array",
                                            "items": {
                                                "type": "object",
                                                "required": ["name", "required", "description"],
                                                "properties": {
                                                    "name": { "type": "string" },
                                                    "required": { "type": "boolean" },
                                                    "description": { "type": "string" },
                                                },
                                            },
                                        },
                                    },
                                },
                            },
                        },
                    },
                },
            }),
        )),
        "tumult_scaffold_experiment" => Some(schema_object(
            &["action", "toon", "valid"],
            json!({
                "action": { "type": "string", "description": "Fully-qualified plugin::action that was scaffolded." },
                "toon": { "type": "string", "description": "Generated experiment in TOON format." },
                "valid": { "type": "boolean", "description": "Whether the generated experiment passes `tumult validate`." },
                "validation_error": {
                    "type": "string",
                    "description": "Validation failure detail; present only when valid is false.",
                },
            }),
        )),
        _ => None,
    }
}

/// Schema for a `ChaosGraph` node summary (`{id, kind, label}`).
fn graph_node_summary_schema() -> Value {
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
fn journal_schema() -> Value {
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
fn journal_summary_schema() -> Value {
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
fn schema_object(required: &[&str], properties: Value) -> ToolOutputSchema {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_structured_tool_has_a_schema_and_only_those() {
        for name in STRUCTURED_TOOLS {
            assert!(
                output_schema_for(name).is_some(),
                "structured tool '{name}' must advertise an output schema"
            );
        }
        assert!(output_schema_for("tumult_validate").is_none());
        assert!(output_schema_for("tumult_discover").is_none());
        assert!(output_schema_for("no_such_tool").is_none());
    }

    #[test]
    fn schemas_are_object_typed_with_declared_required_properties() {
        for name in STRUCTURED_TOOLS {
            let schema = output_schema_for(name).unwrap();
            assert_eq!(schema.type_(), "object", "schema for '{name}'");
            let properties = schema.properties.as_ref().unwrap_or_else(|| {
                panic!("schema for '{name}' must declare properties");
            });
            for required in &schema.required {
                assert!(
                    properties.contains_key(required),
                    "required property '{required}' of '{name}' must be declared"
                );
            }
        }
    }
}
