//! Schema v3 DDL.
//!
//! Wide, ClickHouse-exporter-aligned tables plus `MAP(VARCHAR, VARCHAR)`
//! attribute maps for the dynamic tail (e.g. `resilience.baseline.probe.{name}.*`).
//! Low-cardinality, high-selectivity `resilience.*` keys are materialized as
//! columns by the ingest layer; everything else stays in the maps.
//!
//! v2 adds the manual-evidence tables (`manual_experiments`,
//! `manual_experiment_audit`, `evidence_attachments`) — see `manual.rs`.
//!
//! v3 unifies the tumult-analytics schema family into the same database
//! under unchanged table names: `experiments` / `activity_results` /
//! `load_results` (journal detail), the four `agentic_*` tables, the three
//! `autopilot_*` tables, and the ChaosGraph `graph_nodes` / `graph_edges`
//! tables (DDL owned by `tumult_graph::sql`, executed at migrate time).
//! One database file, one writer, one `schema_meta`.
//!
//! v4 adds the daemon-run tables (`run_registry`, `runs`, `run_audit`) —
//! see `runs.rs`.
//!
//! v5 rebuilds the v4 run tables without primary keys or secondary indexes:
//! a daemon killed mid-write can return with DuckDB's ART indexes desynced
//! from the table after WAL replay, and every UPDATE then fails fatally
//! ("Failed to delete all rows from index"), poisoning the store exactly
//! when orphan reconciliation must write. Run tables are tiny; scans are
//! free and uniqueness is enforced in code.
//!
//! v6 adds the auth tables (`users`, `sessions`, `tokens`,
//! `user_env_scopes`) — additive and index-free under the same rule as the
//! v5 run tables — plus the `run_audit.actor` column carrying the session
//! identity on run audit events (NULL for system events).
//!
//! v7 adds the approval tables (`approval_requests`, `approval_decisions`)
//! — same additive index-free rule — plus the `run_audit` hash chain
//! (`prev_hash` / `new_hash`), making the run trail tamper-evident like
//! `manual_experiment_audit`. See `approvals.rs`.
//!
//! v8 rebuilds `manual_experiments` without a primary key or secondary
//! indexes — the same v5 crash-robustness rule, closing a gap: the table
//! receives UPDATEs throughout its draft → submitted → verified lifecycle,
//! so a daemon killed mid-write could return with ART indexes desynced and
//! every subsequent lifecycle UPDATE would fail fatally. The audit and
//! attachment tables keep theirs: they are INSERT-only, so desynced indexes
//! can never break a write. Uniqueness of `id` is enforced by uuid
//! generation; lookups at this scale are free scans.
//!
//! v9 adds the optional `tokens.expires_at_ns` column (NULL = never
//! expires, so pre-v9 tokens keep working) — additive and idempotent like
//! the v6/v7 ALTERs below.
//!
//! v10 adds the `run_schedules` table (interval-based recurring runs) —
//! additive and index-free under the same rule as the v5 run tables.
//!
//! v11 adds the `webhooks` and `webhook_cursors` tables (admin-managed
//! outbound event notifications) — additive and index-free likewise.
//!
//! v12 adds GameDay campaign columns: `run_registry.kind` (`'gameday'`;
//! NULL = experiment) and `runs.gameday_id` (a campaign child's parent
//! run; NULL = standalone) — additive ALTERs like v9.
//!
//! v13 adds `webhook_dead_letters`: audit events whose webhook delivery
//! failed permanently (bounded retries exhausted) — the dispatcher never
//! advances a cursor past a failed event without recording it here.
//! Additive and index-free under the same rule as the v11 webhook tables.
//!
//! The DDL is split by feature area: [`telemetry`] (v1), [`manual`] (v2/v8
//! shape), [`analytics`] (v3), [`runs`] (v4–v7), [`auth`] (v6/v9),
//! [`table_stakes`] (v10–v13), [`migrations`] (the versioned rebuilds).

mod analytics;
mod auth;
mod manual;
mod migrations;
mod runs;
mod table_stakes;
mod telemetry;

pub use migrations::{MIGRATE_V5_RUN_TABLES_INDEX_FREE, MIGRATE_V8_MANUAL_EXPERIMENTS_INDEX_FREE};

pub const CURRENT_SCHEMA_VERSION: i64 = 13;

/// The full DDL, one batch per feature area, executed in order. All DDL is
/// `IF NOT EXISTS`, so this doubles as the idempotent v0 → v1 migration on
/// every open.
pub const CREATE_TABLES: &[&str] = &[
    telemetry::DDL,
    manual::DDL,
    analytics::DDL,
    runs::DDL,
    auth::DDL,
    table_stakes::DDL,
];

/// Rollup view: one row per experiment run, over the experiment root spans
/// tumult emits as `resilience.experiment`. The outcome lives on tumult's
/// `experiment.completed` log record (capitalised `status` attr), not on the
/// span, so it is resolved via the same join the API list query uses.
/// CREATE OR REPLACE so existing databases pick up view changes on startup.
pub const CREATE_VIEWS: &str = "
CREATE OR REPLACE VIEW experiment_runs AS
SELECT
    s.experiment_id,
    any_value(s.experiment_name) AS experiment_name,
    min(s.ts_ns) AS started_at_ns,
    max(s.ts_ns + s.duration_ns) AS ended_at_ns,
    max(s.duration_ns) AS duration_ns,
    any_value(coalesce(s.outcome_status, l.log_attrs['status'])) AS outcome_status,
    any_value(s.hypothesis_met) AS hypothesis_met
FROM spans s
LEFT JOIN logs l
    ON l.log_attrs['experiment_id'] = s.experiment_id
    AND l.body = 'experiment.completed'
WHERE s.span_name = 'resilience.experiment'
GROUP BY s.experiment_id;
";
