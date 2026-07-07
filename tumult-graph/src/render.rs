//! Topology map view: assemble services + lineage + recommendations into one
//! renderable structure, with text, Mermaid, and JSON renderers.
//!
//! This is the presentation seam: `build_view` fuses the pure computations
//! (lineage cells, recommendations, declared `depends_on` edges) into a
//! [`TopologyMapView`] that the CLI/MCP layers can render without re-deriving
//! anything. All three renderers are deterministic — services are ordered by
//! tier (edge < service < data < other) then id — so output diffs cleanly in
//! golden tests and agent transcripts.

use std::collections::BTreeMap;
// Writing into a `String` is infallible, so the `write!` results below are
// deliberately discarded — this keeps rendering panic-free without `unwrap`.
use std::fmt::Write as _;

use serde::Serialize;

use crate::lineage::{ControlServiceStatus, LineageCell};
use crate::model::NodeSummary;
use crate::recommend::Recommendation;

/// The rolled-up state of one service: the *worst* of its lineage cells
/// (Broken > Untested > Evidenced). `Unknown` means no article was in scope
/// for it at all — distinct from Untested, which means articles exist but no
/// run covered them on this service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceState {
    /// Every in-scope cell on this service is evidenced.
    Evidenced,
    /// At least one control is broken on this service.
    Broken,
    /// No break, but at least one control is untested here.
    Untested,
    /// No lineage cell references this service.
    Unknown,
}

impl ServiceState {
    /// Short glyph used in text/Mermaid labels.
    #[must_use]
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Evidenced => "OK",
            Self::Broken => "BROKEN",
            Self::Untested => "UNTESTED",
            Self::Unknown => "UNKNOWN",
        }
    }

    /// The Mermaid `classDef` name for this state.
    fn class(self) -> &'static str {
        match self {
            Self::Evidenced => "evidenced",
            Self::Broken => "broken",
            Self::Untested => "untested",
            Self::Unknown => "unknown",
        }
    }
}

/// Summary of one broken control on a service, flattened for rendering.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BrokenControl {
    /// The broken compliance article id.
    pub article_id: String,
    /// The deviation node id, when the break carried one.
    pub deviation_id: Option<String>,
    /// The attributed fault, when unambiguous.
    pub fault_id: Option<String>,
    /// The halting guard name, when known.
    pub guard_name: Option<String>,
    /// The run that broke the control.
    pub run_id: Option<String>,
}

/// One service on the map, with its rolled-up compliance verdicts.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ServiceView {
    /// Service node id (`svc:<name>`).
    pub id: String,
    /// Human label (usually the bare service name).
    pub label: String,
    /// Declared tier (`edge`/`service`/`data`), when present.
    pub tier: Option<String>,
    /// Declared owner, when present.
    pub owner: Option<String>,
    /// Worst-of state across the service's lineage cells.
    pub state: ServiceState,
    /// Evidenced article ids (sorted).
    pub evidenced: Vec<String>,
    /// Untested article ids (sorted).
    pub untested: Vec<String>,
    /// Broken controls with their causes (sorted by article id).
    pub broken: Vec<BrokenControl>,
}

/// The full map view: services, dependency edges, and recommendations.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TopologyMapView {
    /// Services in render order (tier rank, then id).
    pub services: Vec<ServiceView>,
    /// Declared `(src, dst)` dependency edges, sorted.
    pub depends_on: Vec<(String, String)>,
    /// Recommendations in their ranked order (from [`crate::recommend`]).
    pub recommendations: Vec<Recommendation>,
}

/// Assemble the view. `services_with_attrs` pairs each service node with its
/// parsed attrs (for `tier`/`owner`); lineage and recommendations come from
/// their respective pure computations and are not re-derived here.
#[must_use]
pub fn build_view(
    services_with_attrs: &[(NodeSummary, serde_json::Value)],
    depends_on: &[(String, String)],
    lineage: &[LineageCell],
    recommendations: &[Recommendation],
) -> TopologyMapView {
    let mut by_service: BTreeMap<&str, Vec<&LineageCell>> = BTreeMap::new();
    for cell in lineage {
        by_service
            .entry(cell.service_id.as_str())
            .or_default()
            .push(cell);
    }

    let mut services: Vec<ServiceView> = services_with_attrs
        .iter()
        .map(|(node, attrs)| service_view(node, attrs, by_service.get(node.id.as_str())))
        .collect();
    services.sort_by(|a, b| {
        (tier_rank(a.tier.as_deref()), a.id.as_str())
            .cmp(&(tier_rank(b.tier.as_deref()), b.id.as_str()))
    });

    let mut depends_on = depends_on.to_vec();
    depends_on.sort();
    depends_on.dedup();

    TopologyMapView {
        services,
        depends_on,
        recommendations: recommendations.to_vec(),
    }
}

/// Roll one service's cells into a [`ServiceView`].
fn service_view(
    node: &NodeSummary,
    attrs: &serde_json::Value,
    cells: Option<&Vec<&LineageCell>>,
) -> ServiceView {
    let mut evidenced = Vec::new();
    let mut untested = Vec::new();
    let mut broken = Vec::new();
    for cell in cells.into_iter().flatten() {
        match cell.status {
            ControlServiceStatus::Evidenced => evidenced.push(cell.article_id.clone()),
            ControlServiceStatus::Untested => untested.push(cell.article_id.clone()),
            ControlServiceStatus::Broken => broken.push(BrokenControl {
                article_id: cell.article_id.clone(),
                deviation_id: cell
                    .cause
                    .as_ref()
                    .map(|c| c.deviation_id.clone())
                    .filter(|id| !id.is_empty()),
                fault_id: cell.cause.as_ref().and_then(|c| c.fault_id.clone()),
                guard_name: cell.cause.as_ref().and_then(|c| c.guard_name.clone()),
                run_id: cell.cause.as_ref().map(|c| c.run_id.clone()),
            }),
        }
    }
    evidenced.sort();
    untested.sort();
    broken.sort_by(|a, b| a.article_id.cmp(&b.article_id));

    let state = if !broken.is_empty() {
        ServiceState::Broken
    } else if !untested.is_empty() {
        ServiceState::Untested
    } else if evidenced.is_empty() {
        ServiceState::Unknown
    } else {
        ServiceState::Evidenced
    };

    ServiceView {
        id: node.id.clone(),
        label: node.label.clone(),
        tier: attrs
            .get("tier")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        owner: attrs
            .get("owner")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        state,
        evidenced,
        untested,
        broken,
    }
}

/// Render order for tiers: traffic flows edge → service → data, and the map
/// reads top-down the same way. Unknown tiers sink to the bottom.
fn tier_rank(tier: Option<&str>) -> u8 {
    match tier {
        Some("edge") => 0,
        Some("service") => 1,
        Some("data") => 2,
        _ => 3,
    }
}

impl TopologyMapView {
    /// Plain-text rendering: a legend, then one block per service with its
    /// verdicts, dependents, and recommendations.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut out = String::from(
            "legend: [OK] evidenced | [BROKEN] control broken | [UNTESTED] no evidence yet | [UNKNOWN] no articles in scope\n",
        );
        for service in &self.services {
            out.push('\n');
            let _ = write!(out, "[{}] {}", service.state.glyph(), service.id);
            let meta: Vec<&str> = [service.tier.as_deref(), service.owner.as_deref()]
                .into_iter()
                .flatten()
                .collect();
            if !meta.is_empty() {
                let _ = write!(out, " ({})", meta.join(", "));
            }
            out.push('\n');
            for control in &service.broken {
                let _ = write!(out, "  {} broken", control.article_id);
                match &control.fault_id {
                    Some(fault) => {
                        let _ = write!(out, " — {fault}");
                    }
                    None => out.push_str(" — cause unattributed"),
                }
                if let Some(guard) = &control.guard_name {
                    let _ = write!(out, " (guard: {guard})");
                }
                out.push('\n');
            }
            for article in &service.untested {
                let _ = writeln!(out, "  {article} untested");
            }
            for article in &service.evidenced {
                let _ = writeln!(out, "  {article} evidenced");
            }
            let dependents = self.dependents_of(&service.id);
            if !dependents.is_empty() {
                let verb = if dependents.len() == 1 {
                    "depends"
                } else {
                    "depend"
                };
                let _ = writeln!(out, "  <- {} {verb} on this", dependents.join(", "));
            }
            for rec in self
                .recommendations
                .iter()
                .filter(|r| r.service_id == service.id)
            {
                let _ = writeln!(
                    out,
                    "  >> RECOMMENDED: {}::{} for {} (score {:.2}) — {}",
                    rec.plugin,
                    rec.action,
                    rec.article_id,
                    rec.score,
                    rec.reasons.join("; ")
                );
            }
        }
        out
    }

    /// Mermaid `graph TD` rendering with state classes, cause annotations,
    /// and recommendation nodes. Ids are sanitized to `[a-zA-Z0-9_]`.
    #[must_use]
    pub fn to_mermaid(&self) -> String {
        let mut out = String::from("graph TD\n");
        for service in &self.services {
            let _ = writeln!(
                out,
                "  {}[\"{} {}\"]",
                sanitize_id(&service.id),
                escape_label(&service.label),
                service.state.glyph()
            );
        }
        for (src, dst) in &self.depends_on {
            let _ = writeln!(out, "  {} --> {}", sanitize_id(src), sanitize_id(dst));
        }
        for service in &self.services {
            let sid = sanitize_id(&service.id);
            for (index, control) in service.broken.iter().enumerate() {
                let cause_id = format!("cause_{sid}_{index}");
                let fault = control.fault_id.as_deref().unwrap_or("unattributed");
                let mut label = format!("fault: {}", escape_label(fault));
                if let Some(guard) = &control.guard_name {
                    let _ = write!(label, "<br/>guard: {}", escape_label(guard));
                }
                let _ = writeln!(out, "  {cause_id}[\"{label}\"]");
                let _ = writeln!(out, "  {cause_id} -.-> {sid}");
            }
        }
        for (index, rec) in self.recommendations.iter().enumerate() {
            let rec_id = format!("rec_{index}");
            let _ = writeln!(
                out,
                "  {rec_id}[\"⚡ {}::{}<br/>for {}\"]",
                escape_label(&rec.plugin),
                escape_label(&rec.action),
                escape_label(&rec.article_id)
            );
            let _ = writeln!(out, "  {rec_id} -.-> {}", sanitize_id(&rec.service_id));
        }
        out.push_str("  classDef evidenced fill:#2e7d32,color:#fff\n");
        out.push_str("  classDef broken fill:#c62828,color:#fff\n");
        out.push_str("  classDef untested fill:#f9a825\n");
        out.push_str("  classDef unknown fill:#546e7a,color:#fff\n");
        out.push_str("  classDef recommended fill:#6a1b9a,color:#fff\n");
        for service in &self.services {
            let _ = writeln!(
                out,
                "  class {} {}",
                sanitize_id(&service.id),
                service.state.class()
            );
        }
        for index in 0..self.recommendations.len() {
            let _ = writeln!(out, "  class rec_{index} recommended");
        }
        out
    }

    /// JSON rendering via serde. Infallible in practice (the view contains
    /// only string-keyed, finite data); degrades to `null` rather than
    /// panicking if that invariant is ever violated.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }

    /// Labels of services that depend on `id`, sorted, `svc:` prefix
    /// stripped for readability.
    fn dependents_of(&self, id: &str) -> Vec<String> {
        let mut dependents: Vec<String> = self
            .depends_on
            .iter()
            .filter(|(_, dst)| dst == id)
            .map(|(src, _)| src.strip_prefix("svc:").unwrap_or(src).to_string())
            .collect();
        dependents.sort();
        dependents.dedup();
        dependents
    }
}

/// Sanitize a node id for Mermaid: `[a-zA-Z0-9_]` only, everything else
/// becomes `_` (`svc:demo-app` → `svc_demo_app`).
fn sanitize_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Escape a Mermaid node label: double quotes would terminate the `["…"]`
/// form, so replace them with the Mermaid `#quot;` entity.
fn escape_label(label: &str) -> String {
    label.replace('"', "#quot;")
}

// View-level behaviour tests live in `tests/render.rs`; this module only
// tests the private string helpers.
#[cfg(test)]
mod tests {
    use super::{escape_label, sanitize_id};

    #[test]
    fn helpers_sanitize_ids_and_escape_labels() {
        assert_eq!(sanitize_id("svc:demo-app"), "svc_demo_app");
        assert_eq!(sanitize_id("svc:db.internal:5432"), "svc_db_internal_5432");
        assert_eq!(sanitize_id("plain_ok_123"), "plain_ok_123");
        assert_eq!(escape_label("a \"b\" c"), "a #quot;b#quot; c");
    }
}
