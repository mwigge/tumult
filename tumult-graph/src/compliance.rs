//! `ComplianceArticle` nodes derived from the `tumult-core` citation registry.
//!
//! The registry ([`tumult_core::compliance::CITATIONS`]) is the single source
//! of truth for the regulatory controls a chaos experiment can supply evidence
//! toward. Each citation becomes one deterministic, static
//! [`NodeKind::ComplianceArticle`] node (`compliance:<FRAMEWORK>/<control>`).
//! These nodes are populated at store-open / schema-migration time, not per
//! journal, because they do not depend on any run.

use tumult_core::compliance::{Citation, ComplianceFramework, CITATIONS};

use crate::model::{Node, NodeKind};

/// The stable node id for a compliance article, e.g. `compliance:DORA/Art.25`.
/// Whitespace in the control id is removed so `Art. 25` → `Art.25`.
#[must_use]
pub fn compliance_article_id(framework: ComplianceFramework, control_id: &str) -> String {
    let control: String = control_id.split_whitespace().collect();
    format!("compliance:{}/{control}", framework.as_report_str())
}

/// Every compliance article node, one per registry citation. Deterministic and
/// independent of any run — safe to upsert on every store open.
#[must_use]
pub fn compliance_article_nodes() -> Vec<Node> {
    CITATIONS.iter().map(article_node).collect()
}

/// Build the [`Node`] for a single citation.
fn article_node(citation: &Citation) -> Node {
    Node {
        id: compliance_article_id(citation.framework, citation.control_id),
        kind: NodeKind::ComplianceArticle,
        label: format!(
            "{} {}",
            citation.framework.as_report_str(),
            citation.control_id
        ),
        attrs: serde_json::json!({
            "framework": citation.framework.as_report_str(),
            "control_id": citation.control_id,
            "title": citation.title,
            "evidence_type": citation.evidence_type.as_str(),
            "strength": citation.strength.as_str(),
            "source_url": citation.source_url,
            "last_verified": citation.last_verified,
        }),
    }
}

/// Resolve a declared `(framework, requirement_id)` pair to its registry
/// citation, if one exists. The framework accepts either the CLI name
/// (`dora`, `pci-dss`) or the report identifier (`DORA`, `PCI-DSS`),
/// case-insensitively; the control id is matched ignoring whitespace and case
/// (`Art. 25` == `art.25`). Returns `None` when nothing matches — callers must
/// not guess.
#[must_use]
pub fn resolve_citation(framework: &str, requirement_id: &str) -> Option<&'static Citation> {
    let fw = parse_framework(framework)?;
    let want = normalize_control(requirement_id);
    CITATIONS
        .iter()
        .find(|c| c.framework == fw && normalize_control(c.control_id) == want)
}

/// Parse a framework label in either its CLI-name or report-identifier form.
fn parse_framework(label: &str) -> Option<ComplianceFramework> {
    let trimmed = label.trim();
    ComplianceFramework::ALL.into_iter().find(|f| {
        f.name().eq_ignore_ascii_case(trimmed) || f.as_report_str().eq_ignore_ascii_case(trimmed)
    })
}

/// Normalise a control identifier for comparison: drop whitespace, lowercase.
fn normalize_control(id: &str) -> String {
    id.chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn article_id_strips_whitespace_from_control() {
        assert_eq!(
            compliance_article_id(ComplianceFramework::Dora, "Art. 25"),
            "compliance:DORA/Art.25"
        );
        assert_eq!(
            compliance_article_id(ComplianceFramework::PciDss, "Req. 12.10.2"),
            "compliance:PCI-DSS/Req.12.10.2"
        );
    }

    #[test]
    fn every_citation_yields_a_node_with_required_attrs() {
        let nodes = compliance_article_nodes();
        assert_eq!(nodes.len(), CITATIONS.len());
        for node in &nodes {
            assert_eq!(node.kind, NodeKind::ComplianceArticle);
            assert!(node.id.starts_with("compliance:"));
            for key in ["framework", "control_id", "strength", "source_url"] {
                assert!(
                    node.attrs.get(key).is_some(),
                    "node {} missing attr {key}",
                    node.id
                );
            }
        }
    }

    #[test]
    fn resolve_matches_by_name_or_report_id_and_normalised_control() {
        // Report id + spaced control.
        let c = resolve_citation("DORA", "Art. 25").expect("DORA Art. 25 resolves");
        assert_eq!(c.control_id, "Art. 25");
        // CLI name + de-spaced control, different case.
        let c = resolve_citation("dora", "art.25").expect("normalised match");
        assert_eq!(c.control_id, "Art. 25");
        // Unknown framework or control → None (no guessing).
        assert!(resolve_citation("hipaa", "Art. 25").is_none());
        assert!(resolve_citation("dora", "Art. 999").is_none());
    }
}
