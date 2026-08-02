// Imported from kronika (Apache-2.0, same author). Pedantic lints are
// scoped to tumult-native crates: this crate predates the pedantic gate and
// carries intentional patterns it flags (timestamp/score casts, f64
// comparisons). CI still applies -D warnings to it.
#![allow(clippy::pedantic)]

//! `tumult-api` — the read-only JSON query API backing the kronika UI.
//!
//! Routes (all under `/api`, all read-only against the store):
//!
//! * `GET /api/overview?range=24h|7d|14d` — KPI cards (value, delta vs the
//!   previous equal window, sparkline), experiments per day, target-system
//!   leaderboard, fault breakdown.
//! * `GET /api/timeseries?metric=<name>&interval=5m|1h|1d&range=…` — any
//!   semantic metric from the metrics directory as a bucketed series.
//! * `GET /api/experiments?range=&outcome=&target=&fault=&q=` — experiment
//!   list, newest first (outcome joined from tumult's `experiment.completed`
//!   log attributes; root spans carry no outcome for real tumult data).
//! * `GET /api/experiments/{id}` — spans (waterfall), correlated logs and
//!   metric points for one experiment.
//! * `GET /api/dimensions` — distinct filter values (outcomes, targets,
//!   faults, experiment names).
//! * `GET /api/metrics` — semantic metrics available for `/api/timeseries`.
//! * `GET /api/logs?range=&severity=&service=&q=&limit=` — raw log rows,
//!   newest first (severity is a case-insensitive exact match, `q` a
//!   contains-match on the body).
//! * `GET /api/logs/volume?range=&interval=&severity=&service=&q=` — log
//!   volume bucketed per severity for the explorer's stacked bar.
//! * `GET /api/traces?range=&service=&min_duration_ms=&outcome=` — traces
//!   grouped from spans (root name/service, span/error counts, experiment
//!   outcome where the trace is an experiment run).
//! * `GET /api/traces/durations?range=` — root-span durations as scatter
//!   points plus p50/p95/p99 percentiles.
//! * `GET /api/traces/{id}` — every span and log of one trace.
//! * `GET /api/metrics/catalog` — raw metric names across sums/gauges/
//!   histograms, with the attribute keys seen on their points.
//! * `GET /api/metrics/query?name=&group_by=&range=&interval=` — bucketed
//!   series for one raw metric (sums → SUM, gauges → AVG, histograms →
//!   avg plus an interpolated p95), optionally split by an attribute key.
//! * `GET /api/topology?range=` — service/target call graph: nodes from
//!   `service_name` and tumult's `resilience.target.name` span attribute,
//!   edges from parent→child span joins and service→target calls.
//! * `POST /api/ask` — natural-language → SQL → rows, guarded by
//!   `tumult_intelligence::sql_guard`; degrades to `{configured:false}` when no LLM
//!   is reachable.
//! * `GET /api/reports` / `GET /api/reports/{name}` — HTML digests written
//!   by the daemon's report scheduler; `POST /api/reports/generate` renders
//!   one metric digest on demand into the same directory. A scoped
//!   principal's digest is confined to its environments; a `<name>.meta.json`
//!   sidecar records that coverage, and global/legacy digests fail closed
//!   for scoped principals (hidden, 404).
//! * `POST /api/import/journal {journal, experiment?}` — daemon-first
//!   journal ingest for the CLI (`TUMULT_DAEMON_URL`): rides the
//!   single-writer channel into the analytics tables, idempotent on
//!   `experiment_id`.
//! * `POST /api/runs/validate {toon, vars?}` — the CLI's full
//!   parse/resolve/validate pipeline as a service; registers the definition
//!   (content-hash dedup) and returns its `registry_id`.
//! * `POST /api/runs/dry-run {registry_id, vars?}` — the resolved execution
//!   plan (hypothesis probes, method steps in order, guards, rollbacks)
//!   with nothing executed.
//! * `POST /api/runs {registry_id, vars?, env?, target?}` — classify the
//!   definition into a risk tier (T0–T3, ADR-013) at request time: T0
//!   enqueues onto the daemon's bounded run queue (202 + `run_id`, 429 on
//!   overload, never silently queued); T1–T3 park in `pending_approval`
//!   with a canonical pin. `POST /api/runs/{id}/stop` e-stops a run
//!   (mid-method cancel with rollbacks, or cancel-before-start when still
//!   queued).
//! * `GET /api/runs?state=&limit=` / `GET /api/runs/{id}` — run list and
//!   one run with its audit trail and approval chain (request + decisions).
//! * `GET /api/approvals` — the pending approval queue;
//!   `POST /api/runs/{id}/approve` / `POST /api/runs/{id}/reject` record an
//!   approver's decision (Approver role, approver ≠ requester, T3 re-runs
//!   the autopilot gate fail-closed); `POST /api/runs/{id}/break-glass`
//!   (Admin) overrides with a mandatory justification and opens a
//!   retrospective manual-evidence draft as compliance debt.
//! * `GET /api/scores?range=` — Gremlin-style resilience scorecard
//!   (freshness-decayed per-experiment scores, target and portfolio rollup).
//! * `GET /api/authoring/catalog` — the live fault catalog (domains →
//!   actions → documented args) from plugin discovery;
//!   `POST /api/authoring/scaffold {plugin?, action, args, target, …}` —
//!   generate experiment TOON from a catalog action plus whether it
//!   validates. The same code paths as the MCP authoring tools; both are
//!   Viewer-level and persist nothing (registration stays behind
//!   `POST /api/runs/validate`).
//! * `POST /api/reports/v2/generate {type,period?,experiment_id?,framework?}`
//!   — build a compliance-grade report (R1 executive digest, R3 game-day,
//!   R2 evidence pack) as PDF + print-HTML + JSON meta under
//!   `reports/v2/`; `GET /api/reports/v2` lists metas and
//!   `GET /api/reports/v2/{id}/pdf|html` serves the artifacts. A scoped
//!   principal's build is confined to its environments; the meta records
//!   that coverage (`env_scopes`), and list/pdf/html fail closed on
//!   global or legacy artifacts for scoped principals.
//!
//! * `POST /api/auth/login` / `POST /api/auth/logout` /
//!   `POST /api/auth/change-password` / `GET /api/me` — session auth. Once
//!   the store has any real user (the v6 `legacy` backfill identity does not
//!   count), every route requires a session cookie or a `kro_` bearer
//!   token; `tumult_api::auth::ROUTE_TABLE` maps
//!   `(method, path)` to a minimum RBAC role, and per-user environment
//!   scopes filter the experiment, run and telemetry reads (logs, traces,
//!   metrics, scores) — a scoped principal sees only its own environments.
//!   `GET|POST /api/users*` and `POST /api/tokens*` are the admin endpoints.
//!
//! Every query runs on a fresh read-only connection inside `spawn_blocking`,
//! so the API coexists with the daemon's single writer and never touches the
//! write lock.

pub mod approvals;
mod ask;
pub mod auth;
pub mod authoring;
pub mod events;
pub mod handlers;
pub mod import;
pub mod lake;
pub mod manual;
pub mod runs;
pub mod schedules;
pub mod sql_util;

use std::path::PathBuf;
use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;

use handlers::experiments::{dimensions, experiment_detail, experiment_windows, experiments};
use handlers::logs::{logs, logs_volume};
use handlers::metrics::{list_metrics, metrics_catalog, metrics_query, timeseries};
use handlers::overview::overview;
use handlers::reports::{
    generate_report, generate_report_v2, get_report, get_report_v2_html, get_report_v2_pdf,
    list_reports, list_reports_v2, scores, scores_tree,
};
use handlers::topology::topology;
use handlers::traces::{trace_detail, trace_durations, traces};

/// Shared handler state: where the store, metric definitions and rendered
/// reports live, plus the LLM client for `/api/ask`, the org tree for
/// `/api/scores/tree` and R1's "By domain", the ingest handle that
/// carries manual-evidence mutations onto the daemon's single writer, and
/// the bounded run queue behind `/api/runs*`.
#[derive(Clone)]
pub struct ApiState {
    db_path: Arc<PathBuf>,
    metrics_dir: Arc<PathBuf>,
    reports_dir: Arc<PathBuf>,
    llm: Arc<dyn tumult_intelligence::llm::Llm>,
    org: Arc<tumult_compliance::OrgTree>,
    ingest: Option<tumult_ingest::IngestWriter>,
    runs: Option<tumult_ingest::RunQueue>,
    /// The autopilot policy gating T3 approvals (ADR-013); `None` fails the
    /// gate closed (T3 approvals are refused 422).
    autopilot_policy: Option<Arc<tumult_autopilot::LoadedPolicy>>,
    /// Whether the API is served over TLS: session cookies get `; Secure`.
    secure_cookies: bool,
}

impl ApiState {
    /// Full constructor (tests inject a stub LLM and a scratch reports dir).
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db_path: PathBuf,
        metrics_dir: PathBuf,
        reports_dir: PathBuf,
        llm: Arc<dyn tumult_intelligence::llm::Llm>,
        org: tumult_compliance::OrgTree,
        ingest: Option<tumult_ingest::IngestWriter>,
        runs: Option<tumult_ingest::RunQueue>,
        autopilot_policy: Option<Arc<tumult_autopilot::LoadedPolicy>>,
        secure_cookies: bool,
    ) -> Self {
        Self {
            db_path: Arc::new(db_path),
            metrics_dir: Arc::new(metrics_dir),
            reports_dir: Arc::new(reports_dir),
            llm,
            org: Arc::new(org),
            ingest,
            runs,
            autopilot_policy,
            secure_cookies,
        }
    }

    /// Daemon constructor: reports live in `<db dir>/reports`, LLM configured
    /// from `KRONIKA_LLM_*` env vars. The org tree loads from
    /// `KRONIKA_ORG_FILE`, defaulting to `<db dir>/org.yaml`; a missing file
    /// means an empty tree (everything rolls up under `(unassigned)`) and an
    /// invalid file logs a warning and falls back to empty. The autopilot
    /// policy gating T3 approvals loads from `KRONIKA_AUTOPILOT_POLICY`
    /// (path to a policy TOML); unset or unreadable/invalid means `None` —
    /// fail closed, T3 approvals are refused.
    #[must_use]
    pub fn from_env_parts(
        db_path: PathBuf,
        metrics_dir: PathBuf,
        ingest: Option<tumult_ingest::IngestWriter>,
        runs: Option<tumult_ingest::RunQueue>,
        secure_cookies: bool,
    ) -> Self {
        let reports_dir = db_path
            .parent()
            .map_or_else(|| PathBuf::from("reports"), |d| d.join("reports"));
        let org_path = std::env::var_os("KRONIKA_ORG_FILE")
            .map(PathBuf::from)
            .or_else(|| db_path.parent().map(|d| d.join("org.yaml")));
        let org = org_path
            .filter(|p| p.exists())
            .map_or_else(tumult_compliance::OrgTree::empty, |p| {
                tumult_compliance::OrgTree::load(&p).unwrap_or_else(|e| {
                    tracing::warn!(path = %p.display(), error = %e, "invalid org file; using empty tree");
                    tumult_compliance::OrgTree::empty()
                })
            });
        let autopilot_policy = std::env::var_os("KRONIKA_AUTOPILOT_POLICY").and_then(|p| {
            let path = PathBuf::from(p);
            let loaded = std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|text| {
                    tumult_autopilot::LoadedPolicy::parse(&text).map_err(|e| e.to_string())
                });
            match loaded {
                Ok(policy) => Some(Arc::new(policy)),
                Err(e) => {
                    tracing::error!(path = %path.display(), error = %e, "autopilot policy failed to load; T3 approvals fail closed");
                    None
                }
            }
        });
        Self::new(
            db_path,
            metrics_dir,
            reports_dir,
            Arc::new(tumult_intelligence::llm::OpenAiCompatClient::from_env()),
            org,
            ingest,
            runs,
            autopilot_policy,
            secure_cookies,
        )
    }

    /// Directory the report scheduler writes into and `/api/reports` reads.
    #[must_use]
    pub fn reports_dir(&self) -> &PathBuf {
        &self.reports_dir
    }

    /// The ingest handle carrying manual-evidence writes (daemon only).
    #[must_use]
    pub fn ingest_handle(&self) -> Option<&tumult_ingest::IngestWriter> {
        self.ingest.as_ref()
    }

    /// The bounded run queue behind `/api/runs*` (daemon only).
    #[must_use]
    pub fn runs_handle(&self) -> Option<&tumult_ingest::RunQueue> {
        self.runs.as_ref()
    }

    /// The autopilot policy gating T3 approvals (`None` fails closed).
    #[must_use]
    pub fn autopilot_policy(&self) -> Option<Arc<tumult_autopilot::LoadedPolicy>> {
        self.autopilot_policy.clone()
    }

    /// Whether session cookies are marked `Secure` (TLS deployments).
    #[must_use]
    pub fn secure_cookies(&self) -> bool {
        self.secure_cookies
    }
}

/// Build the API router. Merge into the daemon's HTTP server.
pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/api/overview", get(overview))
        .route("/api/timeseries", get(timeseries))
        .route("/api/experiments", get(experiments))
        .route("/api/experiments/windows", get(experiment_windows))
        .route("/api/experiments/{id}", get(experiment_detail))
        .route("/api/dimensions", get(dimensions))
        .route("/api/metrics", get(list_metrics))
        .route("/api/logs", get(logs))
        .route("/api/logs/volume", get(logs_volume))
        .route("/api/traces", get(traces))
        .route("/api/traces/durations", get(trace_durations))
        .route("/api/traces/{id}", get(trace_detail))
        .route("/api/metrics/catalog", get(metrics_catalog))
        .route("/api/metrics/query", get(metrics_query))
        .route("/api/topology", get(topology))
        .route("/api/ask", post(ask::ask))
        .route("/api/scores/tree", get(scores_tree))
        .route(
            "/api/manual/experiments",
            get(manual::list).post(manual::create),
        )
        .route(
            "/api/manual/experiments/{id}",
            get(manual::detail).put(manual::update),
        )
        .route("/api/manual/experiments/{id}/submit", post(manual::submit))
        .route("/api/manual/experiments/{id}/verify", post(manual::verify))
        .route("/api/manual/experiments/{id}/reject", post(manual::reject))
        .route(
            "/api/manual/experiments/{id}/attachments",
            post(manual::attach),
        )
        .route("/api/manual/import", post(manual::import))
        .route("/api/import/journal", post(import::import_journal))
        .route("/api/authoring/catalog", get(authoring::catalog))
        .route("/api/authoring/scaffold", post(authoring::scaffold))
        .route("/api/registry", get(runs::registry_list))
        .route("/api/registry/{id}", get(runs::registry_detail))
        .route("/api/runs/validate", post(runs::validate))
        .route("/api/runs/dry-run", post(runs::dry_run))
        .route("/api/runs", get(runs::list).post(runs::create))
        .route("/api/runs/stop-all", post(runs::stop_all))
        .route(
            "/api/schedules",
            get(schedules::list).post(schedules::create),
        )
        .route("/api/schedules/{id}/enable", post(schedules::set_enabled))
        .route("/api/schedules/{id}/delete", post(schedules::delete))
        .route("/api/events", get(events::list))
        .route("/api/runs/{id}", get(runs::detail))
        .route("/api/runs/{id}/audit/verify", get(runs::audit_verify))
        .route("/api/runs/{id}/stop", post(runs::stop))
        .route("/api/approvals", get(approvals::queue))
        .route("/api/runs/{id}/approve", post(approvals::approve))
        .route("/api/runs/{id}/reject", post(approvals::reject))
        .route("/api/runs/{id}/break-glass", post(approvals::break_glass))
        .route("/api/lake/status", get(lake::status))
        .route("/api/lake/export", post(lake::export_now))
        .route("/api/reports", get(list_reports))
        .route("/api/reports/generate", post(generate_report))
        .route("/api/reports/v2", get(list_reports_v2))
        .route("/api/reports/v2/generate", post(generate_report_v2))
        .route("/api/reports/v2/{id}/pdf", get(get_report_v2_pdf))
        .route("/api/reports/v2/{id}/html", get(get_report_v2_html))
        .route("/api/scores", get(scores))
        .route("/api/reports/{name}", get(get_report))
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/change-password", post(auth::change_password))
        .route("/api/me", get(auth::me))
        .route("/api/users", get(auth::list_users).post(auth::create_user))
        .route("/api/users/{id}/role", post(auth::set_role))
        .route("/api/users/{id}/password", post(auth::reset_password))
        .route("/api/users/{id}/disable", post(auth::set_disabled))
        .route("/api/users/{id}/scopes", post(auth::set_scopes))
        .route(
            "/api/tokens",
            get(auth::list_tokens).post(auth::create_token),
        )
        .route("/api/tokens/{id}/revoke", post(auth::revoke_token))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::auth_middleware,
        ))
        .with_state(state)
}
