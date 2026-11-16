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
//! v14 adds private run_recovery_plans containing immutable input references
//! and configuration hashes; no resolved credentials are persisted.
//!
//! v15 keeps private execution fingerprints out of public audit chains.
//!
//! v16 adds the objectives family (`objectives.rs`): `experiments.test_kind`
//! plus RTO/RPO target and measured columns (additive ALTERs), the raw
//! `probe_samples` time series, and `nfr_verdicts` (one row per objective
//! evaluated against a run).
//!
//! v17 adds `risk_suppressions` (`risk.rs`): the hash-chained record of
//! passive-scan findings an operator accepted, with an optional expiry.
//! v18 adds `perf_verdicts` (`perf.rs`): one row per performance
//! regression verdict between two `service.version`s of a target —
//! additive and index-free apart from the target lookup index.
//!
//! v19 adds `forecasts` (`forecast.rs`): the cache of quantile forecasts
//! with model provenance (model, version, weights hash, context window),
//! indexed by series key for the cache lookup and pruned after 30 days.
//!
//! v20 adds the function register (`ict.rs`): `ict_legal_entities`,
//! `ict_functions`, `ict_function_services` and the daily
//! `requirement_status_daily` and `maturity_daily` snapshots — additive
//! and index-free. Every statement is `IF NOT EXISTS`, so a database at
//! v18 or at v19 migrates to v20 the same way.
//!
//! v21 adds `logs.event_name` and `logs.scope_attrs` (additive ALTERs like
//! v9): the OTLP `EventName` and the instrumentation-scope attributes of a
//! log record, NULL on rows stored before v21. It also adds
//! `entity_versions` (`entities.rs`): one row per distinct state of an
//! entity reported through entity events, index-free under the v5 rule
//! because heartbeats and deletes UPDATE it, with the `entities_current`
//! view over the latest row of each entity. Every statement is
//! `IF NOT EXISTS` or `CREATE OR REPLACE`, so a database at v20 migrates on
//! open.
//!
//! The DDL is split by feature area: [`telemetry`] (v1), [`manual`] (v2/v8
//! shape), [`analytics`] (v3), [`runs`] (v4–v7), [`auth`] (v6/v9),
//! [`table_stakes`] (v10–v13), [`migrations`] (the versioned rebuilds).

mod analytics;
mod auth;
mod entities;
mod forecast;
mod ict;
mod manual;
mod migrations;
mod objectives;
mod perf;
mod risk;
mod runs;
mod table_stakes;
mod telemetry;

pub use migrations::{MIGRATE_V5_RUN_TABLES_INDEX_FREE, MIGRATE_V8_MANUAL_EXPERIMENTS_INDEX_FREE};

pub const CURRENT_SCHEMA_VERSION: i64 = 21;

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
    objectives::DDL,
    risk::DDL,
    perf::DDL,
    forecast::DDL,
    ict::DDL,
    entities::DDL,
];

/// Rollup view: one row per experiment run, over the experiment root spans
/// tumult emits as `resilience.experiment`. The outcome is read by the rule
/// of [`run_outcome_sql`].
/// CREATE OR REPLACE so existing databases pick up view changes on startup.
pub const CREATE_VIEWS: &[&str] = &[EXPERIMENT_RUNS_VIEW, entities::VIEW];

/// Additive changes to tables of this version that an earlier build of the
/// same version may have created without them. Run after
/// [`CREATE_TABLES`] and before [`CREATE_VIEWS`].
pub const AMEND_TABLES: &[&str] = &[entities::ADD_RECEIPT_COLUMN];

/// The SQL expression for the status a run ended with, in the spelling of
/// the journal: `completed`, `deviated`, `failed`, `aborted`, `interrupted`
/// or `halted`. NULL for a run that has not ended.
///
/// `span` is the alias of the root span of the run and `log` the alias of
/// its `experiment.completed` log record. The root span carries the status
/// since 3.0.2. The log is read for runs stored before that, which spelled
/// it with a capital, so the result is in lower case.
#[must_use]
pub fn run_outcome_sql(span: &str, log: &str) -> String {
    format!("lower(coalesce({span}.outcome_status, {log}.log_attrs['status']))")
}

const EXPERIMENT_RUNS_VIEW: &str = "
CREATE OR REPLACE VIEW experiment_runs AS
SELECT
    s.experiment_id,
    any_value(s.experiment_name) AS experiment_name,
    min(s.ts_ns) AS started_at_ns,
    max(s.ts_ns + s.duration_ns) AS ended_at_ns,
    max(s.duration_ns) AS duration_ns,
    any_value(lower(coalesce(s.outcome_status, l.log_attrs['status']))) AS outcome_status,
    any_value(s.hypothesis_met) AS hypothesis_met
FROM spans AS s
LEFT JOIN logs AS l
    ON
        l.log_attrs['experiment_id'] = s.experiment_id
        AND l.body = 'experiment.completed'
WHERE s.span_name = 'resilience.experiment'
GROUP BY s.experiment_id;
";
