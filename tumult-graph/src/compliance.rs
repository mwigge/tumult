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
/// case-insensitively. The control id is matched on its alphanumeric
/// skeleton (case, whitespace, dots and dashes ignored; parentheses kept so
/// `Art. 21(2)(b)` and `(c)` stay distinct), and a redundant leading
/// framework prefix is tolerated — `Art. 25`, `art25` and `DORA-Art25` all
/// resolve. Users write these ids by hand in experiment and gameday files;
/// a silently dropped compliance mapping is a worse failure than a lenient
/// match over an id space this small and distinctive. Returns `None` when
/// nothing matches — callers must not guess.
#[must_use]
pub fn resolve_citation(framework: &str, requirement_id: &str) -> Option<&'static Citation> {
    let fw = parse_framework(framework)?;
    let mut want = normalize_control(requirement_id);
    // Strip a redundant framework prefix ("DORA-Art25" declared under DORA).
    for label in [fw.name(), fw.as_report_str()] {
        let prefix = normalize_control(label);
        if let Some(rest) = want.strip_prefix(&prefix) {
            if !rest.is_empty() {
                want = rest.to_string();
            }
            break;
        }
    }
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

/// Normalise a control identifier to its comparison skeleton: lowercase,
/// keeping only alphanumerics and parentheses. `Art. 25`, `art25` and
/// `Art-25` collapse to `art25`; `Art. 21(2)(b)` keeps its parens and stays
/// distinct from `(c)`.
fn normalize_control(id: &str) -> String {
    id.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '(' || *c == ')')
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
        // Hand-written variants users actually produce must resolve too.
        for variant in ["art25", "Art.25", "DORA-Art25", "dora-art-25", "Art 25"] {
            assert_eq!(
                resolve_citation("DORA", variant).map(|c| c.control_id),
                Some("Art. 25"),
                "variant '{variant}' must resolve"
            );
        }
        // Parenthesised NIS2 sub-controls stay distinct.
        assert_ne!(
            resolve_citation("NIS2", "Art. 21(2)(b)").map(|c| c.control_id),
            resolve_citation("NIS2", "NIS2-Art21(2)(c)").map(|c| c.control_id),
        );
        assert!(resolve_citation("NIS2", "NIS2-Art21(2)(c)").is_some());
        // A bare framework prefix with no control does not match anything.
        assert!(resolve_citation("DORA", "DORA").is_none());
        assert_eq!(c.control_id, "Art. 25");
        // CLI name + de-spaced control, different case.
        let c = resolve_citation("dora", "art.25").expect("normalised match");
        assert_eq!(c.control_id, "Art. 25");
        // Unknown framework or control → None (no guessing).
        assert!(resolve_citation("hipaa", "Art. 25").is_none());
        assert!(resolve_citation("dora", "Art. 999").is_none());
    }
}
