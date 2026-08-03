//! Fault catalog derived from the live plugin catalog.
//!
//! The catalog is built from [`tumult_plugin::discovery::discover_all_plugins`]
//! so it never drifts from the shipped plugins. Each discovered plugin's
//! actions and probes are grouped into a fault [`Domain`] via a small curated
//! `plugin -> domain` map (unknown plugins fall into [`Domain::Other`]).
//!
//! Script-plugin manifests do not declare per-action arguments, so each
//! action's arguments come from a curated, documented fallback list keyed by
//! `plugin::action` (see [`documented_args`]). This mirrors the `TUMULT_*`
//! environment variables the plugin scripts actually read.

use serde::{Deserialize, Serialize};

use tumult_plugin::discovery::{
    discover_all_plugins, discover_all_plugins_with_config, DiscoveryError, PluginDiscoveryConfig,
};

/// A fault domain groups related plugins (Network, Database, …). The name is
/// stable and used verbatim in the CLI picker and the MCP catalog tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Domain {
    Network,
    Database,
    State,
    Resource,
    Process,
    Container,
    Time,
    Messaging,
    Load,
    Agentic,
    Other,
}

impl Domain {
    /// Stable human-readable label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Network => "Network",
            Self::Database => "Database",
            Self::State => "State",
            Self::Resource => "Resource",
            Self::Process => "Process",
            Self::Container => "Container",
            Self::Time => "Time",
            Self::Messaging => "Messaging",
            Self::Load => "Load",
            Self::Agentic => "Agentic",
            Self::Other => "Other",
        }
    }

    /// A short tag suitable for the experiment `tags` list.
    #[must_use]
    pub fn tag(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::Database => "database",
            Self::State => "state",
            Self::Resource => "resource",
            Self::Process => "process",
            Self::Container => "container",
            Self::Time => "time",
            Self::Messaging => "messaging",
            Self::Load => "load",
            Self::Agentic => "agentic",
            Self::Other => "other",
        }
    }
}

/// Curated `plugin -> domain` map. Unknown plugins map to [`Domain::Other`].
#[must_use]
pub fn domain_for(plugin: &str) -> Domain {
    match plugin {
        "tumult-network" => Domain::Network,
        "tumult-db-postgres" | "tumult-db-mysql" => Domain::Database,
        "tumult-db-redis" => Domain::State,
        "tumult-stress" => Domain::Resource,
        "tumult-process" => Domain::Process,
        "tumult-containers" | "tumult-pumba" => Domain::Container,
        "tumult-timewarp" => Domain::Time,
        "tumult-kafka" => Domain::Messaging,
        "tumult-loadtest" => Domain::Load,
        "tumult-agentic" => Domain::Agentic,
        _ => Domain::Other,
    }
}

/// Whether a catalog entry is a fault-injecting action or a steady-state probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ActionKind {
    Action,
    Probe,
}

/// A declared argument for a catalog action, surfaced to the picker/tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogArg {
    pub name: String,
    pub required: bool,
    pub description: String,
}

impl CatalogArg {
    fn new(name: &str, required: bool, description: &str) -> Self {
        Self {
            name: name.to_string(),
            required,
            description: description.to_string(),
        }
    }
}

/// A single fault action or probe in the catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogAction {
    /// Owning plugin, e.g. `tumult-network`.
    pub plugin: String,
    /// Action/probe name, e.g. `add-latency`.
    pub name: String,
    /// Human description from the plugin manifest.
    pub description: String,
    /// Whether this is an `action` (fault) or a `probe`.
    pub kind: ActionKind,
    /// Documented arguments (the manifest declares none per-action, so these
    /// come from the curated fallback list).
    pub args: Vec<CatalogArg>,
}

/// A fault domain and the actions grouped under it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogDomain {
    pub domain: Domain,
    /// Domain label (redundant with `domain` but convenient for the tool
    /// output and templating).
    pub label: String,
    pub actions: Vec<CatalogAction>,
}

/// The full fault catalog: domains, each with their actions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaultCatalog {
    pub domains: Vec<CatalogDomain>,
}

impl FaultCatalog {
    /// Total number of actions and probes across all domains.
    #[must_use]
    pub fn action_count(&self) -> usize {
        self.domains.iter().map(|d| d.actions.len()).sum()
    }

    /// Every action across all domains, flattened.
    pub fn all_actions(&self) -> impl Iterator<Item = &CatalogAction> {
        self.domains.iter().flat_map(|d| d.actions.iter())
    }

    /// Find an action by `plugin` and `name`.
    #[must_use]
    pub fn find(&self, plugin: &str, action: &str) -> Option<&CatalogAction> {
        self.all_actions()
            .find(|a| a.plugin == plugin && a.name == action)
    }

    /// Whether the catalog contains any actions.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.action_count() == 0
    }
}

/// Build the fault catalog from the live plugin discovery paths (cwd-relative
/// `./plugins`, `~/.tumult/plugins`, and `TUMULT_PLUGIN_PATH`).
///
/// # Errors
///
/// Returns [`DiscoveryError`] if a plugin directory cannot be read or a
/// manifest is malformed.
pub fn build_catalog() -> Result<FaultCatalog, DiscoveryError> {
    let manifests = discover_all_plugins()?;
    Ok(catalog_from_manifests(&manifests))
}

/// Build the fault catalog using an explicit discovery config. Used by tests
/// and callers that know exactly where the shipped `plugins/` directory lives.
///
/// # Errors
///
/// Returns [`DiscoveryError`] if a plugin directory cannot be read or a
/// manifest is malformed.
pub fn build_catalog_with_config(
    config: &PluginDiscoveryConfig,
) -> Result<FaultCatalog, DiscoveryError> {
    let manifests = discover_all_plugins_with_config(config)?;
    Ok(catalog_from_manifests(&manifests))
}

fn catalog_from_manifests(manifests: &[tumult_plugin::ScriptPluginManifest]) -> FaultCatalog {
    use std::collections::BTreeMap;

    // Group actions/probes by domain, preserving a stable domain order.
    let mut by_domain: BTreeMap<i32, Vec<CatalogAction>> = BTreeMap::new();

    for manifest in manifests {
        let domain = domain_for(&manifest.name);
        let bucket = by_domain.entry(domain_order(domain)).or_default();
        for action in &manifest.actions {
            bucket.push(CatalogAction {
                plugin: manifest.name.clone(),
                name: action.name.clone(),
                description: action.description.clone(),
                kind: ActionKind::Action,
                args: documented_args(&manifest.name, &action.name),
            });
        }
        for probe in &manifest.probes {
            bucket.push(CatalogAction {
                plugin: manifest.name.clone(),
                name: probe.name.clone(),
                description: probe.description.clone(),
                kind: ActionKind::Probe,
                args: documented_args(&manifest.name, &probe.name),
            });
        }
    }

    let domains = by_domain
        .into_iter()
        .map(|(order, mut actions)| {
            actions.sort_by(|a, b| a.plugin.cmp(&b.plugin).then(a.name.cmp(&b.name)));
            let domain = domain_from_order(order);
            CatalogDomain {
                domain,
                label: domain.label().to_string(),
                actions,
            }
        })
        .collect();

    FaultCatalog { domains }
}

/// Stable ordering weight for a domain, so catalog output is deterministic.
fn domain_order(domain: Domain) -> i32 {
    match domain {
        Domain::Network => 0,
        Domain::Database => 1,
        Domain::State => 2,
        Domain::Resource => 3,
        Domain::Process => 4,
        Domain::Container => 5,
        Domain::Time => 6,
        Domain::Messaging => 7,
        Domain::Load => 8,
        Domain::Agentic => 9,
        Domain::Other => 10,
    }
}

fn domain_from_order(order: i32) -> Domain {
    match order {
        0 => Domain::Network,
        1 => Domain::Database,
        2 => Domain::State,
        3 => Domain::Resource,
        4 => Domain::Process,
        5 => Domain::Container,
        6 => Domain::Time,
        7 => Domain::Messaging,
        8 => Domain::Load,
        9 => Domain::Agentic,
        _ => Domain::Other,
    }
}

/// Documented argument list for `plugin::action`.
///
/// Script-plugin manifests carry no per-action argument schema, so this
/// curated map mirrors the `TUMULT_*` environment variables the plugin
/// scripts read. Actions without a specific entry fall back to a per-plugin
/// default, and finally to a single `target` argument.
#[must_use]
pub fn documented_args(plugin: &str, action: &str) -> Vec<CatalogArg> {
    match (plugin, action) {
        ("tumult-network", "add-latency" | "delay-dns") => vec![
            CatalogArg::new("delay_ms", true, "Added latency in milliseconds"),
            CatalogArg::new("jitter_ms", false, "Latency jitter in milliseconds"),
            CatalogArg::new("interface", false, "Network interface (default: eth0)"),
            CatalogArg::new(
                "target_ip",
                false,
                "Restrict the fault to this destination IP",
            ),
        ],
        ("tumult-network", "add-packet-loss") => vec![
            CatalogArg::new("loss_pct", true, "Packet loss percentage"),
            CatalogArg::new("interface", false, "Network interface (default: eth0)"),
        ],
        ("tumult-stress", "cpu-stress") => vec![
            CatalogArg::new("workers", true, "Number of CPU worker threads"),
            CatalogArg::new("load", false, "Target CPU load percentage per worker"),
            CatalogArg::new("timeout", false, "Duration, e.g. 30s"),
        ],
        ("tumult-stress", "memory-stress") => vec![
            CatalogArg::new("workers", true, "Number of memory worker threads"),
            CatalogArg::new("bytes", false, "Memory to allocate per worker, e.g. 256M"),
            CatalogArg::new("timeout", false, "Duration, e.g. 30s"),
        ],
        ("tumult-timewarp", "skew-clock") => vec![
            CatalogArg::new("skew_seconds", true, "Seconds to skew the clock by"),
            CatalogArg::new(
                "target",
                false,
                "Command or process to run under the skewed clock",
            ),
            CatalogArg::new("faketime_cmd", false, "Path to the libfaketime wrapper"),
        ],
        _ => plugin_default_args(plugin),
    }
}

/// Per-plugin default argument list for actions without a specific entry.
fn plugin_default_args(plugin: &str) -> Vec<CatalogArg> {
    match plugin {
        "tumult-network" => vec![
            CatalogArg::new("interface", false, "Network interface (default: eth0)"),
            CatalogArg::new(
                "target_ip",
                false,
                "Restrict the fault to this destination IP",
            ),
        ],
        "tumult-stress" => vec![
            CatalogArg::new("workers", false, "Number of worker threads"),
            CatalogArg::new("timeout", false, "Duration, e.g. 30s"),
        ],
        "tumult-db-postgres" => vec![
            CatalogArg::new("pg_host", true, "PostgreSQL host"),
            CatalogArg::new("pg_port", false, "PostgreSQL port (default: 5432)"),
            CatalogArg::new("pg_user", false, "PostgreSQL user"),
            CatalogArg::new("pg_database", false, "Target database"),
            CatalogArg::new("pg_password", false, "Password (prefer a secret reference)"),
        ],
        "tumult-db-mysql" => vec![
            CatalogArg::new("mysql_host", true, "MySQL host"),
            CatalogArg::new("mysql_port", false, "MySQL port (default: 3306)"),
            CatalogArg::new("mysql_user", false, "MySQL user"),
        ],
        "tumult-db-redis" => vec![
            CatalogArg::new("redis_host", true, "Redis host"),
            CatalogArg::new("redis_port", false, "Redis port (default: 6379)"),
            CatalogArg::new("redis_auth", false, "Redis auth password"),
        ],
        "tumult-containers" | "tumult-pumba" => vec![
            CatalogArg::new("container_id", true, "Target container id or name"),
            CatalogArg::new("runtime", false, "Container runtime (docker/podman)"),
        ],
        "tumult-timewarp" => vec![CatalogArg::new(
            "target",
            false,
            "Command or process to affect",
        )],
        "tumult-kafka" => vec![
            CatalogArg::new("broker", true, "Target Kafka broker"),
            CatalogArg::new("topic", false, "Target topic"),
        ],
        "tumult-process" => vec![CatalogArg::new("process", true, "Process name or pattern")],
        _ => vec![CatalogArg::new("target", true, "Target of the fault")],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_mapping_is_curated() {
        assert_eq!(domain_for("tumult-network"), Domain::Network);
        assert_eq!(domain_for("tumult-db-postgres"), Domain::Database);
        assert_eq!(domain_for("tumult-db-redis"), Domain::State);
        assert_eq!(domain_for("tumult-stress"), Domain::Resource);
        assert_eq!(domain_for("tumult-timewarp"), Domain::Time);
        assert_eq!(domain_for("some-third-party"), Domain::Other);
    }

    #[test]
    fn documented_args_have_a_required_field_for_known_actions() {
        let args = documented_args("tumult-network", "add-latency");
        assert!(args.iter().any(|a| a.name == "delay_ms" && a.required));
    }

    #[test]
    fn documented_args_fall_back_to_target() {
        let args = documented_args("unknown-plugin", "whatever");
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].name, "target");
    }

    #[test]
    fn catalog_from_manifests_groups_by_domain() {
        use std::path::PathBuf;
        use tumult_plugin::manifest::{ScriptAction, ScriptProbe};
        use tumult_plugin::ScriptPluginManifest;

        let manifests = vec![ScriptPluginManifest {
            name: "tumult-network".into(),
            version: "0.1.0".into(),
            description: "net".into(),
            actions: vec![ScriptAction {
                name: "add-latency".into(),
                script: PathBuf::from("actions/add-latency.sh"),
                description: "add latency".into(),
            }],
            probes: vec![ScriptProbe {
                name: "ping-latency".into(),
                script: PathBuf::from("probes/ping-latency.sh"),
                description: "ping".into(),
            }],
        }];

        let catalog = catalog_from_manifests(&manifests);
        assert_eq!(catalog.domains.len(), 1);
        assert_eq!(catalog.domains[0].domain, Domain::Network);
        assert_eq!(catalog.action_count(), 2);
        let latency = catalog.find("tumult-network", "add-latency").unwrap();
        assert_eq!(latency.kind, ActionKind::Action);
        assert!(latency.args.iter().any(|a| a.name == "delay_ms"));
    }

    #[test]
    fn every_domain_has_a_distinct_label_and_tag() {
        let domains = [
            Domain::Network,
            Domain::Database,
            Domain::State,
            Domain::Resource,
            Domain::Process,
            Domain::Container,
            Domain::Time,
            Domain::Messaging,
            Domain::Load,
            Domain::Agentic,
            Domain::Other,
        ];
        let labels: std::collections::HashSet<&str> = domains.iter().map(|d| d.label()).collect();
        let tags: std::collections::HashSet<&str> = domains.iter().map(|d| d.tag()).collect();
        assert_eq!(labels.len(), domains.len());
        assert_eq!(tags.len(), domains.len());
        assert_eq!(Domain::Agentic.label(), "Agentic");
        assert_eq!(Domain::Other.tag(), "other");
    }

    #[test]
    fn find_misses_and_empty_catalog_reports_empty() {
        let catalog = catalog_from_manifests(&[]);
        assert!(catalog.is_empty());
        assert_eq!(catalog.action_count(), 0);
        assert!(catalog.find("tumult-network", "add-latency").is_none());
        assert_eq!(catalog.all_actions().count(), 0);
    }

    #[test]
    fn build_catalog_from_default_search_paths_never_fails() {
        // The default-paths entry point (cwd plugins, user-global plugins,
        // TUMULT_PLUGIN_PATH) is fault-tolerant: whatever those paths hold,
        // building the catalog succeeds.
        let catalog = build_catalog().unwrap();
        assert_eq!(catalog.is_empty(), catalog.action_count() == 0);
    }
}
