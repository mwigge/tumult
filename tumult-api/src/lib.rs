// Imported from kronika (Apache-2.0, same author). Pedantic lints are
// scoped to tumult-native crates: this crate predates the pedantic gate and
// carries intentional patterns it flags (timestamp/score casts, f64
// comparisons). CI still applies -D warnings to it.
#![allow(clippy::pedantic)]

//! `tumult-api` — the control-plane API backing the kronika UI: reads
//! (overview, series, experiments, logs, traces, scores, reports) plus the
//! write paths for runs, schedules, webhooks, gamedays, users and tokens.
//!
//! Routes (all under `/api`; reads run against a fresh read-only
//! connection, mutations ride the daemon's single-writer channel):
//!
//! * `GET /api/overview?range=24h|7d|14d|30d` — KPI cards (value, delta vs the
//!   previous equal window, sparkline), runs per day, target-system
//!   leaderboard, fault injection steps.
//! * `GET /api/timeseries?metric=<name>&interval=5m|1h|1d&range=…` — any
//!   semantic metric from the metrics directory as a bucketed series.
//! * `GET /api/experiments?range=&outcome=&target=&fault=&q=` — experiment
//!   list, newest first. The outcome is one of completed, deviated, failed,
//!   aborted, interrupted and halted, read by `tumult_lake::run_outcome_sql`;
//!   `outcome=incomplete` is a run that has not ended.
//! * `GET /api/experiments/{id}` — spans (waterfall), correlated logs,
//!   metric points and the `evidence` of its journal (`null` for a run
//!   without one) for one experiment.
//! * `GET /api/dimensions` — distinct filter values (outcomes, targets,
//!   faults, experiment names).
//! * `GET /api/metrics` — semantic metrics available for `/api/timeseries`.
//! * `GET /api/logs?range=&severity=&service=&q=&limit=` — raw log rows,
//!   newest first (severity is a case-insensitive exact match, `q` a
//!   contains-match on the body).
//! * `GET /api/logs/volume?range=&interval=&severity=&service=&q=` — log
//!   volume bucketed per severity for the explorer's stacked bar.
//! * `GET /api/entities?type=&liveness=&range=&sort=&limit=&cursor=` —
//!   the current row of every entity that entity events reported
//!   (`KRONIKA_ENTITY_EVENTS`), by key and environment, with its
//!   `liveness` (`live`, `expired`, `gone`). `range` keeps the entities
//!   last received inside it; left out, all are listed. One page of at
//!   most 1000 rows (`limit`, default 100) and about 4 MiB, with `total`,
//!   `truncated` and `next_cursor`; a walk with the cursor returns every
//!   entity once. `sort=recent` lists the entity received last first,
//!   without that promise.
//! * `GET /api/entities/{key}/history?environment=&limit=&cursor=` — the
//!   stored versions of one entity, newest first and paged the same way,
//!   each with its environment; `404` for a key that has none in the
//!   caller's environments. The key is percent-encoded as one path
//!   segment.
//! * `DELETE /api/entities?environment=&type=` — remove the entities of
//!   one environment (required; empty for the entities that name none),
//!   of one type or of all, with their history (Admin, confined to the
//!   caller's environment scopes). Runs on the single writer and is
//!   written to the log, as is a request that was refused or failed;
//!   answers `entities` and `versions` removed.
//! * `GET /api/traces?range=&service=&min_duration_ms=&outcome=` — traces
//!   grouped from spans (root name/service, span/error counts, experiment
//!   outcome where the trace is an experiment run).
//! * `GET /api/traces/durations?range=` — root-span durations as scatter
//!   points plus p50/p95/p99 percentiles.
//! * `GET /api/traces/{id}` — every span and log of one trace.
//! * `GET /api/metrics/catalog` — every raw metric name across sums/gauges/
//!   histograms in the caller's scope, with the attribute keys of all its
//!   points.
//! * `GET /api/metrics/query?name=&group_by=&range=&interval=` — bucketed
//!   series for one raw metric (sums → SUM, gauges → AVG, histograms →
//!   avg plus an interpolated p95), optionally split by an attribute key.
//! * `GET /api/topology?range=` — service/target call graph, kept as a
//!   compatibility view of the Star Map: nodes from `service_name` and the
//!   target system of the naming contract (ids as on the Star Map),
//!   edges from parent→child span joins and service→target calls.
//! * `GET /api/starmap?range=&env=&focus=&depth=&kinds=&budget=&include=` —
//!   the Star Map: organisation, ICT entities, systems, components,
//!   services, workloads and the evidence against them as one graph,
//!   assembled per request by `tumult_graph::starmap`.
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
//!   with nothing executed. The plan carries substituted `${secrets.*}`
//!   values, so it is gated at Operator, not Viewer.
//! * `POST /api/runs {registry_id, vars?, env?, target?}` — classify the
//!   definition into a risk tier (T0–T3, ADR-013) at request time: T0
//!   enqueues onto the daemon's bounded run queue (202 + `run_id`, 429 on
//!   overload, never silently queued); T1–T3 park in `pending_approval`
//!   with a canonical pin. `POST /api/runs/{id}/stop` e-stops a run
//!   (mid-method cancel with rollbacks, or cancel-before-start when still
//!   queued).
//! * `GET /api/runs?state=&limit=&gameday_id=&registry_id=` /
//!   `GET /api/runs/{id}` — run list (`gameday_id`: the children of one
//!   campaign; `registry_id`: the runs of one definition, for a GameDay its
//!   campaigns) and one run with its audit trail and approval chain
//!   (request + decisions).
//! * `GET /api/approvals` — the pending approval queue;
//!   `POST /api/runs/{id}/approve` / `POST /api/runs/{id}/reject` record an
//!   approver's decision (Approver role, approver ≠ requester, T3 re-runs
//!   the autopilot gate fail-closed); `POST /api/runs/{id}/break-glass`
//!   (Admin) overrides with a mandatory justification and opens a
//!   retrospective manual-evidence draft as compliance debt.
//! * `GET /api/scores?range=` — Gremlin-style compliance scorecard
//!   (freshness-decayed per-experiment scores, target and portfolio rollup).
//! * `GET /api/gate/{target}?min_score=&max_age_days=&env=` — CI gate: the
//!   target's score as a `pass|warn|fail` verdict with reasons and evidence.
//! * `GET /api/risks?env=` — the passive risk scan: ranked findings
//!   (discipline gaps, stale evidence, breached objectives, untested
//!   dependencies, workload manifests) with the coverage matrix;
//!   `POST /api/risks/{id}/suppress {reason, expires_in_days?}` accepts a
//!   finding for a while (hash-chained, Operator);
//!   `POST /api/risks/{id}/recommend` turns a finding into a registered
//!   experiment (scaffolded from the fault catalog, or the experiment the
//!   finding is about) and an autopilot candidate with a `risk` trigger,
//!   gated and recorded when a policy is loaded (Operator);
//!   `GET /api/posture?env=` — the continuous posture score next to the
//!   risk summary and the top findings.
//! * `GET /api/dr?target=&environment=&from=&to=&limit=&cursor=`: the DR
//!   view. Per target of the DR test register: its state (`ok`, `due`,
//!   `overdue`, `never_tested`), last DR test, next due date and interval,
//!   covering plan, and RTO and RPO. Then the DR tests of the period (by
//!   default the last 365 days), newest first and paged as the entity list
//!   is, and the counts per state, origin and verdict. A run that did not
//!   recover says the least its RTO was. A query field it does not know is
//!   refused.
//! * `GET /api/dora?lei=&profile=&by=entity|org&env=` — DORA coverage per
//!   article and function of one legal entity, each number with its
//!   unknown share, the expectation table (expected against actual
//!   evidence status) and the reporting-readiness facts;
//!   `GET /api/dora/{article}?entity=` lists every evidence row behind
//!   the cells of one article.
//! * `GET /api/maturity?lei=&profile=&by=node|entity|org&env=` — the
//!   internal maturity level (1–5, a house scale) with its pillars, gates
//!   and history, and the per-node roll-up naming the nodes that cap it.
//! * `GET /api/otel/conformance?range=&env=` — per service, which naming
//!   contract attributes every span in the window carried (target columns,
//!   ICT spine), the resulting level (`none|target|spine`) and what is
//!   missing: the first stop when a target is absent from the posture page.
//! * `GET /api/perf/{target}?probe=&quantile=&env=&baseline=&candidate=&range=&include=`
//!   — performance regression verdict
//!   (`improved|unchanged|regressed|inconclusive`) between two versions of
//!   the target inside one environment (by default the two deployed last),
//!   with a bootstrap interval, the evidence quality of each side and the
//!   stored history; a pure read.
//!   `POST /api/perf/{target}/verdicts` computes the same verdict and
//!   appends it to `perf_verdicts` (Operator).
//!   `GET /api/perf?range=` lists the targets with runs in the lookback;
//!   `GET /api/perf/{target}/version?env=&from_ns=&to_ns=` is the version
//!   that served a window.
//! * `GET /api/forecast?metric=&target=&env=&range=&horizon=` — a semantic
//!   metric's series resampled onto a regular step and forecast `horizon`
//!   steps ahead with a 10–90 band; the Timestral sidecar
//!   (`TUMULTD_FORECAST_URL`) when it answers, the built-in Holt-Winters
//!   provider otherwise (`fallback_reason` is a coarse code), with the
//!   model's provenance in the body and the result cached in the lake's
//!   `forecasts` table per series, model, step, horizon and context window.
//!   `degenerate_band` marks a band that had no width and was floored.
//! * `GET /api/runs/{id}/impact` — the run's counterfactual: each probe's
//!   samples against what the metric would have done without the fault
//!   (forecast forward from pre-fault samples, or backcast from the post
//!   phase), the per-sample impact, and the measured recovery time; 404
//!   when the run has no probe samples.
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
pub mod error;
pub use error::ApiError;
pub mod events;
pub mod forecast;
pub mod gamedays;
pub mod handlers;
pub mod import;
pub mod lake;
pub mod manual;
pub mod manual_files;
pub mod manual_plans;
pub mod registry;
pub mod runs;
pub mod schedules;
pub mod score;
pub mod sql_util;
pub mod webhooks;

use std::path::PathBuf;
use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;

use handlers::entities::{entities, entity_history, purge_entities};
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
    /// Kubernetes manifests the passive risk scanner reads (`KRONIKA_MANIFESTS_DIR`).
    manifests_dir: Option<Arc<PathBuf>>,
    /// House parameters, register state, workloads and response cache of
    /// the DORA coverage and maturity scores.
    score: Arc<score::ScoreRuntime>,
    /// The Timestral forecasting sidecar (`TUMULTD_FORECAST_URL`); `None`
    /// means the built-in Holt-Winters provider.
    forecast_client: Option<Arc<forecast::client::TimestralClient>>,
    /// Skips a failing sidecar for a cool-down (`TUMULTD_FORECAST_COOLDOWN_S`).
    forecast_breaker: Arc<forecast::breaker::Breaker>,
    /// One permit per call to the sidecar that may be under way.
    forecast_slots: Arc<tokio::sync::Semaphore>,
    /// Star Map responses of the last 30 s, by parameters and scopes.
    starmap_cache: Arc<handlers::starmap_cache::StarMapCache>,
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
            manifests_dir: None,
            // No org file, default parameters, no response cache: the
            // daemon passes its own runtime.
            score: Arc::new(score::ScoreRuntime::default().with_ttl(std::time::Duration::ZERO)),
            forecast_client: None,
            forecast_breaker: Arc::default(),
            forecast_slots: Arc::new(tokio::sync::Semaphore::new(forecast::SIDECAR_SLOTS)),
            starmap_cache: Arc::default(),
        }
    }

    /// Score with this runtime (org file, manifests, response cache).
    #[must_use]
    pub fn with_score_runtime(mut self, runtime: Arc<score::ScoreRuntime>) -> Self {
        self.score = runtime;
        self
    }

    /// The shared state of the DORA score endpoints.
    #[must_use]
    pub fn score_runtime(&self) -> &Arc<score::ScoreRuntime> {
        &self.score
    }

    /// How long a failing forecasting sidecar is skipped before it is
    /// tried again; zero disables the circuit breaker.
    #[must_use]
    pub fn with_forecast_cooldown(mut self, cooldown: std::time::Duration) -> Self {
        self.forecast_breaker = Arc::new(forecast::breaker::Breaker::new(cooldown));
        self
    }

    /// Replace the forecasting sidecar's circuit breaker.
    #[cfg(test)]
    pub(crate) fn with_forecast_breaker(mut self, breaker: forecast::breaker::Breaker) -> Self {
        self.forecast_breaker = Arc::new(breaker);
        self
    }

    /// The forecasting sidecar's circuit breaker.
    #[must_use]
    pub fn forecast_breaker(&self) -> &forecast::breaker::Breaker {
        &self.forecast_breaker
    }

    /// Point the forecasting endpoints at a `tumult-forecastd` sidecar;
    /// `None` or a blank URL means the built-in provider. The HTTP client
    /// is built here, once, and shared by every request.
    ///
    /// # Errors
    /// A message naming the problem when the URL is not `http(s)://host…`
    /// or the HTTP client cannot be built.
    pub fn with_forecast_url(mut self, url: Option<String>) -> Result<Self, String> {
        let Some(url) = url.filter(|u| !u.trim().is_empty()) else {
            self.forecast_client = None;
            return Ok(self);
        };
        let client = forecast::client::TimestralClient::new(&url)?;
        self.forecast_client = Some(Arc::new(client));
        Ok(self)
    }

    /// The forecasting sidecar's base URL, when configured.
    #[must_use]
    pub fn forecast_url(&self) -> Option<&str> {
        self.forecast_client.as_deref().map(|c| c.base())
    }

    /// The slots for calls to the forecasting sidecar.
    pub(crate) fn forecast_slots(&self) -> &tokio::sync::Semaphore {
        &self.forecast_slots
    }

    /// The forecasting sidecar's client, when configured.
    pub(crate) fn forecast_client(&self) -> Option<&forecast::client::TimestralClient> {
        self.forecast_client.as_deref()
    }

    /// Configure forecasting from the environment: the sidecar from
    /// `TUMULTD_FORECAST_URL` and the circuit breaker's cool-down from
    /// `TUMULTD_FORECAST_COOLDOWN_S` (an invalid cool-down logs a warning
    /// and keeps the default).
    ///
    /// # Errors
    /// A message naming `TUMULTD_FORECAST_URL` when it is set to something
    /// other than an `http(s)` URL, or the HTTP client cannot be built: a
    /// daemon told to use a sidecar it can never reach should not start.
    pub fn forecast_from_env(self) -> Result<Self, String> {
        let url = std::env::var("TUMULTD_FORECAST_URL").ok();
        let cooldown_raw = std::env::var("TUMULTD_FORECAST_COOLDOWN_S").ok();
        let cooldown =
            forecast::breaker::parse_cooldown(cooldown_raw.as_deref()).unwrap_or_else(|e| {
                tracing::warn!(
                    error.type = "invalid_config",
                    error.message = %e,
                    "invalid TUMULTD_FORECAST_COOLDOWN_S; using the default"
                );
                forecast::breaker::DEFAULT_COOLDOWN
            });
        self.with_forecast_cooldown(cooldown)
            .with_forecast_url(url)
            .map_err(|e| format!("TUMULTD_FORECAST_URL: {e}"))
    }

    /// Read the Star Map cache's time from `clock` instead of the
    /// system's. For tests, which move the clock rather than sleep.
    #[doc(hidden)]
    #[must_use]
    pub fn with_starmap_clock(
        mut self,
        clock: Arc<dyn Fn() -> std::time::Instant + Send + Sync>,
    ) -> Self {
        let cache = handlers::starmap_cache::StarMapCache::with_clock(clock);
        self.starmap_cache = Arc::new(cache);
        self
    }

    /// Point the passive risk scanner at a directory of Kubernetes manifests.
    #[must_use]
    pub fn with_manifests_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.manifests_dir = dir.map(Arc::new);
        self
    }

    /// The manifests directory the risk scanner reads, when configured.
    #[must_use]
    pub fn manifests_dir(&self) -> Option<&PathBuf> {
        self.manifests_dir.as_deref()
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
        let org_path = org_file_path(&db_path);
        let score_org_file = org_path.clone();
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
        let manifests_dir = std::env::var_os("KRONIKA_MANIFESTS_DIR").map(PathBuf::from);
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
        .with_manifests_dir(manifests_dir.clone())
        .with_score_runtime(Arc::new(
            score::ScoreRuntime::open(
                score_org_file,
                manifests_dir,
                score::cache_ttl_from_env(),
                sql_util::now_ns(),
            )
            .with_proof_days(score::proof_days_from_env()),
        ))
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

/// Where the org file (org tree, function register, score parameters)
/// is read from: `KRONIKA_ORG_FILE`, else `<db dir>/org.yaml`.
#[must_use]
pub fn org_file_path(db_path: &std::path::Path) -> Option<PathBuf> {
    std::env::var_os("KRONIKA_ORG_FILE")
        .map(PathBuf::from)
        .or_else(|| db_path.parent().map(|d| d.join("org.yaml")))
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
        .route("/api/entities", get(entities).delete(purge_entities))
        .route("/api/entities/{key}/history", get(entity_history))
        .route("/api/traces", get(traces))
        .route("/api/traces/durations", get(trace_durations))
        .route("/api/traces/{id}", get(trace_detail))
        .route("/api/metrics/catalog", get(metrics_catalog))
        .route("/api/metrics/query", get(metrics_query))
        .route("/api/topology", get(topology))
        .route("/api/starmap", get(handlers::starmap::starmap))
        .route("/api/ask", post(ask::ask))
        .route("/api/scores/tree", get(scores_tree))
        .route("/api/gate/{target}", get(handlers::gate::gate))
        .route("/api/risks", get(handlers::risks::risks))
        .route("/api/risks/{id}/suppress", post(handlers::risks::suppress))
        .route(
            "/api/risks/{id}/recommend",
            post(handlers::recommend::recommend),
        )
        .route("/api/posture", get(handlers::risks::posture))
        .route("/api/dr", get(handlers::dr::view))
        .route("/api/forecast", get(forecast::forecast))
        .route("/api/runs/{id}/impact", get(forecast::run_impact))
        .route(
            "/api/otel/conformance",
            get(handlers::conformance::conformance),
        )
        .route("/api/perf", get(handlers::perf::targets))
        .route("/api/dora", get(handlers::dora::dora))
        .route("/api/dora/{article}", get(handlers::dora::article))
        .route("/api/maturity", get(handlers::dora::maturity))
        .route("/api/perf/{target}", get(handlers::perf::perf))
        .route("/api/perf/{target}/version", get(handlers::perf::version))
        .route("/api/perf/{target}/verdicts", post(handlers::perf::store))
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
            "/api/manual/experiments/{id}/audit/verify",
            get(manual::audit_verify),
        )
        .route(
            "/api/manual/experiments/{id}/attachments",
            post(manual::attach),
        )
        .route(
            "/api/manual/experiments/{id}/attachments/{attachment_id}/file",
            get(manual_files::download),
        )
        .route("/api/manual/import", post(manual::import))
        .route(
            "/api/manual/plans",
            get(manual_plans::list).post(manual_plans::create),
        )
        .route(
            "/api/manual/plans/{id}",
            get(manual_plans::detail).patch(manual_plans::edit),
        )
        .route("/api/manual/plans/{id}/cancel", post(manual_plans::cancel))
        .route(
            "/api/manual/plans/{id}/audit/verify",
            get(manual_plans::audit_verify),
        )
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
        .route("/api/gamedays", get(gamedays::list))
        .route("/api/gamedays/validate", post(gamedays::validate))
        .route("/api/gamedays/{id}", get(gamedays::detail))
        .route("/api/gamedays/{id}/runs", post(gamedays::start_campaign))
        .route("/api/webhooks", get(webhooks::list).post(webhooks::create))
        .route("/api/webhooks/{id}/enable", post(webhooks::set_enabled))
        .route("/api/webhooks/{id}/delete", post(webhooks::delete))
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
