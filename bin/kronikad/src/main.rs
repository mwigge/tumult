//! `kronikad` — the kronika daemon.
//!
//! Default command (`serve`): open the store, spawn the single-writer
//! channel, and run the OTLP/gRPC (`:4317`) and OTLP/HTTP (`:4318`, with
//! `GET /healthz`) ingest servers until SIGTERM/SIGINT. The HTTP server also
//! exposes `GET /report?metric=<name>`, which renders a metric report from
//! the live store — this works while the daemon holds the write lock, unlike
//! the `report` subcommand which needs the daemon stopped.
//!
//! The same HTTP server mounts the read-only query API (`/api/*`, from
//! `kronika-api`) that backs the web UI. When `KRONIKA_REPORT_INTERVAL`
//! (e.g. `1h`, `30m`) is set, a scheduler renders a metric digest per
//! interval into `<db dir>/reports/report_<epoch>.html`; `/api/reports`
//! lists them. Automatic reporting is off by default.
//!
//! Subcommands:
//! * `kronikad import <file>` — manual CSV / tumult journal JSON import.
//! * `kronikad report --metric <name>` — print an HTML report to stdout.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use clap::{Parser, Subcommand};
use kronika_ingest::{Config, IngestWriter, ManualImporter};
use kronika_store::Store;

#[derive(Parser)]
#[command(
    name = "kronikad",
    version,
    about = "kronika — the chronicle of your resilience work"
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
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "kronikad=info,kronika_ingest=info".into()),
        )
        .init();

    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => serve().await,
        Command::Import { file, label } => import(file, label),
        Command::Report { metric, out } => report(metric, out),
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

async fn serve() -> Result<()> {
    let config = Config::from_env().map_err(anyhow::Error::msg)?;
    tracing::info!(
        db = %config.db_path.display(),
        grpc = %config.otlp_grpc_addr,
        http = %config.otlp_http_addr,
        "starting kronikad"
    );

    let store = Store::open(&config.db_path).context("open store")?;
    let writer = store.writer().context("open store writer")?;
    let (ingest, writer_task) = IngestWriter::spawn(writer, 1024);

    // OTLP/gRPC server (tumult exporter target).
    let grpc_addr = config.otlp_grpc_addr;
    let grpc_ingest = ingest.clone();
    let grpc_server = tokio::spawn(async move {
        let result = kronika_ingest::grpc::router(grpc_ingest)
            .serve_with_shutdown(grpc_addr, shutdown_signal("grpc"))
            .await;
        tracing::info!("gRPC server future completed: {result:?}");
        result
    });

    // OTLP/HTTP server (smedja exporter target) + health + live reports + the
    // read-only query API backing the web UI.
    let http_addr = config.otlp_http_addr;
    let report_state = ReportState {
        db_path: config.db_path.clone(),
        metrics_dir: config.metrics_dir.clone(),
    };
    let api_state =
        kronika_api::ApiState::from_env_parts(config.db_path.clone(), config.metrics_dir.clone());
    if let Some(interval) = report_interval_from_env() {
        spawn_report_scheduler(
            config.db_path.clone(),
            config.metrics_dir.clone(),
            api_state.reports_dir().clone(),
            interval,
            std::sync::Arc::new(kronika_ai::OpenAiCompatClient::from_env()),
        );
    }
    let http_server = tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(http_addr).await?;
        let app = kronika_ingest::http::router(ingest)
            .merge(report_router(report_state))
            .merge(kronika_api::router(api_state))
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
    tracing::info!("kronikad stopped cleanly");
    Ok(())
}

fn import(file: PathBuf, label: Option<String>) -> Result<()> {
    let config = Config::from_env().map_err(anyhow::Error::msg)?;
    let store = Store::open(&config.db_path)
        .context("open store (stop the daemon first if it is running, or set KRONIKA_DB)")?;
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
    let defs = kronika_metrics::load_dir(metrics_dir)
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
    let report = kronika_report::build_report(
        &reader,
        std::slice::from_ref(def),
        &format!("kronika — {metric}"),
        None,
    )?;
    Ok(ReportLookup::Html(kronika_report::render_html(&report)))
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
#[folder = "../../web/build/"]
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

/// Parse a `KRONIKA_REPORT_INTERVAL` value (`45s`, `30m`, `1h`, …).
fn parse_report_interval(raw: &str) -> Option<std::time::Duration> {
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
    match parse_report_interval(raw) {
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
/// narrative section is prepended (see `kronika_report::narrative`).
async fn write_digest(
    db_path: &std::path::Path,
    metrics_dir: &std::path::Path,
    reports_dir: &std::path::Path,
    interval: std::time::Duration,
    llm: std::sync::Arc<dyn kronika_ai::Llm>,
) -> Result<PathBuf> {
    let (db, mdir) = (db_path.to_path_buf(), metrics_dir.to_path_buf());
    let report = tokio::task::spawn_blocking(move || -> Result<kronika_report::Report> {
        let defs = kronika_metrics::load_dir(&mdir)
            .with_context(|| format!("load metrics from {}", mdir.display()))?;
        let store = Store::at(&db);
        let reader = store.read_only().context("open store read-only")?;
        let now_s = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let from_ns = (now_s - interval.as_secs()) as i64 * 1_000_000_000;
        let to_ns = now_s as i64 * 1_000_000_000;
        Ok(kronika_report::build_report(
            &reader,
            &defs,
            &format!("kronika digest — last {}s", interval.as_secs()),
            Some((from_ns, to_ns)),
        )?)
    })
    .await??;
    // Best-effort LLM narrative: unreachable LLM, timeout or a reply with no
    // grounded sentences leaves the digest unchanged.
    let report =
        kronika_report::narrative::narrate(&llm, report, std::time::Duration::from_secs(30)).await;
    let now_s = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    std::fs::create_dir_all(reports_dir)?;
    let path = reports_dir.join(format!("report_{now_s}.html"));
    std::fs::write(&path, kronika_report::render_html(&report))
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
    llm: std::sync::Arc<dyn kronika_ai::Llm>,
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
