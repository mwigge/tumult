//! CLI command implementations.
//!
//! Each command handler takes parsed CLI arguments and orchestrates the
//! appropriate tumult-core operations.

mod agentic;
mod analyze;
mod autopilot;
mod chaosgraph;
mod compliance;
mod gameday;
mod init;
mod load;
mod mcp;
mod new;
mod recommend;
mod report;
mod run;
mod store;
mod topology;
mod trend;
mod types;
mod validate;

pub use gameday::{cmd_gameday_analyze, cmd_gameday_create, cmd_gameday_run};
pub use recommend::{cmd_agents, cmd_recommend, AgentArgs};
pub use report::cmd_report;
pub use run::cmd_run;
pub use store::{
    cmd_import, cmd_store_backup, cmd_store_import_legacy, cmd_store_migrate, cmd_store_path,
    cmd_store_purge, cmd_store_stats,
};
pub use tumult_exec::ProviderExecutor;

// Re-export the extracted command surface so the crate's public API and every
// existing call site (including sibling submodules that reference `super::…`)
// resolve exactly as before.
pub use agentic::{
    cmd_agentic_list_scenario_packs, cmd_agentic_proxy, cmd_agentic_replay, cmd_agentic_run_live,
    cmd_agentic_run_scenario, cmd_agentic_smoke, cmd_agentic_trajectory,
};
pub use analyze::cmd_analyze;
pub use autopilot::{
    cmd_autopilot_export, cmd_autopilot_notify_change, cmd_autopilot_once, cmd_autopilot_respond,
    cmd_autopilot_status,
};
pub use chaosgraph::{
    cmd_chaosgraph_coverage_gaps, cmd_chaosgraph_neighbors, cmd_chaosgraph_query,
};
pub use compliance::cmd_compliance;
pub use init::cmd_init;
pub use mcp::{cmd_mcp_serve, Transport as McpTransportKind};
pub use new::{cmd_new, cmd_templates};
pub use topology::{
    cmd_topology_discover_k8s, cmd_topology_import, cmd_topology_lineage, cmd_topology_map,
    cmd_topology_recommend,
};
pub use trend::{cmd_export, cmd_trend};
pub use types::{
    build_load_override, parse_duration_str, parse_var_args, ComplianceFramework, ExportFormat,
    LoadToolArg, ReportFormat,
};
pub use validate::{cmd_discover, cmd_validate};

// Crate-internal helpers referenced by sibling submodules via `super::…`.
pub(crate) use init::print_dry_run;
pub(crate) use validate::validate_path_no_symlink;

#[cfg(test)]
pub(crate) use tumult_exec::native_registry;
#[cfg(test)]
pub(crate) use validate::render_discover;

#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) use compliance::compliance_verdict;
#[cfg(test)]
pub(crate) use init::{generate_template, init_at};
#[cfg(test)]
pub(crate) use load::*;
#[cfg(test)]
pub(crate) use report::{generate_html_report, generate_junit_report};
// Names the test modules pull in via `use super::super::*`, previously provided
// by this module's private `use` aliases before the split.
#[cfg(test)]
pub(crate) use std::path::Path;
#[cfg(test)]
pub(crate) use tumult_core::types::{Experiment, Provider};
