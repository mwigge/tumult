// Imported from kronika (Apache-2.0, same author). Pedantic lints are
// scoped to tumult-native crates: this crate predates the pedantic gate and
// carries intentional patterns it flags (timestamp/score casts, f64
// comparisons). CI still applies -D warnings to it.
#![allow(clippy::pedantic)]

//! `tumultd` — the kronika daemon.
//!
//! Default command (`serve`): open the store, spawn the single-writer
//! channel, and run the OTLP/gRPC (`:4317`) and OTLP/HTTP (`:4318`, with
//! `GET /healthz`) ingest servers until SIGTERM/SIGINT. The HTTP server also
//! exposes `GET /report?metric=<name>`, which renders a metric report from
//! the live store — this works while the daemon holds the write lock, unlike
//! the `report` subcommand which needs the daemon stopped.
//!
//! The same HTTP server mounts the read-only query API (`/api/*`, from
//! `tumult-api`) that backs the web UI. When `KRONIKA_REPORT_INTERVAL`
//! (e.g. `1h`, `30m`) is set, a scheduler renders a metric digest per
//! interval into `<db dir>/reports/report_<epoch>.html`; `/api/reports`
//! lists them. Automatic reporting is off by default.
//!
//! The parquet lake job runs on `KRONIKA_LAKE_INTERVAL` (default `24h`,
//! `0`/`off` disables): incremental export of every table into
//! `KRONIKA_LAKE_DIR` (default `<db dir>/lake`), then — only when
//! `KRONIKA_RETENTION_DAYS > 0` — deletion of already-exported hot rows
//! older than that many days (the manual-evidence tables are never
//! deleted). `POST /api/lake/export` triggers the same job on demand.
//!
//! The daemon also executes experiments itself: `/api/runs*` (validate,
//! dry-run, enqueue, e-stop) is backed by a bounded in-process run queue
//! (`TUMULTD_RUN_CONCURRENCY` / `TUMULTD_RUN_QUEUE_DEPTH`). At startup,
//! runs left active by a previous process lifetime are reconciled —
//! marked orphaned, their rollbacks attempted — before the servers accept
//! traffic. A telemetry loopback points the daemon's own OTel exporter at
//! its own gRPC ingest, so daemon-run experiments land in the store (and
//! the UI) exactly like CLI runs.
//!
//! Subcommands:
//! * `tumultd import <file>` — manual CSV / tumult journal JSON import.
//! * `tumultd report --metric <name>` — print an HTML report to stdout.
//! * `tumultd create-admin` — create the first admin user (daemon stopped).

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use clap::{Parser, Subcommand};
use tumult_ingest::{Config, IngestWriter, ManualImporter};
use tumult_lake::{Store, TokenRow, UserRow};

#[derive(Parser)]
#[command(
    name = "tumultd",
    version,
    about = "Krönika — the chronicle of your resilience work"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the ingest daemon (default when no subcommand is given).
    Serve,
    /// Manually import a CSV or tumult journal JSON file into the store.
    Import {
        /// Path to the file to import.
        file: PathBuf,
        /// Optional label recorded on the import batch.
        #[arg(long)]
        label: Option<String>,
    },
    /// Print an HTML report for a semantic metric to stdout (or --out).
    Report {
        /// Metric name from the metrics directory (e.g. hypothesis_pass_rate).
        #[arg(long)]
        metric: String,
        /// Write the report to this file instead of stdout.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Create the first admin user with a generated one-time password.
    ///
    /// Opens the store directly: stop the daemon first (or point
    /// TUMULT_LAKE_PATH / --db at a store the daemon is not holding).
    CreateAdmin {
        /// Username for the new admin.
        #[arg(long, default_value = "admin")]
        username: String,
        /// Store path override (defaults to the TUMULT_LAKE_PATH resolution).
        #[arg(long)]
        db: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "tumultd=info,tumult_ingest=info".into()),
        )
        .init();

    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => serve().await,
        Command::Import { file, label } => import(file, label),
        Command::Report { metric, out } => report(metric, out),
        Command::CreateAdmin { username, db } => create_admin(&username, db),
    }
}

/// Wait for SIGTERM or SIGINT.
async fn shutdown_signal(name: &'static str) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate()).expect("install SIGTERM handler");
        tokio::select! {
            _ = sigterm.recv() => {},
            _ = tokio::signal::ctrl_c() => {},
        }
        tracing::info!(server = name, "shutdown signal received");
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Drop guard that flushes OpenTelemetry spans on every exit path from
/// `serve` — including `?` early returns — so telemetry is not lost exactly
/// on the failed runs where it matters most (mirrors the CLI's guard).
struct TelemetryShutdown(tumult_otel::telemetry::TumultTelemetry);

impl Drop for TelemetryShutdown {
    fn drop(&mut self) {
        self.0.shutdown();
    }
}

async fn serve() -> Result<()> {
    let config = Config::from_env().map_err(anyhow::Error::msg)?;
    tracing::info!(
        db = %config.db_path.display(),
        grpc = %config.otlp_grpc_addr,
        http = %config.otlp_http_addr,
        "starting tumultd"
    );

    // Telemetry loopback: the daemon's own OTel exporter points at its own
    // gRPC ingest, so experiments the daemon executes (run queue, orphan
    // rollbacks) land in the store exactly like CLI runs. An unspecified
    // bind address (0.0.0.0) exports via localhost. When the ingest is
    // token-guarded the loopback must authenticate too — otherwise its spans
    // are rejected (UNAUTHENTICATED) and silently dropped.
    let loopback = if config.otlp_grpc_addr.ip().is_unspecified() {
        format!("http://127.0.0.1:{}", config.otlp_grpc_addr.port())
    } else {
        format!("http://{}", config.otlp_grpc_addr)
    };
    let loopback_headers = config.ingest_token.as_deref().map(|token| {
        let mut map = tonic::metadata::MetadataMap::new();
        map.insert(
            "authorization",
            format!("Bearer {token}")
                .parse()
                .expect("ingest token is valid gRPC metadata"),
        );
        map
    });
    let otel_config = tumult_otel::config::TelemetryConfig {
        enabled: true,
        otlp_endpoint: Some(loopback),
        otlp_headers: loopback_headers,
        ..tumult_otel::config::TelemetryConfig::from_env()
    };
    let _telemetry_guard =
        TelemetryShutdown(tumult_otel::telemetry::TumultTelemetry::new(otel_config));

    let store = Store::open(&config.db_path).context("open store")?;
    let writer = store.writer().context("open store writer")?;

    // Auth bind guard + demo bootstrap: refuses a network-exposed bind with
    // zero users and no bootstrap password; provisions the env bootstrap
    // admin/token on the zero-users path. Runs before any server binds.
    enforce_bind_guard(&writer, &store, &config)?;

    let (ingest, writer_task) = IngestWriter::spawn_reconnect(writer, config.db_path.clone(), 1024);

    // The run queue's executor: the CLI's providers with the run's
    // resolved TUMULT_CONFIG_* / TUMULT_SECRET_* environment injected.
    let run_factory: tumult_ingest::runs::ExecutorFactory = std::sync::Arc::new(|env| {
        std::sync::Arc::new(tumult_exec::ProviderExecutor::with_injected_env(env))
    });

    // Reconcile runs left active by a previous process lifetime (crash,
    // kill -9) before accepting traffic: mark orphaned, attempt rollbacks,
    // record the outcome in each run's audit trail.
    match tumult_ingest::runs::reconcile_orphans(&ingest, &config.db_path, &run_factory).await {
        Ok(0) => {}
        Ok(count) => tracing::warn!(
            count,
            "reconciled orphaned runs from a previous process lifetime"
        ),
        Err(e) => tracing::error!(error = %e, "orphan reconciliation failed"),
    }

    let run_queue = tumult_ingest::RunQueue::spawn(
        ingest.clone(),
        config.db_path.clone(),
        tumult_ingest::RunQueueConfig::from_env(),
        run_factory,
    );

    // OTLP/gRPC server (tumult exporter target).
    let grpc_addr = config.otlp_grpc_addr;
    let grpc_ingest = ingest.clone();
    let grpc_token = config.ingest_token.clone();
    let grpc_server = tokio::spawn(async move {
        let result = tumult_ingest::grpc::router_with_token(grpc_ingest, grpc_token)
            .serve_with_shutdown(grpc_addr, shutdown_signal("grpc"))
            .await;
        tracing::info!("gRPC server future completed: {result:?}");
        result
    });

    // OTLP/HTTP server (smedja exporter target) + health + live reports + the
    // read-only query API backing the web UI.
    let http_addr = config.otlp_http_addr;
    tumult_ingest::http::warn_if_unauthenticated(&http_addr, config.ingest_token.as_deref());
    let http_token = config.ingest_token.clone();
    let report_state = ReportState {
        db_path: config.db_path.clone(),
        metrics_dir: config.metrics_dir.clone(),
    };
    let api_state = tumult_api::ApiState::from_env_parts(
        config.db_path.clone(),
        config.metrics_dir.clone(),
        Some(ingest.clone()),
        Some(run_queue),
        // Secure cookies whenever the API is served beyond loopback.
        !tumult_auth::host_is_loopback(&http_addr.ip().to_string()),
    );
    if let Some(interval) = report_interval_from_env() {
        spawn_report_scheduler(
            config.db_path.clone(),
            config.metrics_dir.clone(),
            api_state.reports_dir().clone(),
            interval,
            std::sync::Arc::new(tumult_intelligence::llm::OpenAiCompatClient::from_env()),
        );
    }
    if let Some(interval) = lake_interval_from_env() {
        spawn_lake_scheduler(
            config.db_path.clone(),
            ingest.clone(),
            tumult_lake::lake::LakeConfig::from_env(&config.db_path),
            interval,
        );
    }
    let http_server = tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(http_addr).await?;
        let app = tumult_ingest::http::router_with_token(ingest, http_token)
            .merge(report_router(report_state))
            .merge(tumult_api::router(api_state))
            // Everything that is not /v1, /report, /healthz or /api is the UI.
            .fallback(ui_handler);
        let result = axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal("http"))
            .await;
        tracing::info!("HTTP server future completed: {result:?}");
        result
    });

    let (grpc_result, http_result) = tokio::join!(grpc_server, http_server);
    grpc_result
        .context("gRPC server task panicked")?
        .context("gRPC server")?;
    http_result
        .context("HTTP server task panicked")?
        .context("HTTP server")?;

    tracing::info!("servers stopped; draining ingest writer");
    // Both servers are down; drop their channel clones (moved into the tasks)
    // so the writer task sees the channel close and exits.
    writer_task.await.context("ingest writer task")?;
    tracing::info!("tumultd stopped cleanly");
    Ok(())
}

fn import(file: PathBuf, label: Option<String>) -> Result<()> {
    let config = Config::from_env().map_err(anyhow::Error::msg)?;
    let store = Store::open(&config.db_path)
        .context("open store (stop the daemon first if it is running, or set TUMULT_LAKE_PATH)")?;
    let writer = store.writer()?;
    let summary = ManualImporter::new(&writer)
        .import_file(&file, label)
        .with_context(|| format!("import {}", file.display()))?;
    println!(
        "imported {} rows as {} (batch {})",
        summary.rows, summary.format, summary.batch_id
    );
    Ok(())
}

/// Current time as epoch nanoseconds (row timestamps).
fn now_ns() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as i64)
}

/// Insert a user row with `password` hashed via argon2id. `must_change`
/// marks a one-time bootstrap password that has to be rotated on first login.
fn insert_user(
    writer: &tumult_lake::Writer,
    username: &str,
    password: &str,
    role: &str,
    must_change: bool,
) -> Result<UserRow> {
    let user = UserRow {
        id: uuid::Uuid::new_v4().to_string(),
        username: username.to_string(),
        password_hash: tumult_auth::hash_password(password)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?,
        role: role.to_string(),
        must_change,
        disabled: false,
        created_at_ns: now_ns(),
    };
    writer.create_user(&user)?;
    Ok(user)
}

/// `tumultd create-admin`: generate a one-time password and create the admin
/// user. The password is printed to stdout exactly once and never logged.
fn create_admin(username: &str, db: Option<PathBuf>) -> Result<()> {
    let config = Config::from_env().map_err(anyhow::Error::msg)?;
    let db_path = db.unwrap_or(config.db_path);
    let store = Store::open(&db_path)
        .context("open store (stop the daemon first if it is running, or set TUMULT_LAKE_PATH)")?;
    if store
        .read_only()
        .context("open store read-only")?
        .user_by_username(username)
        .context("check existing users")?
        .is_some()
    {
        anyhow::bail!("user {username:?} already exists");
    }
    let writer = store.writer()?;
    let password = tumult_auth::new_password();
    insert_user(&writer, username, &password, "admin", true)?;
    println!("created admin user: {username}");
    println!("one-time password: {password}");
    println!("this password must be changed on first login");
    Ok(())
}

/// Secure-by-default bind policy plus the demo bootstrap paths, run in
/// `serve` before any server binds (the store is open; the writer is the
/// same one the ingest channel then takes over). "Zero users" below means
/// zero *real* users — the v6 `legacy` backfill identity (disabled,
/// unverifiable) does not count, so an upgraded pre-auth store still
/// bootstraps instead of locking itself out:
///
/// * HTTP bind on a non-loopback host with zero users and no
///   `KRONIKA_BOOTSTRAP_ADMIN_PASSWORD` → refuse to start: the daemon will
///   not expose an unauthenticated API on a network interface.
/// * `KRONIKA_BOOTSTRAP_ADMIN_PASSWORD` set with zero users → create the
///   `admin` user with that exact password (`must_change = false`) — a loud
///   demo/dev path. Ignored (logged) when users already exist.
/// * `KRONIKA_BOOTSTRAP_TOKEN` set in that same zero-users bootstrap →
///   provision a `kro_`-prefixed API token (stored only as its sha256) for
///   the bootstrap admin. The value must start with `kro_`; anything else
///   refuses startup. When no bootstrap admin is created (no password set),
///   the token env var is ignored with a warning: there is no user to own it.
/// * Loopback bind with zero users → start unauthenticated (dev mode), with
///   a warning.
fn enforce_bind_guard(writer: &tumult_lake::Writer, store: &Store, config: &Config) -> Result<()> {
    let http_loopback = tumult_auth::host_is_loopback(&config.otlp_http_addr.ip().to_string());
    let bootstrap_password = std::env::var("KRONIKA_BOOTSTRAP_ADMIN_PASSWORD")
        .ok()
        .filter(|p| !p.is_empty());
    let bootstrap_token = std::env::var("KRONIKA_BOOTSTRAP_TOKEN")
        .ok()
        .filter(|t| !t.is_empty());
    let users_exist = store
        .read_only()
        .context("open store read-only")?
        .real_users_exist()
        .context("check for existing users")?;

    if users_exist {
        if bootstrap_password.is_some() {
            tracing::info!("KRONIKA_BOOTSTRAP_ADMIN_PASSWORD ignored: users already exist");
        }
        if bootstrap_token.is_some() {
            tracing::info!("KRONIKA_BOOTSTRAP_TOKEN ignored: users already exist");
        }
        return Ok(());
    }

    if let Some(password) = bootstrap_password {
        // Validate the token before writing anything: a bad value must not
        // leave a half-provisioned bootstrap behind.
        if let Some(token) = bootstrap_token.as_deref() {
            if !token.starts_with("kro_") {
                anyhow::bail!(
                    "KRONIKA_BOOTSTRAP_TOKEN must start with \"kro_\"; refusing to start"
                );
            }
        }
        let admin = insert_user(writer, "admin", &password, "admin", false)?;
        tracing::warn!(
            "created bootstrap admin from KRONIKA_BOOTSTRAP_ADMIN_PASSWORD — this is a \
             demo/dev bootstrap path and must never be used in production"
        );
        if let Some(token) = bootstrap_token {
            writer.create_token(&TokenRow {
                id: uuid::Uuid::new_v4().to_string(),
                user_id: admin.id.clone(),
                name: "bootstrap".into(),
                token_hash: tumult_auth::sha256_hex(&token),
                created_at_ns: now_ns(),
                last_used_at_ns: None,
                revoked: false,
            })?;
            tracing::warn!(
                "provisioned bootstrap API token for the bootstrap admin (demo/dev path)"
            );
        }
        return Ok(());
    }

    if bootstrap_token.is_some() {
        tracing::warn!(
            "KRONIKA_BOOTSTRAP_TOKEN ignored: no bootstrap admin was created \
             (set KRONIKA_BOOTSTRAP_ADMIN_PASSWORD too)"
        );
    }
    if !http_loopback {
        anyhow::bail!(
            "refusing to serve the API on non-loopback address {} without authentication: \
             run `tumultd create-admin` (with the daemon stopped) or set \
             KRONIKA_BOOTSTRAP_ADMIN_PASSWORD for a demo bootstrap, or bind \
             KRONIKA_OTLP_HTTP_ADDR to 127.0.0.1 for local-only access",
            config.otlp_http_addr
        );
    }
    tracing::warn!(
        "no users exist: the API is running unauthenticated on loopback (dev mode); \
         run `tumultd create-admin` to enable authentication"
    );
    Ok(())
}

fn report(metric: String, out: Option<PathBuf>) -> Result<()> {
    let config = Config::from_env().map_err(anyhow::Error::msg)?;
    match render_metric_report(&config.db_path, &config.metrics_dir, &metric)? {
        ReportLookup::Html(html) => match out {
            Some(path) => {
                std::fs::write(&path, &html)
                    .with_context(|| format!("write report to {}", path.display()))?;
                eprintln!("wrote {}", path.display());
            }
            None => print!("{html}"),
        },
        ReportLookup::UnknownMetric(msg) => anyhow::bail!(msg),
    }
    Ok(())
}

/// Outcome of looking up and rendering one metric report.
enum ReportLookup {
    Html(String),
    UnknownMetric(String),
}

/// Load metric definitions, find `metric`, and render its HTML report against
/// the store at `db_path` (opened read-only). Shared by the `report`
/// subcommand and the live `GET /report` endpoint.
fn render_metric_report(
    db_path: &std::path::Path,
    metrics_dir: &std::path::Path,
    metric: &str,
) -> Result<ReportLookup> {
    let defs = tumult_metrics::load_dir(metrics_dir)
        .with_context(|| format!("load metrics from {}", metrics_dir.display()))?;
    let Some(def) = defs.iter().find(|d| d.name == metric) else {
        return Ok(ReportLookup::UnknownMetric(format!(
            "metric {metric:?} not found; available: {}",
            defs.iter()
                .map(|d| d.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    };

    // Cross-process, DuckDB allows only one process with the store open
    // read-write — so this fails while another daemon holds `db_path`. Inside
    // the daemon process (the /report endpoint) the read-only connection
    // shares the in-process instance and coexists with the ingest writer.
    let store = Store::at(db_path);
    let reader = store.read_only().context("open store read-only")?;
    let report = tumult_report::build_report(
        &reader,
        std::slice::from_ref(def),
        &format!("Krönika — {metric}"),
        None,
    )?;
    Ok(ReportLookup::Html(tumult_report::render_html(&report)))
}

/// State for the live report endpoint: where the store and metric
/// definitions live.
#[derive(Clone)]
struct ReportState {
    db_path: PathBuf,
    metrics_dir: PathBuf,
}

/// The compiled web UI (SvelteKit static SPA), embedded into the binary.
/// `web/build/` must exist at compile time — run `npm ci && npm run build`
/// in `web/` first (the Dockerfile does this in a node stage).
#[derive(rust_embed::RustEmbed)]
#[folder = "../web/build/"]
struct UiAssets;

/// Serve the embedded SPA: real files by path, `index.html` at `/`, and the
/// `200.html` app shell for every other non-API path (client-side routing).
async fn ui_handler(uri: axum::http::Uri) -> Response {
    use axum::http::header;
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    if let Some(file) = UiAssets::get(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        // Fingerprinted assets are safe to cache forever.
        let cache = if path.starts_with("_app/immutable/") {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        };
        let mut resp = (
            [(header::CONTENT_TYPE, mime.as_ref().to_string())],
            file.data,
        )
            .into_response();
        resp.headers_mut().insert(
            header::CACHE_CONTROL,
            cache.parse().expect("static header value"),
        );
        return resp;
    }
    match UiAssets::get("200.html") {
        Some(file) => (
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            file.data,
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "web UI is not embedded").into_response(),
    }
}

/// `GET /report?metric=<name>` — render a metric report from the live store
/// while the daemon is running (used by the docker demo's report step).
fn report_router(state: ReportState) -> Router {
    Router::new()
        .route("/report", get(report_handler))
        .with_state(state)
}

async fn report_handler(
    State(state): State<ReportState>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let Some(metric) = params.get("metric").cloned() else {
        return (StatusCode::BAD_REQUEST, "missing query parameter: metric").into_response();
    };
    let result = tokio::task::spawn_blocking(move || {
        render_metric_report(&state.db_path, &state.metrics_dir, &metric)
    })
    .await;
    match result {
        Ok(Ok(ReportLookup::Html(html))) => Html(html).into_response(),
        Ok(Ok(ReportLookup::UnknownMetric(msg))) => (StatusCode::NOT_FOUND, msg).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("report task failed: {e}"),
        )
            .into_response(),
    }
}

// ---------------------------------------------------------------------------
// Automatic reporting (KRONIKA_REPORT_INTERVAL)

/// Parse an interval value (`45s`, `30m`, `1h`, `1d`, …).
fn parse_interval(raw: &str) -> Option<std::time::Duration> {
    let (num, unit) = raw.split_at(raw.len().checked_sub(1)?);
    let n: u64 = num.trim().parse().ok()?;
    if n == 0 {
        return None;
    }
    let secs = match unit {
        "s" => n,
        "m" => n.checked_mul(60)?,
        "h" => n.checked_mul(3_600)?,
        "d" => n.checked_mul(86_400)?,
        _ => return None,
    };
    Some(std::time::Duration::from_secs(secs))
}

/// `None` when the env var is unset, empty, `0` or `off`; invalid values are
/// warned about and treated as off.
fn report_interval_from_env() -> Option<std::time::Duration> {
    let raw = std::env::var("KRONIKA_REPORT_INTERVAL").ok()?;
    let raw = raw.trim();
    if raw.is_empty() || raw == "0" || raw.eq_ignore_ascii_case("off") {
        return None;
    }
    match parse_interval(raw) {
        Some(d) => {
            tracing::info!(interval = ?d, "automatic reporting enabled");
            Some(d)
        }
        None => {
            tracing::warn!(
                value = raw,
                "invalid KRONIKA_REPORT_INTERVAL (want e.g. 30m or 1h); automatic reporting disabled"
            );
            None
        }
    }
}

/// Render one digest for the trailing `interval` window and write it to
/// `reports_dir/report_<epoch>.html`. When the LLM is reachable, a grounded
/// narrative section is prepended (see `tumult_report::narrative`).
async fn write_digest(
    db_path: &std::path::Path,
    metrics_dir: &std::path::Path,
    reports_dir: &std::path::Path,
    interval: std::time::Duration,
    llm: std::sync::Arc<dyn tumult_intelligence::llm::Llm>,
) -> Result<PathBuf> {
    let (db, mdir) = (db_path.to_path_buf(), metrics_dir.to_path_buf());
    let report = tokio::task::spawn_blocking(move || -> Result<tumult_report::Report> {
        let defs = tumult_metrics::load_dir(&mdir)
            .with_context(|| format!("load metrics from {}", mdir.display()))?;
        let store = Store::at(&db);
        let reader = store.read_only().context("open store read-only")?;
        let now_s = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let from_ns = (now_s - interval.as_secs()) as i64 * 1_000_000_000;
        let to_ns = now_s as i64 * 1_000_000_000;
        Ok(tumult_report::build_report(
            &reader,
            &defs,
            &format!("Krönika digest — last {}s", interval.as_secs()),
            Some((from_ns, to_ns)),
        )?)
    })
    .await??;
    // Best-effort LLM narrative: unreachable LLM, timeout or a reply with no
    // grounded sentences leaves the digest unchanged.
    let report =
        tumult_report::narrative::narrate(&llm, report, std::time::Duration::from_secs(30)).await;
    let now_s = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    std::fs::create_dir_all(reports_dir)?;
    let path = reports_dir.join(format!("report_{now_s}.html"));
    std::fs::write(&path, tumult_report::render_html(&report))
        .with_context(|| format!("write digest to {}", path.display()))?;
    Ok(path)
}

/// Spawn the report scheduler: one digest per interval, written into
/// `<db dir>/reports/` where `/api/reports` picks it up. Failures are logged
/// and the schedule continues.
fn spawn_report_scheduler(
    db_path: PathBuf,
    metrics_dir: PathBuf,
    reports_dir: PathBuf,
    interval: std::time::Duration,
    llm: std::sync::Arc<dyn tumult_intelligence::llm::Llm>,
) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        // Skip the immediate first tick: produce the first digest after one
        // full interval, once ingest has had time to land data.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            let (db, mdir, rdir, llm) = (
                db_path.clone(),
                metrics_dir.clone(),
                reports_dir.clone(),
                llm.clone(),
            );
            match write_digest(&db, &mdir, &rdir, interval, llm).await {
                Ok(path) => {
                    tracing::info!(path = %path.display(), "scheduled digest written")
                }
                Err(e) => tracing::warn!(error = %format!("{e:#}"), "scheduled digest failed"),
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Parquet lake export + retention (KRONIKA_LAKE_*)

/// `Some(24h)` by default; `KRONIKA_LAKE_INTERVAL` overrides, `0`/`off`
/// disables the lake job entirely.
fn lake_interval_from_env() -> Option<std::time::Duration> {
    let default = std::time::Duration::from_secs(86_400);
    let Ok(raw) = std::env::var("KRONIKA_LAKE_INTERVAL") else {
        return Some(default);
    };
    let raw = raw.trim();
    if raw.is_empty() || raw == "0" || raw.eq_ignore_ascii_case("off") {
        return None;
    }
    match parse_interval(raw) {
        Some(d) => Some(d),
        None => {
            tracing::warn!(
                value = raw,
                "invalid KRONIKA_LAKE_INTERVAL (want e.g. 30m or 24h); using 24h"
            );
            Some(default)
        }
    }
}

/// One export pass: fresh read-only reader (a long-lived reader pins its
/// snapshot), then retention deletes on the single writer when the policy
/// asks for them.
async fn run_lake_job(
    db_path: &std::path::Path,
    ingest: &IngestWriter,
    cfg: &tumult_lake::lake::LakeConfig,
) -> Result<()> {
    let (db, cfg2) = (db_path.to_path_buf(), cfg.clone());
    let report = tokio::task::spawn_blocking(move || -> Result<_> {
        let store = Store::at(&db);
        let reader = store.read_only().context("open store read-only")?;
        Ok(tumult_lake::lake::export(&reader, &cfg2)?)
    })
    .await??;
    let total: u64 = report.tables.iter().map(|t| t.rows).sum();
    tracing::info!(
        rows = total,
        files = report.tables.iter().map(|t| t.files.len()).sum::<usize>(),
        dir = %report.lake_dir,
        "lake export complete"
    );
    if cfg.retention_days > 0 {
        let cfg3 = cfg.clone();
        let slot = std::sync::Arc::new(std::sync::Mutex::new(None));
        let slot2 = std::sync::Arc::clone(&slot);
        ingest
            .write(tumult_ingest::Batch::Exec(Box::new(move |writer| {
                *slot2.lock().unwrap_or_else(|e| e.into_inner()) = Some(
                    tumult_lake::lake::enforce_retention(writer, &cfg3).map_err(|e| e.to_string()),
                );
                Ok(())
            })))
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        match slot.lock().unwrap_or_else(|e| e.into_inner()).take() {
            Some(Ok(deleted)) => {
                let total: u64 = deleted.values().sum();
                if total > 0 {
                    tracing::info!(rows = total, "lake retention reclaimed hot rows");
                }
            }
            Some(Err(e)) => anyhow::bail!("retention failed: {e}"),
            None => anyhow::bail!("retention did not run"),
        };
    }
    Ok(())
}

/// Spawn the lake scheduler: one export (+ optional retention) per interval.
/// Failures are logged and the schedule continues — the watermark makes the
/// next run retry from the last good state.
fn spawn_lake_scheduler(
    db_path: PathBuf,
    ingest: IngestWriter,
    cfg: tumult_lake::lake::LakeConfig,
    interval: std::time::Duration,
) {
    tracing::info!(
        interval = ?interval,
        dir = %cfg.dir.display(),
        retention_days = cfg.retention_days,
        "lake export job enabled"
    );
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        // Skip the immediate first tick: export after one full interval.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if let Err(e) = run_lake_job(&db_path, &ingest, &cfg).await {
                tracing::warn!(error = %format!("{e:#}"), "lake export job failed");
            }
        }
    });
}
