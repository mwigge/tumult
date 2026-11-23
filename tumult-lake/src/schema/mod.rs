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
//! v22 makes a manual record a DR record (`manual.rs`): planned and actual
//! RTO and RPO, the data verified flag, participants, the plan it fulfils
//! and the version of its seal as columns of `manual_experiments`, added
//! after the v8 rebuild; the steps, actions and links of a record; and,
//! for later changes of v22, the plans, their audit, the sign-offs, the
//! file columns of `evidence_attachments` and the `link_hash` of
//! `manual_experiment_audit` and `manual_plan_audit`.
//! Every statement is `IF NOT EXISTS`, so a database at v21 migrates on
//! open and its records keep their version 1 seal.
//!
//! The DDL is split by feature area: [`telemetry`] (v1), [`manual`] (v2/v8
//! shape), [`analytics`] (v3), [`runs`] (v4–v7), [`auth`] (v6/v9),
//! [`table_stakes`] (v10–v13), [`migrations`] (the versioned rebuilds).

use tumult_core::types::ExperimentStatus;

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

pub(crate) use manual::V22_COLUMNS as MANUAL_V22_COLUMNS;
pub use migrations::{MIGRATE_V5_RUN_TABLES_INDEX_FREE, MIGRATE_V8_MANUAL_EXPERIMENTS_INDEX_FREE};

pub const CURRENT_SCHEMA_VERSION: i64 = 22;

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

/// The views, in the order they are created: the root of every run, the
/// completion log of every run, the rollup of every run and the current
/// state of every entity. CREATE OR REPLACE so existing databases pick up
/// view changes on startup.
#[must_use]
pub fn create_views() -> [String; 4] {
    [
        run_roots_view(),
        run_completions_view(),
        experiment_runs_view(),
        entities::VIEW.to_string(),
    ]
}

/// Additive changes to tables of this version that an earlier build of the
/// same version may have created without them. Run after
/// [`CREATE_TABLES`] and before [`create_views`].
pub const AMEND_TABLES: &[&str] = &[entities::ADD_RECEIPT_COLUMN];

/// The SQL expression for the status a run ended with, in the spelling of
/// the journal: `completed`, `deviated`, `failed`, `aborted`, `interrupted`
/// or `halted`. NULL for a run that has not ended.
///
/// `span` is the alias of the root span of the run and `log` the alias of
/// its row of `run_completions`, the last `experiment.completed` log record
/// of the run. The root span carries the status since 3.0.2. The log is
/// read for runs stored before that, which spelled it with a capital, so
/// the result is in lower case.
#[must_use]
pub fn run_outcome_sql(span: &str, log: &str) -> String {
    format!("lower(coalesce({span}.outcome_status, {log}.log_attrs['status']))")
}

/// The SQL aggregate for the mean time to recover, in seconds: the mean
/// recovery time of the runs that report one. NULL when no run reports
/// one, never 0.
///
/// `root` is the alias of a row of `run_roots`, one row a run. A traced run
/// reports the recovery time on its root span; a run stored only as a
/// journal reports the recovery time its journal measured, which is there
/// only when its definition declares an objective. A span that is no root
/// of a run is not read.
#[must_use]
pub fn mean_recovery_time_sql(root: &str) -> String {
    format!("avg({root}.recovery_time_s)")
}

/// The SQL expression for whether the hypothesis of a run stored as a
/// journal was met, by the rule of `Journal::hypothesis_met`: the verdict
/// after the method when the run got that far, FALSE when the hypothesis
/// failed before the method, and NULL when the run judged none.
///
/// `run` is the alias of a row of `experiments`.
#[must_use]
pub fn hypothesis_met_sql(run: &str) -> String {
    // Laid out as the column of `run_roots` it renders, in lines short
    // enough for the SQL standard of the views.
    format!(
        "coalesce(
        {run}.hypothesis_after_met,
        CASE WHEN NOT {run}.hypothesis_before_met THEN FALSE END
    )"
    )
}

/// What the way a run ended says about the system it tested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunVerdict {
    /// The run completed: the hypothesis held.
    Pass,
    /// The run deviated, failed or was halted.
    Fail,
    /// The run was aborted before its method or interrupted: it proves
    /// nothing either way, so it is neither a pass nor a failure and is
    /// left out of counts and rates.
    NoEvidence,
}

impl RunVerdict {
    /// The verdict as [`run_verdict_sql`] spells it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::NoEvidence => "no_evidence",
        }
    }
}

/// The verdict of each status a run can end with. [`run_verdict`] and
/// [`run_verdict_sql`] both read it.
const RUN_VERDICTS: [(ExperimentStatus, RunVerdict); 6] = [
    (ExperimentStatus::Completed, RunVerdict::Pass),
    (ExperimentStatus::Deviated, RunVerdict::Fail),
    (ExperimentStatus::Failed, RunVerdict::Fail),
    (ExperimentStatus::Halted, RunVerdict::Fail),
    (ExperimentStatus::Aborted, RunVerdict::NoEvidence),
    (ExperimentStatus::Interrupted, RunVerdict::NoEvidence),
];

/// The verdict of a run that ended with `status`, in either spelling.
/// `None` for any other text, as for a run that has not ended: what that
/// counts as is the caller's to say.
#[must_use]
pub fn run_verdict(status: &str) -> Option<RunVerdict> {
    RUN_VERDICTS
        .iter()
        .find(|(ending, _)| ending.to_string().eq_ignore_ascii_case(status))
        .map(|(_, verdict)| *verdict)
}

/// The SQL expression for the [`run_verdict`] of the status `status`, in
/// the spelling of [`RunVerdict::as_str`]. NULL where `run_verdict` gives
/// `None`.
#[must_use]
pub fn run_verdict_sql(status: &str) -> String {
    // Laid out as a column of a SELECT, one arm a line, in lines short
    // enough for the SQL standard of the views.
    let arms: Vec<String> = RUN_VERDICTS
        .iter()
        .map(|(ending, verdict)| format!("\n        WHEN '{ending}' THEN '{}'", verdict.as_str()))
        .collect();
    format!("CASE lower({status}){}\n    END", arms.concat())
}

/// The root of every run, in the columns of `spans`, with `journal_only`.
///
/// A run that sent its trace has its `resilience.experiment` span here as it
/// is. A run stored only as a journal (a plain `tumult run`, a journal
/// import) left no span: its row is built from the journal, says
/// `journal_only`, has no trace id, and takes its target from the run node
/// of the graph, which holds what the definition declared. The hypothesis verdict follows
/// `Journal::hypothesis_met`, by [`hypothesis_met_sql`]. Every column of
/// `spans` is named, so a new one has to be added here.
fn run_roots_view() -> String {
    format!(
        "
CREATE OR REPLACE VIEW run_roots AS
SELECT
    s.ts_ns,
    s.trace_id,
    s.span_id,
    s.parent_span_id,
    s.span_name,
    s.span_kind,
    s.duration_ns,
    s.status_code,
    s.status_message,
    s.service_name,
    s.service_version,
    s.experiment_id,
    s.experiment_name,
    s.outcome_status,
    s.fault_type,
    s.fault_subtype,
    s.fault_severity,
    s.blast_radius,
    s.target_system,
    s.target_technology,
    s.target_environment,
    s.plugin_name,
    s.hypothesis_met,
    s.recovery_time_s,
    s.span_attrs,
    s.resource_attrs,
    s.events,
    FALSE AS journal_only
FROM spans AS s
WHERE s.span_name = 'resilience.experiment'
UNION ALL
SELECT
    e.started_at_ns AS ts_ns,
    NULL AS trace_id,
    NULL AS span_id,
    NULL AS parent_span_id,
    'resilience.experiment' AS span_name,
    'Internal' AS span_kind,
    e.ended_at_ns - e.started_at_ns AS duration_ns,
    CASE WHEN lower(e.status) = 'completed' THEN 'Ok' ELSE 'Error' END AS status_code,
    '' AS status_message,
    'tumult' AS service_name,
    NULL AS service_version,
    e.experiment_id,
    e.title AS experiment_name,
    lower(e.status) AS outcome_status,
    NULL AS fault_type,
    NULL AS fault_subtype,
    NULL AS fault_severity,
    NULL AS blast_radius,
    json_extract_string(g.attrs, '$.target_system') AS target_system,
    json_extract_string(g.attrs, '$.target_technology') AS target_technology,
    json_extract_string(g.attrs, '$.target_environment') AS target_environment,
    NULL AS plugin_name,
    {hypothesis_met} AS hypothesis_met,
    e.rto_measured_s AS recovery_time_s,
    NULL AS span_attrs,
    NULL AS resource_attrs,
    NULL AS events,
    TRUE AS journal_only
FROM experiments AS e
LEFT JOIN graph_nodes AS g ON g.id = 'run:' || e.experiment_id
WHERE NOT EXISTS (
    SELECT 1 FROM spans AS r
    WHERE
        r.span_name = 'resilience.experiment'
        AND r.experiment_id = e.experiment_id
);
",
        hypothesis_met = hypothesis_met_sql("e")
    )
}

/// The completion log of every run: the last `experiment.completed` record
/// stored for its id, so a run that logged its end twice is still one row.
/// A root joins it by `experiment_id` as the `log` of [`run_outcome_sql`].
fn run_completions_view() -> String {
    format!(
        "\nCREATE OR REPLACE VIEW run_completions AS\n{};\n",
        run_completions_sql(None)
    )
}

/// The rows of the view `run_completions`, as a query. With `since_ns` it
/// reads only the records from that time on. A reader that asks about the
/// runs since a time joins this in place of the view: a run completes
/// after it starts, and a filter on the view does not reach below its
/// window, so the view reads every log record there is.
#[must_use]
pub fn run_completions_sql(since_ns: Option<i64>) -> String {
    // A condition of two parts is laid out one part a line, as in the views.
    let filter = since_ns.map_or_else(
        || "WHERE body = 'experiment.completed'".to_string(),
        |ns| format!("WHERE\n    body = 'experiment.completed'\n    AND ts_ns >= {ns}"),
    );
    format!(
        "SELECT
    ts_ns,
    log_attrs,
    log_attrs['experiment_id'] AS experiment_id
FROM logs
{filter}
QUALIFY
    row_number() OVER (
        PARTITION BY log_attrs['experiment_id']
        ORDER BY ts_ns DESC
    ) = 1"
    )
}

/// Rollup view: one row per experiment run, over the roots of the runs.
/// The outcome is read by the rule of [`run_outcome_sql`].
fn experiment_runs_view() -> String {
    format!(
        "
CREATE OR REPLACE VIEW experiment_runs AS
SELECT
    s.experiment_id,
    any_value(s.experiment_name) AS experiment_name,
    min(s.ts_ns) AS started_at_ns,
    max(s.ts_ns + s.duration_ns) AS ended_at_ns,
    max(s.duration_ns) AS duration_ns,
    any_value({outcome}) AS outcome_status,
    any_value(s.hypothesis_met) AS hypothesis_met
FROM run_roots AS s
LEFT JOIN run_completions AS l ON s.experiment_id = l.experiment_id
GROUP BY s.experiment_id;",
        outcome = run_outcome_sql("s", "l")
    )
}
