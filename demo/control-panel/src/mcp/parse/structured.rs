//! Parsers for tools that return `structuredContent`: compliance, the
//! ChaosGraph query/neighbors/coverage-gaps trio, fault catalog, scaffold,
//! and whoami.

use serde_json::Value;

use crate::mcp::protocol::McpError;

use super::outcomes::{
    CatalogAction, CatalogArg, CatalogDomain, CatalogOutcome, ComplianceCitation,
    ComplianceOutcome, CoverageGap, CoverageGapsOutcome, GraphEdge, GraphEgoOutcome, GraphNode,
    GraphNodesOutcome, ScaffoldOutcome, UnevidencedArticle, WhoamiOutcome,
};
use super::{content_text, str_field};

/// Parse a `tumult_compliance` `tools/call` result into a [`ComplianceOutcome`]
/// from its `structuredContent`.
///
/// # Errors
/// Returns [`McpError::Protocol`] on a tool-level error or missing structured
/// content.
pub fn parse_compliance_result(result: &Value) -> Result<ComplianceOutcome, McpError> {
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(McpError::Protocol(content_text(result).unwrap_or_else(
            || "compliance tool reported an error".to_string(),
        )));
    }
    let sc = result.get("structuredContent").ok_or_else(|| {
        McpError::Protocol("compliance result missing structuredContent".to_string())
    })?;
    let citations = sc
        .get("citations")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .take(4)
                .map(|c| ComplianceCitation {
                    control_id: str_field(c, "control_id"),
                    title: str_field(c, "title"),
                    strength: str_field(c, "strength"),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(ComplianceOutcome {
        framework: str_field(sc, "framework"),
        pass_rate: sc.get("pass_rate").and_then(Value::as_f64).unwrap_or(0.0),
        recovery_compliance: sc.get("recovery_compliance").and_then(Value::as_f64),
        verdict: str_field(sc, "verdict"),
        journals_evaluated: sc
            .get("journals_evaluated")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        disclaimer: str_field(sc, "disclaimer"),
        source_url: sc
            .get("source_url")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        citations,
    })
}

/// Parse a `tumult_chaosgraph_query` result into a [`GraphNodesOutcome`].
///
/// # Errors
/// Returns [`McpError::Protocol`] on a tool-level error or missing structured
/// content.
pub fn parse_graph_query_result(result: &Value) -> Result<GraphNodesOutcome, McpError> {
    let sc = graph_structured(result, "query")?;
    Ok(GraphNodesOutcome {
        kind: str_field(sc, "kind"),
        count: sc
            .get("count")
            .and_then(Value::as_u64)
            .map_or_else(|| 0, |c| usize::try_from(c).unwrap_or(usize::MAX)),
        nodes: parse_graph_nodes(sc.get("nodes")),
    })
}

/// Parse a `tumult_chaosgraph_neighbors` result into a [`GraphEgoOutcome`].
///
/// # Errors
/// Returns [`McpError::Protocol`] on a tool-level error or missing structured
/// content.
pub fn parse_graph_neighbors_result(result: &Value) -> Result<GraphEgoOutcome, McpError> {
    let sc = graph_structured(result, "neighbors")?;
    let edges = sc
        .get("edges")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|e| GraphEdge {
                    src: str_field(e, "src"),
                    rel: str_field(e, "rel"),
                    dst: str_field(e, "dst"),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(GraphEgoOutcome {
        node_id: str_field(sc, "node_id"),
        depth: sc
            .get("depth")
            .and_then(Value::as_u64)
            .and_then(|d| u32::try_from(d).ok())
            .unwrap_or(1),
        nodes: parse_graph_nodes(sc.get("nodes")),
        edges,
    })
}

/// Parse a `tumult_chaosgraph_coverage_gaps` result into a
/// [`CoverageGapsOutcome`].
///
/// # Errors
/// Returns [`McpError::Protocol`] on a tool-level error or missing structured
/// content.
pub fn parse_coverage_gaps_result(result: &Value) -> Result<CoverageGapsOutcome, McpError> {
    let sc = graph_structured(result, "coverage_gaps")?;
    let gaps = sc
        .get("gaps")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|g| CoverageGap {
                    id: str_field(g, "id"),
                    plugin: str_field(g, "plugin"),
                    action: str_field(g, "action"),
                    domain: str_field(g, "domain"),
                })
                .collect()
        })
        .unwrap_or_default();
    let unevidenced_articles = sc
        .get("unevidenced_articles")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|a| UnevidencedArticle {
                    id: str_field(a, "id"),
                    control_id: str_field(a, "control_id"),
                    strength: str_field(a, "strength"),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(CoverageGapsOutcome {
        count: sc
            .get("count")
            .and_then(Value::as_u64)
            .map_or_else(|| 0, |c| usize::try_from(c).unwrap_or(usize::MAX)),
        gaps,
        framework: sc
            .get("framework")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        unevidenced_articles,
    })
}

/// Parse a `tumult_fault_catalog` result into a [`CatalogOutcome`] from its
/// `structuredContent` (`{action_count, domains:[{domain,label,actions:[…]}]}`).
///
/// # Errors
/// Returns [`McpError::Protocol`] on a tool-level error or missing structured
/// content.
pub fn parse_catalog_result(result: &Value) -> Result<CatalogOutcome, McpError> {
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(McpError::Protocol(content_text(result).unwrap_or_else(
            || "fault_catalog tool reported an error".to_string(),
        )));
    }
    let sc = result.get("structuredContent").ok_or_else(|| {
        McpError::Protocol("fault_catalog result missing structuredContent".to_string())
    })?;
    let domains = sc
        .get("domains")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(parse_catalog_domain).collect())
        .unwrap_or_default();
    let action_count = sc
        .get("action_count")
        .and_then(Value::as_u64)
        .map_or_else(|| 0, |c| usize::try_from(c).unwrap_or(usize::MAX));
    Ok(CatalogOutcome {
        action_count,
        domains,
    })
}

/// Parse one `{domain, label, actions}` object.
fn parse_catalog_domain(v: &Value) -> CatalogDomain {
    let actions = v
        .get("actions")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(parse_catalog_action).collect())
        .unwrap_or_default();
    CatalogDomain {
        domain: str_field(v, "domain"),
        label: str_field(v, "label"),
        actions,
    }
}

/// Parse one `{plugin, name, description, kind, args}` action object.
fn parse_catalog_action(v: &Value) -> CatalogAction {
    let args = v
        .get("args")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|a| CatalogArg {
                    name: str_field(a, "name"),
                    required: a.get("required").and_then(Value::as_bool).unwrap_or(false),
                    description: str_field(a, "description"),
                })
                .collect()
        })
        .unwrap_or_default();
    CatalogAction {
        plugin: str_field(v, "plugin"),
        name: str_field(v, "name"),
        description: str_field(v, "description"),
        kind: str_field(v, "kind"),
        args,
    }
}

/// Parse a `tumult_scaffold_experiment` result into a [`ScaffoldOutcome`] from
/// its `structuredContent` (`{action, toon, valid, validation_error?}`).
///
/// A scaffold that produces an *invalid* experiment is NOT a tool error — the
/// tool returns `valid: false` with a `validation_error`, which the UI badges.
/// Only a true tool-level failure (`isError: true`) maps to an error here.
///
/// # Errors
/// Returns [`McpError::Protocol`] on a tool-level error or missing structured
/// content.
pub fn parse_scaffold_result(result: &Value) -> Result<ScaffoldOutcome, McpError> {
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(McpError::Protocol(content_text(result).unwrap_or_else(
            || "scaffold_experiment tool reported an error".to_string(),
        )));
    }
    let sc = result.get("structuredContent").ok_or_else(|| {
        McpError::Protocol("scaffold_experiment result missing structuredContent".to_string())
    })?;
    Ok(ScaffoldOutcome {
        action: str_field(sc, "action"),
        toon: str_field(sc, "toon"),
        valid: sc.get("valid").and_then(Value::as_bool).unwrap_or(false),
        validation_error: sc
            .get("validation_error")
            .and_then(Value::as_str)
            .map(ToString::to_string),
    })
}

/// Parse a `tumult_whoami` result into a [`WhoamiOutcome`] from its
/// `structuredContent` (`{role, authenticated}`).
///
/// # Errors
/// Returns [`McpError::Protocol`] on a tool-level error or missing structured
/// content.
pub fn parse_whoami_result(result: &Value) -> Result<WhoamiOutcome, McpError> {
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(McpError::Protocol(
            content_text(result).unwrap_or_else(|| "whoami tool reported an error".to_string()),
        ));
    }
    let sc = result
        .get("structuredContent")
        .ok_or_else(|| McpError::Protocol("whoami result missing structuredContent".to_string()))?;
    let role = sc
        .get("role")
        .and_then(Value::as_str)
        .ok_or_else(|| McpError::Protocol("whoami result missing role".to_string()))?
        .to_string();
    let authenticated = sc
        .get("authenticated")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok(WhoamiOutcome {
        role,
        authenticated,
    })
}

/// Pull the `structuredContent` object from a ChaosGraph tool result, mapping a
/// tool-level error into [`McpError::Protocol`].
fn graph_structured<'a>(result: &'a Value, tool: &str) -> Result<&'a Value, McpError> {
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(McpError::Protocol(content_text(result).unwrap_or_else(
            || format!("chaosgraph {tool} tool reported an error"),
        )));
    }
    result.get("structuredContent").ok_or_else(|| {
        McpError::Protocol(format!(
            "chaosgraph {tool} result missing structuredContent"
        ))
    })
}

/// Parse a `nodes` array of `{id, kind, label}` objects.
fn parse_graph_nodes(nodes: Option<&Value>) -> Vec<GraphNode> {
    nodes
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|n| GraphNode {
                    id: str_field(n, "id"),
                    kind: str_field(n, "kind"),
                    label: str_field(n, "label"),
                })
                .collect()
        })
        .unwrap_or_default()
}
