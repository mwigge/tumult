//! Output schemas advertised for tools that return structured content.
//!
//! The `#[mcp_tool]` macro always emits `output_schema: None`, so
//! `handle_list_tools_request` patches each generated [`Tool`] with the
//! schema returned by [`output_schema_for`]. Schemas are hand-written,
//! compact JSON Schemas derived from the serde types that produce the
//! structured content (`tumult_core::types::Journal` et al.). Free-form
//! fields (`ActivityResult::output` / `error`) are documented as opaque
//! strings.

mod schemas;

use rust_mcp_sdk::schema::ToolOutputSchema;
use serde_json::json;

use schemas::{
    autopilot_decision_schema, graph_node_summary_schema, journal_schema, journal_summary_schema,
    lineage_cell_schema, recommendation_schema, schema_object,
};

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
    "tumult_whoami",
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
    "tumult_topology_import",
    "tumult_topology_map",
    "tumult_compliance_lineage",
    "tumult_recommend_injection",
    "tumult_autopilot_run",
    "tumult_autopilot_status",
    "tumult_autopilot_respond",
    "tumult_autopilot_export",
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
        "tumult_whoami" => Some(schema_object(
            &["role", "authenticated"],
            json!({
                "role": {
                    "type": "string",
                    "enum": ["viewer", "operator"],
                    "description": "The caller's resolved access role: viewer (read-only tools) or operator (all tools).",
                },
                "authenticated": {
                    "type": "boolean",
                    "description": "True when a configured bearer token validated the request; false in loopback open mode (no auth configured).",
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
                                "enum": ["targets", "injects", "yielded", "observed_on", "exhibited", "evidences", "maps_to_compliance", "gap_in", "depends_on", "caused_by"],
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
        "tumult_topology_import" => Some(schema_object(
            &["services", "dependencies", "service_ids"],
            json!({
                "services": { "type": "integer", "description": "Number of declared services imported." },
                "dependencies": { "type": "integer", "description": "Number of depends_on edges imported." },
                "service_ids": {
                    "type": "array",
                    "description": "Imported service node ids (svc:<name>), sorted.",
                    "items": { "type": "string" },
                },
            }),
        )),
        "tumult_topology_map" => Some(schema_object(
            &["format", "map"],
            json!({
                "format": { "type": "string", "enum": ["text", "mermaid", "json"] },
                "map": {
                    "type": "object",
                    "description": "The full topology map view, regardless of the text rendering chosen.",
                    "required": ["services", "depends_on", "recommendations"],
                    "properties": {
                        "services": {
                            "type": "array",
                            "description": "Services in render order with rolled-up compliance verdicts.",
                            "items": {
                                "type": "object",
                                "required": ["id", "label", "state", "evidenced", "untested", "broken"],
                                "properties": {
                                    "id": { "type": "string" },
                                    "label": { "type": "string" },
                                    "tier": { "type": ["string", "null"] },
                                    "owner": { "type": ["string", "null"] },
                                    "state": { "type": "string", "enum": ["evidenced", "broken", "untested", "unknown"] },
                                    "evidenced": { "type": "array", "items": { "type": "string" } },
                                    "untested": { "type": "array", "items": { "type": "string" } },
                                    "broken": {
                                        "type": "array",
                                        "items": {
                                            "type": "object",
                                            "required": ["article_id"],
                                            "properties": {
                                                "article_id": { "type": "string" },
                                                "deviation_id": { "type": ["string", "null"] },
                                                "fault_id": { "type": ["string", "null"] },
                                                "guard_name": { "type": ["string", "null"] },
                                                "run_id": { "type": ["string", "null"] },
                                            },
                                        },
                                    },
                                },
                            },
                        },
                        "depends_on": {
                            "type": "array",
                            "description": "Declared (src, dst) dependency edges, sorted.",
                            "items": { "type": "array", "items": { "type": "string" } },
                        },
                        "recommendations": {
                            "type": "array",
                            "description": "Ranked injection recommendations (empty when recommend=false).",
                            "items": recommendation_schema(),
                        },
                    },
                },
            }),
        )),
        "tumult_compliance_lineage" => Some(schema_object(
            &["cells", "counts"],
            json!({
                "cells": {
                    "type": "array",
                    "description": "Lineage cells, sorted by (article_id, service_id).",
                    "items": lineage_cell_schema(),
                },
                "counts": {
                    "type": "object",
                    "required": ["evidenced", "broken", "untested"],
                    "properties": {
                        "evidenced": { "type": "integer" },
                        "broken": { "type": "integer" },
                        "untested": { "type": "integer" },
                    },
                },
            }),
        )),
        "tumult_recommend_injection" => Some(schema_object(
            &["recommendations"],
            json!({
                "recommendations": {
                    "type": "array",
                    "description": "Ranked, explained injection recommendations.",
                    "items": recommendation_schema(),
                },
            }),
        )),
        "tumult_autopilot_run" => Some(schema_object(
            &["decisions", "enacted", "policy_hash", "executed"],
            json!({
                "decisions": {
                    "type": "array",
                    "description": "Every decision gated and persisted this pass, in gate order.",
                    "items": autopilot_decision_schema(),
                },
                "enacted": { "type": "integer", "description": "Number of playbooks actually run (always 0 when execute=false)." },
                "policy_hash": { "type": "string", "description": "sha256 of the policy text this pass was gated by." },
                "executed": { "type": "boolean", "description": "Whether execute=true was in effect (enact verdicts ran playbooks)." },
            }),
        )),
        "tumult_autopilot_status" => Some(schema_object(
            &["decisions", "count"],
            json!({
                "decisions": {
                    "type": "array",
                    "description": "Recorded decisions, newest first, with their latest lifecycle event.",
                    "items": autopilot_decision_schema(),
                },
                "count": { "type": "integer", "description": "Number of decisions returned (after verdict filter and limit)." },
            }),
        )),
        "tumult_autopilot_respond" => Some(schema_object(
            &["decision_id", "action"],
            json!({
                "decision_id": { "type": "string", "description": "The decision responded to." },
                "action": {
                    "type": "string",
                    "enum": ["human_approved", "human_denied"],
                    "description": "The lifecycle event appended by this response.",
                },
            }),
        )),
        "tumult_autopilot_export" => Some(schema_object(
            &["dir"],
            json!({
                "dir": { "type": "string", "description": "Directory the Parquet files were written to." },
            }),
        )),
        _ => None,
    }
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
