//! Declared service topology: parse a TOML document into [`GraphDelta`]
//! rows, mirroring the coverage-gap pattern — a sentinel `run_id` keeps
//! re-imports idempotent, and this module never touches a database.
//!
//! Topology is *declared*, never guessed: services and their `depends_on`
//! edges come from a reviewed file (default `~/.tumult/topology.toml`), so
//! the map's shape is as trustworthy as the repo that commits it. Service
//! names are normalized exactly like run-derived hosts so `svc:` ids from
//! experiment runs collide (join) with declared ones.

use serde::Deserialize;
use std::collections::HashSet;

use crate::model::{Edge, EdgeRel, GraphDelta, Node, NodeKind};
use crate::service::normalize_service;

/// Sentinel `run_id` under which declared-topology edges are stored, so the
/// whole topology sub-graph can be cleared and re-imported on demand.
pub const TOPOLOGY_RUN_ID: &str = "__topology__";

/// A parsed topology document.
#[derive(Debug, Clone, Deserialize)]
pub struct TopologyDoc {
    #[serde(default, rename = "service")]
    pub services: Vec<TopologyService>,
}

/// One declared service.
#[derive(Debug, Clone, Deserialize)]
pub struct TopologyService {
    /// Service name; normalized to the same form run-derived services use.
    pub name: String,
    /// Names of services this one depends on (must be declared in the doc).
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub owner: Option<String>,
    /// Free-form tier tag (e.g. `edge`, `service`, `data`).
    #[serde(default)]
    pub tier: Option<String>,
}

#[derive(Debug)]
pub enum TopologyError {
    /// The TOML failed to parse.
    Parse(String),
    /// A `depends_on` entry names a service not declared in the document.
    UnknownDependency { service: String, dependency: String },
    /// Two `[[service]]` blocks normalize to the same name.
    Duplicate(String),
    /// The document declares no services at all.
    Empty,
}

impl std::fmt::Display for TopologyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(err) => write!(f, "topology TOML parse error: {err}"),
            Self::UnknownDependency { service, dependency } => write!(
                f,
                "service '{service}' depends on '{dependency}', which is not declared"
            ),
            Self::Duplicate(name) => write!(f, "service '{name}' is declared twice"),
            Self::Empty => write!(f, "topology document declares no [[service]] blocks"),
        }
    }
}

impl std::error::Error for TopologyError {}

/// Parse and validate a topology TOML document.
///
/// Cycles in `depends_on` are allowed — real systems have them; consumers
/// must traverse with a visited set.
pub fn parse_topology(toml_text: &str) -> Result<TopologyDoc, TopologyError> {
    let mut doc: TopologyDoc =
        toml::from_str(toml_text).map_err(|err| TopologyError::Parse(err.to_string()))?;
    if doc.services.is_empty() {
        return Err(TopologyError::Empty);
    }

    for service in &mut doc.services {
        service.name = normalize_service(&service.name);
        for dep in &mut service.depends_on {
            *dep = normalize_service(dep);
        }
    }

    let mut names: HashSet<&str> = HashSet::new();
    for service in &doc.services {
        if !names.insert(service.name.as_str()) {
            return Err(TopologyError::Duplicate(service.name.clone()));
        }
    }
    for service in &doc.services {
        for dep in &service.depends_on {
            if !names.contains(dep.as_str()) {
                return Err(TopologyError::UnknownDependency {
                    service: service.name.clone(),
                    dependency: dep.clone(),
                });
            }
        }
    }
    Ok(doc)
}

/// Turn a validated document into graph rows: one `svc:` node per service
/// (attrs mark it `declared` and carry owner/tier) plus `depends_on` edges.
/// Deterministic and deduplicated.
#[must_use]
pub fn topology_delta(doc: &TopologyDoc) -> GraphDelta {
    let mut delta = GraphDelta::default();
    let mut seen_edges = HashSet::new();

    for service in &doc.services {
        let mut attrs = serde_json::json!({ "declared": true });
        if let Some(owner) = &service.owner {
            attrs["owner"] = serde_json::json!(owner);
        }
        if let Some(tier) = &service.tier {
            attrs["tier"] = serde_json::json!(tier);
        }
        delta.nodes.push(Node {
            id: format!("svc:{}", service.name),
            kind: NodeKind::Service,
            label: service.name.clone(),
            attrs,
        });
        for dep in &service.depends_on {
            let src = format!("svc:{}", service.name);
            let dst = format!("svc:{dep}");
            if seen_edges.insert((src.clone(), dst.clone())) {
                delta.edges.push(Edge {
                    src,
                    rel: EdgeRel::DependsOn,
                    dst,
                    attrs: serde_json::json!({}),
                });
            }
        }
    }
    delta
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEMO: &str = r#"
        [[service]]
        name = "gateway:8080"
        depends_on = ["api"]
        tier = "edge"

        [[service]]
        name = "api"
        depends_on = ["db"]
        owner = "team-core"

        [[service]]
        name = "db"
        tier = "data"
    "#;

    #[test]
    fn parses_and_normalizes() {
        let doc = parse_topology(DEMO).unwrap();
        assert_eq!(doc.services.len(), 3);
        // ":8080" is stripped exactly like run-derived hosts; case is preserved.
        assert_eq!(doc.services[0].name, "gateway");
        assert_eq!(doc.services[0].depends_on, vec!["api"]);
    }

    #[test]
    fn delta_shape() {
        let doc = parse_topology(DEMO).unwrap();
        let delta = topology_delta(&doc);
        assert_eq!(delta.nodes.len(), 3);
        assert_eq!(delta.edges.len(), 2);
        assert!(delta.nodes.iter().all(|n| n.kind == NodeKind::Service));
        assert!(delta.nodes.iter().all(|n| n.attrs["declared"] == true));
        let edge = &delta.edges[0];
        assert_eq!(edge.src, "svc:gateway");
        assert_eq!(edge.rel, EdgeRel::DependsOn);
        assert_eq!(edge.dst, "svc:api");
        assert_eq!(delta.nodes[1].attrs["owner"], "team-core");
    }

    #[test]
    fn unknown_dependency_rejected() {
        let bad = "[[service]]\nname = \"a\"\ndepends_on = [\"ghost\"]\n";
        assert!(matches!(
            parse_topology(bad),
            Err(TopologyError::UnknownDependency { .. })
        ));
    }

    #[test]
    fn duplicates_and_empty_rejected() {
        let dup = "[[service]]\nname = \"a\"\n[[service]]\nname = \"a:9090\"\n";
        assert!(matches!(parse_topology(dup), Err(TopologyError::Duplicate(_))));
        assert!(matches!(parse_topology(""), Err(TopologyError::Empty)));
    }

    #[test]
    fn cycles_are_allowed() {
        let cyclic = "[[service]]\nname=\"a\"\ndepends_on=[\"b\"]\n[[service]]\nname=\"b\"\ndepends_on=[\"a\"]\n";
        assert!(parse_topology(cyclic).is_ok());
    }
}
