//! `autopilot` subcommand arguments.

use std::path::PathBuf;

use super::GraphFormat;

#[derive(clap::Subcommand, Debug)]
pub(crate) enum AutopilotAction {
    /// Run one pass of the decision loop: assemble, gate, and record every
    /// decision. Audit-before-act: decisions are persisted BEFORE anything
    /// runs, and without --execute nothing is injected at all.
    Once {
        /// Path to the autopilot policy TOML (`[autopilot]` table)
        #[arg(long)]
        policy: PathBuf,
        /// Actually run playbook experiments for enact verdicts — real
        /// fault injection. Off by default (decide + record only).
        #[arg(long)]
        execute: bool,
        /// Maximum candidates gated in this pass (default: 3, max 10)
        #[arg(long)]
        limit: Option<u32>,
        /// Analytics store path (default: ~/.tumult/lake.duckdb, override with
        /// `TUMULT_LAKE_PATH`)
        #[arg(long)]
        store: Option<PathBuf>,
    },
    /// List recorded decisions with their latest lifecycle event
    Status {
        /// Filter by verdict: enact, downgrade, propose, or veto
        #[arg(long)]
        verdict: Option<String>,
        /// Maximum number of decisions shown (default: 20)
        #[arg(long)]
        limit: Option<u32>,
        /// Output format
        #[arg(long, value_enum, default_value_t = GraphFormat::Text)]
        format: GraphFormat,
        /// Analytics store path (default: ~/.tumult/lake.duckdb, override with
        /// `TUMULT_LAKE_PATH`)
        #[arg(long)]
        store: Option<PathBuf>,
    },
    /// Approve a proposed decision — runs its playbook experiment after a
    /// full re-gate against current state, which requires the policy the
    /// decision was gated under
    Approve {
        /// Decision id (from `tumult autopilot status`)
        id: String,
        /// Path to the autopilot policy TOML (`[autopilot]` table) — required:
        /// an approval re-gates against current state before the playbook runs
        #[arg(long)]
        policy: PathBuf,
        /// Analytics store path (default: ~/.tumult/lake.duckdb, override with
        /// `TUMULT_LAKE_PATH`)
        #[arg(long)]
        store: Option<PathBuf>,
    },
    /// Deny a proposed decision — records the veto feedback the autonomy
    /// ladder consumes
    Deny {
        /// Decision id (from `tumult autopilot status`)
        id: String,
        /// Reason for the denial, persisted with the response event
        #[arg(long)]
        reason: Option<String>,
        /// Analytics store path (default: ~/.tumult/lake.duckdb, override with
        /// `TUMULT_LAKE_PATH`)
        #[arg(long)]
        store: Option<PathBuf>,
    },
    /// Record a deploy/config change event against a service — the next
    /// autopilot pass treats its evidence as invalidated
    #[command(name = "notify-change")]
    NotifyChange {
        /// Service name (bare or svc: id)
        #[arg(long)]
        service: String,
        /// What reported the change (e.g. deploy-webhook)
        #[arg(long, default_value = "manual")]
        source: String,
        /// Optional detail about what changed
        #[arg(long)]
        detail: Option<String>,
        /// Analytics store path (default: ~/.tumult/lake.duckdb, override with
        /// `TUMULT_LAKE_PATH`)
        #[arg(long)]
        store: Option<PathBuf>,
    },
    /// Export the decision and event tables as Parquet files
    Export {
        /// Output directory for the Parquet files
        dir: PathBuf,
        /// Analytics store path (default: ~/.tumult/lake.duckdb, override with
        /// `TUMULT_LAKE_PATH`)
        #[arg(long)]
        store: Option<PathBuf>,
    },
}
