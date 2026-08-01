//! `topology` subcommand: format enum and action arguments.

use std::path::PathBuf;

/// Rendering for `topology` command output.
///
/// One enum for the whole `topology` family so every view accepts the same
/// format tokens. Only `map` renders Mermaid; `lineage` and `recommend`
/// reject it at dispatch time with a pointer to `map`.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TopologyFormat {
    /// Readable text rendering (default)
    Text,
    /// Mermaid `graph TD` diagram (`topology map` only)
    Mermaid,
    /// The structured result as pretty JSON
    Json,
}

impl TopologyFormat {
    /// The format token the shared tool implementation expects.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Mermaid => "mermaid",
            Self::Json => "json",
        }
    }
}

#[derive(clap::Subcommand, Debug)]
pub(crate) enum TopologyAction {
    /// Import a declared topology TOML (services + `depends_on`) into the store
    Import {
        /// Path to the topology TOML file (e.g. ~/.tumult/topology.toml)
        path: PathBuf,
        /// Analytics store path (default: ~/.tumult/lake.duckdb, override with
        /// `TUMULT_LAKE_PATH`)
        #[arg(long)]
        store: Option<PathBuf>,
    },
    /// Propose a topology TOML from a live cluster's Services, for human
    /// review before `topology import` (never writes the store or graph)
    #[command(name = "discover-k8s")]
    DiscoverK8s {
        /// Namespace to scan; repeatable (default: all namespaces except
        /// kube-system — pass `--namespace kube-system` to include it)
        #[arg(long = "namespace")]
        namespace: Vec<String>,
        /// Write the proposed TOML to this file instead of stdout
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Render the compliance-aware service map (text, Mermaid, or JSON)
    Map {
        /// Scope to one framework (dora, nis2, pci-dss, …)
        #[arg(long)]
        framework: Option<String>,
        /// Scope to one control id (e.g. `Art.25`)
        #[arg(long)]
        control: Option<String>,
        /// Output format
        #[arg(long, value_enum, default_value_t = TopologyFormat::Text)]
        format: TopologyFormat,
        /// Skip injection recommendations
        #[arg(long)]
        no_recommend: bool,
        /// Maximum number of recommendations
        #[arg(long, default_value_t = 3)]
        limit: u32,
        /// Analytics store path (default: ~/.tumult/lake.duckdb, override with
        /// `TUMULT_LAKE_PATH`)
        #[arg(long)]
        store: Option<PathBuf>,
    },
    /// Show the (article × service) compliance lineage matrix
    Lineage {
        /// Scope to one framework (dora, nis2, pci-dss, …)
        #[arg(long)]
        framework: Option<String>,
        /// Scope to one control id (e.g. `Art.25`)
        #[arg(long)]
        control: Option<String>,
        /// Filter to one service (bare name or `svc:` id)
        #[arg(long)]
        service: Option<String>,
        /// Output format (text or json; Mermaid is `topology map` only)
        #[arg(long, value_enum, default_value_t = TopologyFormat::Text)]
        format: TopologyFormat,
        /// Analytics store path (default: ~/.tumult/lake.duckdb, override with
        /// `TUMULT_LAKE_PATH`)
        #[arg(long)]
        store: Option<PathBuf>,
    },
    /// Rank the next most valuable fault injections, with reasons
    Recommend {
        /// Scope to one framework (dora, nis2, pci-dss, …)
        #[arg(long)]
        framework: Option<String>,
        /// Maximum number of recommendations
        #[arg(long, default_value_t = 3)]
        limit: u32,
        /// Output format (text or json; Mermaid is `topology map` only)
        #[arg(long, value_enum, default_value_t = TopologyFormat::Text)]
        format: TopologyFormat,
        /// Analytics store path (default: ~/.tumult/lake.duckdb, override with
        /// `TUMULT_LAKE_PATH`)
        #[arg(long)]
        store: Option<PathBuf>,
    },
}
