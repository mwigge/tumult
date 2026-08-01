//! `chaosgraph` subcommand arguments.

use std::path::PathBuf;

use super::GraphFormat;

#[derive(clap::Subcommand, Debug)]
pub(crate) enum ChaosGraphAction {
    /// List graph nodes of a given kind (experiment, fault, service, journal, …)
    Query {
        /// Node kind to list
        #[arg(long)]
        kind: String,
        /// Case-insensitive label substring filter
        #[arg(long)]
        filter: Option<String>,
        /// Output format
        #[arg(long, value_enum, default_value_t = GraphFormat::Text)]
        format: GraphFormat,
        /// Analytics store path (default: ~/.tumult/lake.duckdb, override with
        /// `TUMULT_LAKE_PATH`)
        #[arg(long)]
        store: Option<PathBuf>,
    },
    /// Show the ego sub-graph (neighbours) around a node
    Neighbors {
        /// Node id to center on (e.g. `exp:My experiment`)
        #[arg(long)]
        node: String,
        /// Restrict to a single relation (e.g. `injects`, `targets`)
        #[arg(long)]
        rel: Option<String>,
        /// Traversal depth in hops
        #[arg(long, default_value_t = 1)]
        depth: u32,
        /// Output format
        #[arg(long, value_enum, default_value_t = GraphFormat::Text)]
        format: GraphFormat,
        /// Analytics store path (default: ~/.tumult/lake.duckdb, override with
        /// `TUMULT_LAKE_PATH`)
        #[arg(long)]
        store: Option<PathBuf>,
    },
    /// Report chaos actions never exercised by a tested run
    #[command(name = "coverage-gaps")]
    CoverageGaps {
        /// Annotate with this framework's still-unevidenced articles
        #[arg(long)]
        framework: Option<String>,
        /// Filter gaps to a fault domain (plugin name substring)
        #[arg(long)]
        domain: Option<String>,
        /// Also persist the derived gap sub-graph into the store so
        /// `chaosgraph query/neighbors` can navigate it. Takes a write lock, so
        /// it conflicts with a running MCP server on the same store.
        #[arg(long)]
        refresh: bool,
        /// Output format
        #[arg(long, value_enum, default_value_t = GraphFormat::Text)]
        format: GraphFormat,
        /// Analytics store path (default: ~/.tumult/lake.duckdb, override with
        /// `TUMULT_LAKE_PATH`)
        #[arg(long)]
        store: Option<PathBuf>,
    },
}
