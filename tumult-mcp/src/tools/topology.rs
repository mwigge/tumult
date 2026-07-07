//! Topology tools: declared-topology import, the compliance-aware service
//! map, the compliance lineage matrix, and injection recommendations over
//! the persistent analytics store.

use std::collections::HashMap;
use std::fmt::Write as _;

use tumult_graph::lineage::{compute_lineage, ControlServiceStatus};
use tumult_graph::render::build_view;
use tumult_graph::{parse_topology, topology_delta};

use crate::error::ToolError;
use crate::tools::StructuredReport;

mod inputs;

use inputs::{
    canonical_framework, gather_inputs, open_store, open_store_ro, recommendations_for,
};

/// `topology_import`: parse a declared topology TOML (inline or from a file)
/// and replace the store's declared-topology sub-graph with it.
///
/// This is the one topology tool that opens the store **read-write**: the
/// import persists `svc:` nodes and `depends_on` edges under the sentinel
/// topology run id. The write is brief (clear + insert of a small delta),
/// idempotent (re-import converges to the same rows), and Operator-gated at
/// the dispatch layer.
///
/// The structured object is `{services, dependencies, service_ids}`.
///
/// # Errors
///
/// Returns a [`ToolError`] if neither or both of `toml_content`/`path` are
/// given, the file cannot be read, the TOML is invalid, or the store does
/// not exist or cannot be written.
pub fn topology_import(
    store_path: &str,
    toml_content: Option<&str>,
    path: Option<&str>,
) -> Result<StructuredReport, ToolError> {
    let content = match (toml_content, path) {
        (Some(content), None) => content.to_string(),
        (None, Some(path)) => std::fs::read_to_string(path).map_err(|e| {
            ToolError::NotFound(format!("cannot read topology file {path}: {e}"))
        })?,
        _ => {
            return Err(ToolError::InvalidInput(
                "provide exactly one of toml_content (inline TOML) or path (a topology TOML file)"
                    .into(),
            ))
        }
    };
    let doc = parse_topology(&content).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
    let delta = topology_delta(&doc);

    let store = open_store(store_path)?;
    store
        .refresh_topology(&delta)
        .map_err(|e| ToolError::Store(e.to_string()))?;

    let mut service_ids: Vec<String> = delta.nodes.iter().map(|n| n.id.clone()).collect();
    service_ids.sort();
    let (services, dependencies) = (delta.nodes.len(), delta.edges.len());

    let mut structured = serde_json::Map::new();
    structured.insert("services".into(), serde_json::json!(services));
    structured.insert("dependencies".into(), serde_json::json!(dependencies));
    structured.insert("service_ids".into(), serde_json::json!(service_ids));

    Ok(StructuredReport {
        text: format!("imported {services} services, {dependencies} dependencies\n"),
        structured,
    })
}

/// `topology_map`: the compliance-aware service map — declared services with
/// worst-of lineage state, `depends_on` edges, break causes, and (optionally)
/// ranked injection recommendations — rendered as text, Mermaid, or JSON.
///
/// The structured object is `{format, map}`; `map` is always the full view
/// JSON regardless of the text rendering chosen.
///
/// # Errors
///
/// Returns a [`ToolError`] if the store does not exist or cannot be read, or
/// an unknown `framework`/`format` is given.
pub fn topology_map(
    store_path: &str,
    framework: Option<&str>,
    control: Option<&str>,
    format: Option<&str>,
    with_recommendations: Option<bool>,
    limit: Option<u32>,
) -> Result<StructuredReport, ToolError> {
    let format = format.unwrap_or("text");
    if !matches!(format, "text" | "mermaid" | "json") {
        return Err(ToolError::InvalidInput(format!(
            "unknown format '{format}'; valid values: text, mermaid, json"
        )));
    }
    let framework = canonical_framework(framework)?;

    let store = open_store_ro(store_path)?;
    let inputs = gather_inputs(&store)?;
    let lineage = compute_lineage(&inputs.lineage_input(), framework, control);
    let recs = if with_recommendations.unwrap_or(true) {
        recommendations_for(&store, &inputs, &lineage, limit.unwrap_or(3) as usize)?
    } else {
        Vec::new()
    };
    let view = build_view(&inputs.services_with_attrs, &inputs.depends_on, &lineage, &recs);

    let text = match format {
        "mermaid" => view.to_mermaid(),
        "json" => format!(
            "topology map: {} service(s), {} dependency edge(s), {} recommendation(s); full view in structured content",
            view.services.len(),
            view.depends_on.len(),
            view.recommendations.len()
        ),
        _ => view.to_text(),
    };

    let mut structured = serde_json::Map::new();
    structured.insert("format".into(), serde_json::json!(format));
    structured.insert("map".into(), view.to_json());

    Ok(StructuredReport {
        text: crate::tools::cap_text(text, "scope with framework or control"),
        structured,
    })
}

/// `compliance_lineage`: the (article × service) lineage matrix, optionally
/// scoped by framework, control, and service.
///
/// The structured object is `{cells, counts}` with one entry per cell and
/// `counts` keyed by status (`evidenced` / `broken` / `untested`).
///
/// # Errors
///
/// Returns a [`ToolError`] if the store does not exist or cannot be read, or
/// an unknown `framework` is given.
pub fn compliance_lineage(
    store_path: &str,
    framework: Option<&str>,
    control: Option<&str>,
    service: Option<&str>,
) -> Result<StructuredReport, ToolError> {
    let framework = canonical_framework(framework)?;
    let store = open_store_ro(store_path)?;
    let inputs = gather_inputs(&store)?;
    let mut cells = compute_lineage(&inputs.lineage_input(), framework, control);
    if let Some(service) = service {
        let full_id = format!("svc:{service}");
        cells.retain(|cell| cell.service_id == full_id || cell.service_id == service);
    }

    let mut counts = HashMap::from([("evidenced", 0u64), ("broken", 0), ("untested", 0)]);
    let mut text = format!("lineage: {} cell(s)\n", cells.len());
    for cell in &cells {
        let (word, count_key) = match cell.status {
            ControlServiceStatus::Evidenced => ("EVIDENCED", "evidenced"),
            ControlServiceStatus::Broken => ("BROKEN", "broken"),
            ControlServiceStatus::Untested => ("UNTESTED", "untested"),
        };
        *counts.entry(count_key).or_default() += 1;
        let _ = write!(text, "  {word} {} on {}", cell.article_id, cell.service_id);
        if let Some(strength) = &cell.evidence_strength {
            let _ = write!(text, " (strength {strength})");
        }
        if let Some(cause) = &cell.cause {
            let fault = cause.fault_id.as_deref().unwrap_or("unattributed");
            let _ = write!(text, " — {fault}");
            if let Some(guard) = &cause.guard_name {
                let _ = write!(text, " (guard: {guard})");
            }
        }
        text.push('\n');
    }
    let _ = writeln!(
        text,
        "counts: {} evidenced, {} broken, {} untested",
        counts["evidenced"], counts["broken"], counts["untested"]
    );

    let mut structured = serde_json::Map::new();
    structured.insert(
        "cells".into(),
        serde_json::to_value(&cells).unwrap_or(serde_json::Value::Null),
    );
    structured.insert(
        "counts".into(),
        serde_json::json!({
            "evidenced": counts["evidenced"],
            "broken": counts["broken"],
            "untested": counts["untested"],
        }),
    );

    Ok(StructuredReport {
        text: crate::tools::cap_text(text, "scope with framework, control, or service"),
        structured,
    })
}

/// `recommend_injection`: ranked, explained injection recommendations from
/// the lineage matrix, declared topology, and plugin catalog.
///
/// The structured object is `{recommendations}` in ranked order.
///
/// # Errors
///
/// Returns a [`ToolError`] if the store does not exist or cannot be read, or
/// an unknown `framework` is given.
pub fn recommend_injection(
    store_path: &str,
    framework: Option<&str>,
    limit: Option<u32>,
) -> Result<StructuredReport, ToolError> {
    let framework = canonical_framework(framework)?;
    let store = open_store_ro(store_path)?;
    let inputs = gather_inputs(&store)?;
    let lineage = compute_lineage(&inputs.lineage_input(), framework, None);
    let recs = recommendations_for(&store, &inputs, &lineage, limit.unwrap_or(3) as usize)?;

    let mut text = format!("{} recommendation(s)\n", recs.len());
    for (index, rec) in recs.iter().enumerate() {
        let _ = writeln!(
            text,
            "{}. {}::{} on {} for {} (score {:.2})",
            index + 1,
            rec.plugin,
            rec.action,
            rec.service_id,
            rec.article_id,
            rec.score
        );
        for reason in &rec.reasons {
            let _ = writeln!(text, "   - {reason}");
        }
    }

    let mut structured = serde_json::Map::new();
    structured.insert(
        "recommendations".into(),
        serde_json::to_value(&recs).unwrap_or(serde_json::Value::Null),
    );

    Ok(StructuredReport {
        text: crate::tools::cap_text(text, "reduce limit or scope with framework"),
        structured,
    })
}

#[cfg(test)]
mod tests;
