//! Kubernetes Service discovery that *proposes* — never writes — topology.
//!
//! Tumult topology is declared, not guessed: the graph is only ever fed from
//! a reviewed TOML file (see `tumult-graph`'s `topology` module). This module
//! therefore stops one step short of the graph: it lists the cluster's
//! Services and renders a PROPOSED topology TOML for a human to review, edit
//! (Kubernetes does not know service *dependencies*, so every `depends_on`
//! is left empty), and only then feed to `tumult topology import`.
//!
//! Split for testability without a cluster:
//!
//! * [`reduce`] — pure `Service` → [`DiscoveredService`] mapping,
//! * [`proposed_topology_toml`] — pure rendering,
//! * [`discover_services`] — the thin async list-and-map shell, exercised
//!   against the fake-apiserver harness in `tests/fake_apiserver.rs`.

use k8s_openapi::api::core::v1::Service;
use kube::api::{Api, ListParams};
use kube::Client;

use crate::error::KubeError;

/// Preferred label for a service's tier.
const TIER_LABEL: &str = "tumult.io/tier";
/// Fallback tier label (the Kubernetes recommended-labels component).
const TIER_FALLBACK_LABEL: &str = "app.kubernetes.io/component";
/// Label naming the owning team. Deliberately the *only* owner source:
/// `app.kubernetes.io/managed-by` names a deploy tool (helm, argocd), not a
/// team, so it is never used.
const OWNER_LABEL: &str = "tumult.io/owner";
/// Selector keys whose values name the backing app; surfaced as review
/// context in the proposed TOML's comments.
const SELECTOR_APP_KEYS: &[&str] = &["app", "app.kubernetes.io/name"];

/// One discovered Kubernetes Service, reduced to topology-relevant facts.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DiscoveredService {
    /// The Service name — the name the cluster's DNS resolves, so it is what
    /// a declared topology entry should be called.
    pub name: String,
    pub namespace: String,
    /// From the `tumult.io/tier` label, falling back to
    /// `app.kubernetes.io/component`.
    pub tier: Option<String>,
    /// From the `tumult.io/owner` label only (team labels, never tool labels).
    pub owner: Option<String>,
    /// `spec.selector` values under `app` / `app.kubernetes.io/name`, for
    /// context comments in the proposed TOML.
    pub selector_apps: Vec<String>,
}

/// Build a client from the ambient credentials `kube-rs` discovers
/// (in-cluster service account, `KUBECONFIG`, or `~/.kube/config`).
///
/// Exposed here so callers (the CLI) do not need a direct `kube` dependency.
///
/// # Errors
///
/// Returns [`KubeError`] when no kubeconfig or in-cluster credentials can be
/// inferred.
pub async fn default_client() -> Result<Client, KubeError> {
    Ok(Client::try_default().await?)
}

/// List Services and reduce them to [`DiscoveredService`] facts.
///
/// With an empty `namespaces` the whole cluster is scanned, skipping
/// `kube-system` (request it explicitly to include it). The apiserver's own
/// `kubernetes` Service in `default` is always skipped — it is cluster
/// plumbing, never a workload. Output order is deterministic:
/// (namespace, name), deduplicated.
///
/// # Errors
///
/// Returns [`KubeError`] if a Kubernetes API call fails.
#[must_use = "callers must render or inspect the discovered services"]
pub async fn discover_services(
    client: Client,
    namespaces: &[String],
) -> Result<Vec<DiscoveredService>, KubeError> {
    let _span = crate::telemetry::begin_discover_services(namespaces.len());
    let params = ListParams::default();
    let mut services: Vec<DiscoveredService> = Vec::new();

    if namespaces.is_empty() {
        let api: Api<Service> = Api::all(client);
        services.extend(
            api.list(&params)
                .await?
                .into_iter()
                .filter_map(reduce)
                .filter(|svc| svc.namespace != "kube-system"),
        );
    } else {
        for namespace in namespaces {
            let api: Api<Service> = Api::namespaced(client.clone(), namespace);
            services.extend(api.list(&params).await?.into_iter().filter_map(reduce));
        }
    }

    services.retain(|svc| !is_apiserver_service(svc));
    sort_and_dedup(&mut services);
    Ok(services)
}

/// Render discovered services as a PROPOSED topology TOML for human review.
///
/// Pure — testable without a cluster. The output is a valid document for
/// `tumult_graph`'s `parse_topology` (round-trip covered by tests): one
/// `[[service]]` block per unique service name, `depends_on = []` with a
/// comment explaining that dependencies must be filled in by humans, and
/// `tier`/`owner` when the labels were present. When the same service name
/// appears in several namespaces, only the first (namespace-sorted) block is
/// active; the rest are emitted commented-out so the reviewer resolves the
/// clash instead of the tool guessing.
#[must_use]
pub fn proposed_topology_toml(services: &[DiscoveredService]) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    out.push_str("# proposed by tumult topology discover-k8s — REVIEW before import\n");
    out.push_str("#\n");
    out.push_str("# Discovery is input to the reviewed topology file, not truth: Kubernetes\n");
    out.push_str("# knows which Services exist, but not which service depends on which, so\n");
    out.push_str("# every depends_on below is empty and must be filled in by a human before\n");
    out.push_str("# running `tumult topology import`.\n");

    if services.is_empty() {
        out.push_str("#\n# no services discovered — nothing to propose\n");
        return out;
    }

    let mut seen_names: Vec<&str> = Vec::new();
    for svc in services {
        let duplicate = seen_names.contains(&svc.name.as_str());
        if !duplicate {
            seen_names.push(&svc.name);
        }

        out.push('\n');
        let prefix = if duplicate { "# " } else { "" };
        if duplicate {
            out.push_str("# DUPLICATE NAME: a service with this name was already proposed from\n");
            out.push_str("# another namespace — resolve manually before importing.\n");
        }
        // Writing into a String is infallible.
        let _ = write!(out, "{prefix}# namespace: {}", svc.namespace);
        if !svc.selector_apps.is_empty() {
            let _ = write!(out, " — selects app(s): {}", svc.selector_apps.join(", "));
        }
        out.push('\n');
        let _ = writeln!(out, "{prefix}[[service]]");
        let _ = writeln!(out, "{prefix}name = {}", toml_string(&svc.name));
        let _ = writeln!(
            out,
            "{prefix}depends_on = [] # not discoverable from Kubernetes — fill in by hand"
        );
        if let Some(tier) = &svc.tier {
            let _ = writeln!(out, "{prefix}tier = {}", toml_string(tier));
        }
        if let Some(owner) = &svc.owner {
            let _ = writeln!(out, "{prefix}owner = {}", toml_string(owner));
        }
    }
    out
}

/// Reduce one `Service` object to topology-relevant facts.
///
/// Pure, so label extraction is unit-testable against constructed
/// `k8s_openapi` objects. Returns `None` for objects without a name (never
/// produced by a real apiserver, but the field is optional in the schema).
#[must_use]
pub fn reduce(service: Service) -> Option<DiscoveredService> {
    let name = service.metadata.name?;
    let namespace = service
        .metadata
        .namespace
        .unwrap_or_else(|| "default".to_string());

    let labels = service.metadata.labels.unwrap_or_default();
    let tier = labels
        .get(TIER_LABEL)
        .or_else(|| labels.get(TIER_FALLBACK_LABEL))
        .cloned();
    let owner = labels.get(OWNER_LABEL).cloned();

    let selector = service.spec.and_then(|s| s.selector).unwrap_or_default();
    let mut selector_apps: Vec<String> = SELECTOR_APP_KEYS
        .iter()
        .filter_map(|key| selector.get(*key).cloned())
        .collect();
    selector_apps.dedup();

    Some(DiscoveredService {
        name,
        namespace,
        tier,
        owner,
        selector_apps,
    })
}

/// The apiserver's own `kubernetes` Service in the `default` namespace.
fn is_apiserver_service(svc: &DiscoveredService) -> bool {
    svc.namespace == "default" && svc.name == "kubernetes"
}

/// Deterministic (namespace, name) order; drops exact duplicates (e.g. the
/// same namespace passed twice on the command line).
fn sort_and_dedup(services: &mut Vec<DiscoveredService>) {
    services.sort_by(|a, b| {
        (a.namespace.as_str(), a.name.as_str()).cmp(&(b.namespace.as_str(), b.name.as_str()))
    });
    services.dedup_by(|a, b| a.namespace == b.namespace && a.name == b.name);
}

/// Quote a value as a TOML basic string. Kubernetes names and label values
/// cannot contain quotes or backslashes, but escaping keeps the renderer
/// total instead of trusting that.
fn toml_string(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn service_json(namespace: &str, name: &str) -> serde_json::Value {
        serde_json::json!({
            "apiVersion": "v1",
            "kind": "Service",
            "metadata": { "name": name, "namespace": namespace },
            "spec": { "ports": [{ "port": 80 }] },
        })
    }

    fn svc(namespace: &str, name: &str) -> DiscoveredService {
        DiscoveredService {
            name: name.to_string(),
            namespace: namespace.to_string(),
            tier: None,
            owner: None,
            selector_apps: Vec::new(),
        }
    }

    // ── reduce (pure Service → DiscoveredService mapping) ─────

    #[test]
    fn reduce_extracts_tumult_labels_and_selector_apps() {
        let mut value = service_json("prod", "checkout");
        value["metadata"]["labels"] = serde_json::json!({
            "tumult.io/tier": "edge",
            "tumult.io/owner": "team-payments",
            "app.kubernetes.io/component": "ignored-when-tumult-tier-present",
        });
        value["spec"]["selector"] = serde_json::json!({
            "app": "checkout-app",
            "app.kubernetes.io/name": "checkout-chart",
        });
        let service: Service = serde_json::from_value(value).unwrap();

        let discovered = reduce(service).expect("named service reduces");
        assert_eq!(discovered.name, "checkout");
        assert_eq!(discovered.namespace, "prod");
        assert_eq!(discovered.tier.as_deref(), Some("edge"));
        assert_eq!(discovered.owner.as_deref(), Some("team-payments"));
        assert_eq!(
            discovered.selector_apps,
            vec!["checkout-app", "checkout-chart"]
        );
    }

    #[test]
    fn reduce_falls_back_to_component_label_for_tier() {
        let mut value = service_json("prod", "db");
        value["metadata"]["labels"] =
            serde_json::json!({ "app.kubernetes.io/component": "database" });
        let service: Service = serde_json::from_value(value).unwrap();

        let discovered = reduce(service).unwrap();
        assert_eq!(discovered.tier.as_deref(), Some("database"));
        assert_eq!(
            discovered.owner, None,
            "owner comes from tumult.io/owner only"
        );
    }

    #[test]
    fn reduce_never_takes_owner_from_managed_by() {
        let mut value = service_json("prod", "web");
        value["metadata"]["labels"] = serde_json::json!({ "app.kubernetes.io/managed-by": "helm" });
        let service: Service = serde_json::from_value(value).unwrap();
        assert_eq!(reduce(service).unwrap().owner, None);
    }

    #[test]
    fn reduce_without_labels_or_selector_yields_bare_service() {
        let service: Service = serde_json::from_value(service_json("staging", "cache")).unwrap();
        let discovered = reduce(service).unwrap();
        assert_eq!(discovered, svc("staging", "cache"));
    }

    #[test]
    fn reduce_without_name_yields_none() {
        let service = Service {
            metadata: kube::api::ObjectMeta {
                namespace: Some("prod".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(reduce(service).is_none());
    }

    #[test]
    fn reduce_headless_selectorless_service_has_no_apps() {
        // ExternalName / manually-endpointed services have no selector at all.
        let service: Service = serde_json::from_value(serde_json::json!({
            "apiVersion": "v1",
            "kind": "Service",
            "metadata": { "name": "legacy-oracle", "namespace": "prod" },
            "spec": { "type": "ExternalName", "externalName": "oracle.corp.example" },
        }))
        .unwrap();
        assert!(reduce(service).unwrap().selector_apps.is_empty());
    }

    // ── skip list + ordering ──────────────────────────────────

    #[test]
    fn apiserver_service_is_recognized_only_in_default_namespace() {
        assert!(is_apiserver_service(&svc("default", "kubernetes")));
        assert!(!is_apiserver_service(&svc("prod", "kubernetes")));
        assert!(!is_apiserver_service(&svc("default", "web")));
    }

    #[test]
    fn sort_and_dedup_orders_by_namespace_then_name_and_drops_repeats() {
        let mut services = vec![
            svc("prod", "web"),
            svc("infra", "vault"),
            svc("prod", "api"),
            svc("prod", "web"), // same namespace listed twice on the CLI
        ];
        sort_and_dedup(&mut services);
        let order: Vec<(&str, &str)> = services
            .iter()
            .map(|s| (s.namespace.as_str(), s.name.as_str()))
            .collect();
        assert_eq!(
            order,
            vec![("infra", "vault"), ("prod", "api"), ("prod", "web")]
        );
    }

    // ── proposed_topology_toml (pure rendering) ───────────────

    fn labeled(namespace: &str, name: &str, tier: &str, owner: &str) -> DiscoveredService {
        DiscoveredService {
            tier: Some(tier.to_string()),
            owner: Some(owner.to_string()),
            selector_apps: vec![format!("{name}-app")],
            ..svc(namespace, name)
        }
    }

    #[test]
    fn proposed_toml_round_trips_through_parse_topology() {
        let services = vec![
            labeled("prod", "api", "service", "team-core"),
            svc("prod", "db"),
            labeled("prod", "gateway", "edge", "team-edge"),
        ];
        let toml = proposed_topology_toml(&services);

        let doc = tumult_graph::topology::parse_topology(&toml)
            .expect("proposed TOML must be a valid topology document");
        assert_eq!(doc.services.len(), 3);
        assert_eq!(doc.services[0].name, "api");
        assert_eq!(doc.services[0].depends_on, Vec::<String>::new());
        assert_eq!(doc.services[0].tier.as_deref(), Some("service"));
        assert_eq!(doc.services[0].owner.as_deref(), Some("team-core"));
        assert_eq!(doc.services[1].tier, None);
        assert_eq!(doc.services[1].owner, None);
    }

    #[test]
    fn proposed_toml_carries_review_header_and_context_comments() {
        let toml = proposed_topology_toml(&[labeled("prod", "api", "service", "team-core")]);
        assert!(
            toml.starts_with("# proposed by tumult topology discover-k8s — REVIEW before import\n")
        );
        assert!(toml.contains("# namespace: prod — selects app(s): api-app\n"));
        assert!(
            toml.contains("depends_on = [] # not discoverable from Kubernetes"),
            "must tell the reviewer to fill dependencies in:\n{toml}"
        );
    }

    #[test]
    fn duplicate_names_across_namespaces_are_commented_out_and_still_parse() {
        let services = vec![svc("prod", "web"), svc("staging", "web")];
        let toml = proposed_topology_toml(&services);

        assert!(toml.contains("# DUPLICATE NAME"), "toml:\n{toml}");
        assert!(
            toml.contains("# [[service]]\n# name = \"web\""),
            "toml:\n{toml}"
        );
        let doc = tumult_graph::topology::parse_topology(&toml).expect("still valid");
        assert_eq!(doc.services.len(), 1, "only the first web is active");
    }

    #[test]
    fn empty_discovery_renders_header_only_note() {
        let toml = proposed_topology_toml(&[]);
        assert!(toml.contains("no services discovered"));
        assert!(!toml.contains("[[service]]"));
    }

    #[test]
    fn rendering_is_deterministic() {
        let services = vec![
            labeled("prod", "api", "service", "team-core"),
            svc("prod", "db"),
        ];
        assert_eq!(
            proposed_topology_toml(&services),
            proposed_topology_toml(&services)
        );
    }

    #[test]
    fn toml_string_escapes_quotes_and_backslashes() {
        assert_eq!(toml_string("plain"), "\"plain\"");
        assert_eq!(toml_string("a\"b\\c"), "\"a\\\"b\\\\c\"");
    }

    #[test]
    fn tier_and_owner_survive_the_round_trip_via_btreemap_labels() {
        // Construct via the typed API too (not only JSON) to pin the label
        // lookup against k8s-openapi's BTreeMap representation.
        let mut labels = BTreeMap::new();
        labels.insert("tumult.io/tier".to_string(), "data".to_string());
        labels.insert("tumult.io/owner".to_string(), "team-db".to_string());
        let service = Service {
            metadata: kube::api::ObjectMeta {
                name: Some("postgres".into()),
                namespace: Some("prod".into()),
                labels: Some(labels),
                ..Default::default()
            },
            ..Default::default()
        };
        let discovered = reduce(service).unwrap();
        let doc =
            tumult_graph::topology::parse_topology(&proposed_topology_toml(&[discovered])).unwrap();
        assert_eq!(doc.services[0].tier.as_deref(), Some("data"));
        assert_eq!(doc.services[0].owner.as_deref(), Some("team-db"));
    }
}
