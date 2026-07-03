//! CLI command implementations.
//!
//! Each command handler takes parsed CLI arguments and orchestrates the
//! appropriate tumult-core operations.

mod exec;
mod gameday;
mod load;
mod report;
mod run;
mod store;

// Command handlers split out of this module. `mod.rs` cannot use bare directory
// resolution for a same-named subdirectory, so the extracted submodules live in
// `commands/mod/` and are wired up here with explicit `#[path]` attributes.
#[path = "mod/types.rs"]
mod types;
#[path = "mod/agentic.rs"]
mod agentic;
#[path = "mod/validate.rs"]
mod validate;
#[path = "mod/analyze.rs"]
mod analyze;
#[path = "mod/trend.rs"]
mod trend;
#[path = "mod/compliance.rs"]
mod compliance;
#[path = "mod/init.rs"]
mod init;

pub use exec::ProviderExecutor;
pub use gameday::{cmd_gameday_analyze, cmd_gameday_create, cmd_gameday_run};
pub use report::cmd_report;
pub use run::cmd_run;
pub use store::{
    cmd_import, cmd_store_backup, cmd_store_migrate, cmd_store_path, cmd_store_purge,
    cmd_store_stats,
};

// Re-export the extracted command surface so the crate's public API and every
// existing call site (including sibling submodules that reference `super::…`)
// resolve exactly as before.
pub use agentic::{
    cmd_agentic_list_scenario_packs, cmd_agentic_proxy, cmd_agentic_replay,
    cmd_agentic_run_live, cmd_agentic_run_scenario, cmd_agentic_smoke,
};
pub use analyze::cmd_analyze;
pub use compliance::cmd_compliance;
pub use init::cmd_init;
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
mod tests;
#[cfg(test)]
pub(crate) use load::*;
#[cfg(test)]
pub(crate) use report::{generate_html_report, generate_junit_report};
#[cfg(test)]
pub(crate) use compliance::compliance_verdict;
#[cfg(test)]
pub(crate) use init::{generate_template, init_at};
// Names the test modules pull in via `use super::super::*`, previously provided
// by this module's private `use` aliases before the split.
#[cfg(test)]
pub(crate) use std::path::Path;
#[cfg(test)]
pub(crate) use tumult_core::types::{Experiment, Provider};
