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
//! Both listeners default to `0.0.0.0`; on a non-loopback bind the daemon
//! fails closed without `KRONIKA_INGEST_TOKEN` (unauthenticated ingest would
//! accept telemetry from anywhere) and without API users (see
//! `enforce_bind_guard`). TLS is optional via `KRONIKA_TLS_CERT` /
//! `KRONIKA_TLS_KEY` (PEM): when set, the axum HTTP server serves HTTPS and
//! the tonic gRPC server serves TLS with the same pair, and the daemon's own
//! exporter loopback moves to a plaintext ephemeral-port loopback listener;
//! when unset, a network-exposed bind logs a loud plaintext warning.
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
    about = "Tumult — analytics for your resilience work"
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

/// Validated TLS materials for both servers: the rustls config for the
/// HTTPS (axum) listener and the PEM identity for the tonic gRPC server.
struct TlsMaterials {
    http: axum_server::tls_rustls::RustlsConfig,
    grpc_identity: tonic::transport::Identity,
}

/// Load the optional TLS configuration (`KRONIKA_TLS_CERT` /
/// `KRONIKA_TLS_KEY`). Returns `None` when TLS is not configured; when it is,
/// the certificate chain and key are parsed here so a bad pair fails startup
/// with a clear message before any listener binds.
async fn load_tls(config: &Config) -> Result<Option<TlsMaterials>> {
    let (Some(cert), Some(key)) = (config.tls_cert.as_deref(), config.tls_key.as_deref()) else {
        return Ok(None);
    };
    // rustls has both the ring and aws-lc-rs providers compiled in via the
    // wider dependency graph; without an explicit process default, building a
    // `ServerConfig` panics on the ambiguity. Ignore "already installed".
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let http = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key)
        .await
        .with_context(|| {
            format!(
                "load TLS certificate chain and key from KRONIKA_TLS_CERT ({}) and \
                 KRONIKA_TLS_KEY ({})",
                cert.display(),
                key.display()
            )
        })?;
    let cert_pem = std::fs::read(cert)
        .with_context(|| format!("read KRONIKA_TLS_CERT ({})", cert.display()))?;
    let key_pem =
        std::fs::read(key).with_context(|| format!("read KRONIKA_TLS_KEY ({})", key.display()))?;
    let grpc_identity = tonic::transport::Identity::from_pem(cert_pem, key_pem);
    tracing::info!(
        cert = %cert.display(),
        "TLS enabled for the HTTP and gRPC servers"
    );
    Ok(Some(TlsMaterials {
        http,
        grpc_identity,
    }))
}

/// Loud startup warning when a network-exposed listener serves plaintext:
/// bearer tokens and telemetry cross the wire unencrypted.
fn warn_if_plaintext_on_network(config: &Config, tls_enabled: bool) {
    if tls_enabled {
        return;
    }
    if !config.otlp_http_addr.ip().is_loopback() {
        tracing::warn!(
            addr = %config.otlp_http_addr,
            "TLS is OFF: the API, web UI and OTLP/HTTP ingest are served in plaintext on a \
             network interface — bearer tokens and telemetry cross the wire unencrypted; \
             set KRONIKA_TLS_CERT/KRONIKA_TLS_KEY or terminate TLS at a reverse proxy"
        );
    }
    if !config.otlp_grpc_addr.ip().is_loopback() {
        tracing::warn!(
            addr = %config.otlp_grpc_addr,
            "TLS is OFF: OTLP/gRPC ingest is served in plaintext on a network interface; \
             set KRONIKA_TLS_CERT/KRONIKA_TLS_KEY or terminate TLS at a reverse proxy"
        );
    }
}

async fn serve() -> Result<()> {
    let config = Config::from_env().map_err(anyhow::Error::msg)?;
    // Fail-closed: refuse a token-less OTLP ingest on a network interface.
    config.ensure_ingest_auth().map_err(anyhow::Error::msg)?;
    // Load (and thereby validate) the TLS materials before binding anything:
    // a bad cert/key fails startup here with a clear message.
    let tls = load_tls(&config).await?;
    warn_if_plaintext_on_network(&config, tls.is_some());
    tracing::info!(
        db = %config.db_path.display(),
        grpc = %config.otlp_grpc_addr,
        http = %config.otlp_http_addr,
        tls = tls.is_some(),
        "starting tumultd"
    );

    // With gRPC TLS enabled, the daemon's own exporter cannot speak TLS back
    // to it (the opentelemetry-otlp client here is built without TLS), so it
    // exports to a plaintext listener on an ephemeral loopback port instead:
    // network-facing ingest stays TLS-only while self-telemetry keeps working.
    // An unspecified public bind (0.0.0.0) exports via localhost.
    let internal_grpc_listener = if tls.is_some() {
        Some(
            tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                .await
                .context("bind internal loopback gRPC listener")?,
        )
    } else {
        None
    };
    let loopback = if let Some(listener) = &internal_grpc_listener {
        format!("http://{}", listener.local_addr()?)
    } else if config.otlp_grpc_addr.ip().is_unspecified() {
        format!("http://127.0.0.1:{}", config.otlp_grpc_addr.port())
    } else {
        format!("http://{}", config.otlp_grpc_addr)
    };
    // When the ingest is token-guarded the loopback must authenticate too —
    // otherwise its spans are rejected (UNAUTHENTICATED) and silently dropped.
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

    // OTLP/gRPC server (tumult exporter target) — TLS when configured.
    // The router is built before the task spawns so an invalid TLS identity
    // fails startup here instead of inside the task.
    let grpc_addr = config.otlp_grpc_addr;
    let grpc_router = tumult_ingest::grpc::router_with_token_tls(
        ingest.clone(),
        config.ingest_token.clone(),
        tls.as_ref().map(|t| t.grpc_identity.clone()),
    )
    .context("invalid TLS identity for the gRPC server (KRONIKA_TLS_CERT/KRONIKA_TLS_KEY)")?;
    let grpc_server = tokio::spawn(async move {
        let result = grpc_router
            .serve_with_shutdown(grpc_addr, shutdown_signal("grpc"))
            .await;
        tracing::info!("gRPC server future completed: {result:?}");
        result
    });

    // Internal plaintext gRPC loopback (only when the public gRPC listener
    // serves TLS): carries the daemon's own exporter traffic, bound to an
    // ephemeral 127.0.0.1 port chosen before the telemetry init above.
    let internal_grpc_server = internal_grpc_listener.map(|listener| {
        let router =
            tumult_ingest::grpc::router_with_token(ingest.clone(), config.ingest_token.clone());
        tokio::spawn(async move {
            let result = router
                .serve_with_incoming_shutdown(
                    tokio_stream::wrappers::TcpListenerStream::new(listener),
                    shutdown_signal("grpc-loopback"),
                )
                .await;
            tracing::info!("internal gRPC loopback server future completed: {result:?}");
            result
        })
    });

    // OTLP/HTTP server (smedja exporter target) + health + live reports + the
    // read-only query API backing the web UI.
    let http_addr = config.otlp_http_addr;
    let http_tls = tls.as_ref().map(|t| t.http.clone());
    let http_token = config.ingest_token.clone();
    let report_state = ReportState {
        db_path: config.db_path.clone(),
        metrics_dir: config.metrics_dir.clone(),
    };
    let api_state = tumult_api::ApiState::from_env_parts(
        config.db_path.clone(),
        config.metrics_dir.clone(),
        Some(ingest.clone()),
        Some(run_queue.clone()),
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
    // Background tasks holding an `IngestWriter` clone must stop — and drop
    // that clone — before the drain below waits for the writer channel to
    // close, or shutdown hangs on the still-open channel.
    let background_shutdown = tokio_util::sync::CancellationToken::new();
    let lake_task = lake_interval_from_env().map(|interval| {
        spawn_lake_scheduler(
            config.db_path.clone(),
            ingest.clone(),
            tumult_lake::lake::LakeConfig::from_env(&config.db_path),
            interval,
            background_shutdown.clone(),
        )
    });
    let http_server = tokio::spawn(async move {
        let app = tumult_ingest::http::router_with_token(ingest, http_token)
            .merge(report_router(report_state))
            .merge(tumult_api::router(api_state))
            // Everything that is not /v1, /report, /healthz or /api is the UI.
            .fallback(ui_handler);
        let result = match http_tls {
            Some(tls_config) => {
                let handle = axum_server::Handle::new();
                let shutdown = handle.clone();
                tokio::spawn(async move {
                    shutdown_signal("https").await;
                    shutdown.graceful_shutdown(Some(std::time::Duration::from_secs(10)));
                });
                let result = axum_server::bind_rustls(http_addr, tls_config)
                    .handle(handle)
                    .serve(app.into_make_service_with_connect_info::<std::net::SocketAddr>())
                    .await;
                tracing::info!("HTTPS server future completed: {result:?}");
                result
            }
            None => {
                let listener = tokio::net::TcpListener::bind(http_addr).await?;
                // ConnectInfo gives /api/auth/login the peer IP for its
                // per-ip|username throttle key.
                let result = axum::serve(
                    listener,
                    app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
                )
                .with_graceful_shutdown(shutdown_signal("http"))
                .await;
                tracing::info!("HTTP server future completed: {result:?}");
                result
            }
        };
        result
    });

    let (grpc_result, http_result) = tokio::join!(grpc_server, http_server);
    grpc_result
        .context("gRPC server task panicked")?
        .context("gRPC server")?;
    http_result
        .context("HTTP server task panicked")?
        .context("HTTP server")?;
    if let Some(task) = internal_grpc_server {
        task.await
            .context("internal gRPC loopback task panicked")?
            .context("internal gRPC loopback server")?;
    }

    tracing::info!("servers stopped; draining ingest writer");
    // Both servers are down; their channel clones (moved into the tasks)
    // are dropped. Order matters from here: signal the background tasks
    // that hold an `IngestWriter` clone, wait for them to finish so those
    // clones are dropped, and only then wait for the writer channel to
    // close — the drain completes only once every sender is gone.
    background_shutdown.cancel();
    if let Some(task) = lake_task {
        task.await.context("lake scheduler task panicked")?;
    }
    run_queue.shutdown();
    drop(run_queue);
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
                expires_at_ns: None,
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
        &format!("Tumult — {metric}"),
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
            &format!("Tumult digest — last {}s", interval.as_secs()),
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
/// next run retry from the last good state. The task holds an `IngestWriter`
/// clone, so it must stop on `shutdown` (dropping the clone) before the
/// daemon's drain waits for the writer channel to close; the returned handle
/// lets the caller wait for exactly that. An export already in flight runs
/// to completion first.
fn spawn_lake_scheduler(
    db_path: PathBuf,
    ingest: IngestWriter,
    cfg: tumult_lake::lake::LakeConfig,
    interval: std::time::Duration,
    shutdown: tokio_util::sync::CancellationToken,
) -> tokio::task::JoinHandle<()> {
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
            tokio::select! {
                _ = ticker.tick() => {
                    if let Err(e) = run_lake_job(&db_path, &ingest, &cfg).await {
                        tracing::warn!(error = %format!("{e:#}"), "lake export job failed");
                    }
                }
                () = shutdown.cancelled() => {
                    tracing::info!("lake export job stopping (shutdown)");
                    break;
                }
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Tests

#[cfg(test)]
mod tests {
    use super::*;

    /// Throwaway self-signed `localhost` certificate/key pair (generated with
    /// `openssl req -x509`), used to exercise the TLS load path.
    const CERT_PEM: &str = "-----BEGIN CERTIFICATE-----
MIIDJTCCAg2gAwIBAgIUHvcqpsPfQCAnuxXOJLnZKClsYrswDQYJKoZIhvcNAQEL
BQAwFDESMBAGA1UEAwwJbG9jYWxob3N0MB4XDTI2MDczMTEyNTYyOFoXDTI2MDgw
MTEyNTYyOFowFDESMBAGA1UEAwwJbG9jYWxob3N0MIIBIjANBgkqhkiG9w0BAQEF
AAOCAQ8AMIIBCgKCAQEAtMVXRp4+rZ/6CfN2uLYBdUuI/3STDSWr6piuVvCi7S1J
bszJTo9ILKvvljKkhhkV9u4g+6cPzD5rt/rd8/92EWC+8lVkYPoLPM9ydLa4VbqV
/z6P7xt2DQmTY8oqfI+GkiXBvLfMKW63bqfYAEp+ZzwZPKibmykCmUScGGOfeezd
YinTrvDnlrttw6Jf5H6eu/CJAmZA1iKqjCQcxv3guUgUN6PECwfc7kIvCnSvNSlo
38f5zzg4xiyUA/larv9HtL4WZVrXwXnIrYv+tbA2eJJxUKwr8Pck/XKUHn5KhtDa
hmEp3wMdjJQSbXclgsyG43jIQ25CldbfYHEComUpTQIDAQABo28wbTAdBgNVHQ4E
FgQU7hn+N0c9xOL25lD8KslytcX7U0IwHwYDVR0jBBgwFoAU7hn+N0c9xOL25lD8
KslytcX7U0IwDwYDVR0TAQH/BAUwAwEB/zAaBgNVHREEEzARgglsb2NhbGhvc3SH
BH8AAAEwDQYJKoZIhvcNAQELBQADggEBAAUvKDh7FLD/DtMlERVvPiu7rtgBfLcK
S2Wd858fZ/IhvR3mvXNQxcWVsfjb8/O5ZlY0/+YCZMTjuL9YpBPix3WY50ktjHDk
f1HBjXWNS8hLLKpM7f3jwFsuE/OaYdRTu7ob2JOI2lDIjajHpKp0XU/a9pTq+xNX
XNpP0Bj7ElEmQKtNeJUqoMXzT1pPQJNpDLAyaNSYbz4yuSy7tXNoMI4Dy2VB5nLB
ye+6Z1fl8Y9TdaxZBQolepjbBEQQ/dqeu3+WLn17FInao9yR81/7J0CxAtv1YXen
qPsqtfAE6Ug4+5TQUqwIxtvzYHFF+pG2Lx+fcDCTQN3GDwXpJJdUQo8=
-----END CERTIFICATE-----
";
    const KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----
MIIEvAIBADANBgkqhkiG9w0BAQEFAASCBKYwggSiAgEAAoIBAQC0xVdGnj6tn/oJ
83a4tgF1S4j/dJMNJavqmK5W8KLtLUluzMlOj0gsq++WMqSGGRX27iD7pw/MPmu3
+t3z/3YRYL7yVWRg+gs8z3J0trhVupX/Po/vG3YNCZNjyip8j4aSJcG8t8wpbrdu
p9gASn5nPBk8qJubKQKZRJwYY5957N1iKdOu8OeWu23Dol/kfp678IkCZkDWIqqM
JBzG/eC5SBQ3o8QLB9zuQi8KdK81KWjfx/nPODjGLJQD+Vqu/0e0vhZlWtfBecit
i/61sDZ4knFQrCvw9yT9cpQefkqG0NqGYSnfAx2MlBJtdyWCzIbjeMhDbkKV1t9g
cQKiZSlNAgMBAAECggEAOqSUShP2+GNf/Y9uUcC1m2QcNucN92NjsJDEae7ZpACf
hGLJ4YLo4pkKedrG9bu4nOkmaQ0KunL7he1LyKZ0mnGcsEfUbwNe1uTjWAqYpTMJ
Cws0LVjmxJb5KhPBEbSL7uhxv7OOd1h0CGFJ2NpRxFLCSyPVixHURn1z+BOFfks1
GVi+e5oGG0nsGMw7EJ7dYr6Qu1mmvRuulshJlbTRFrveKXW5qIZLGsnkmvhR8A69
N9hbvsALfo4yaeKEQTXWzCBmYYdDPqRVosGOuwOSG10NP3O5KH2WAsrGX5J2qTCZ
4d3RMTJ9jdET3CQ4r3mhLaG2cegiWB7LLZpRF0ulXwKBgQDwlp61p7KfRfCYj25f
AgcZwL/KjTVD866XRz2p7LVTTxG2kZIIjaHYOZcenACe3KJ75BnrN6d7m/1GeW3N
XgVdiW35s2JoR07hRH+fdF1dBx/DSMMOhB9YquOuNEzgq+LSgb017kUfUaOkQFEY
mMxs6HgNA8Yq0v0l8zdDoD+UHwKBgQDAWcfqN55NQr76z8nTnKKjeuMPoL7WSJ+J
CBgVbP/eZAefnKqhA7MZFZZOfKmsBtspeHqj78ZqTXysZOAXSWCNlFnmw5EZ9NXf
T8WO+SVFt7S3ykjYsSMfC9+aBYQpyFjjp4Sjp/gyYGdMTt/y/kMEomx6+lEiindh
iKlK6aV1EwKBgBRQw6oXNRAZ+c0IH4vKQgs8qXVTIzJPu2huzZgxssYMITTHagtq
2kXF5yrghXTksJvBkSa5llzruSFgU5NJ4y4Y0r6JFUA09UY0YIp4awHV/iqhVEc/
hN4Z4AvvwqYeHZMk/XM2YYPZgvX1sGNhU7HGl4yRywQGuPWhagM93uCFAoGAGy/V
aM5pqoPnmG2sGiPGfRLOaxQORR1Ip0akmMqqM5Wx2iZ7m3x5YO9DKl7GYJErguYL
d4ZZZgcDux4a6k+tvPUd69byeFe5rvGIe9fNI9h+S4fk2fPXgfjcptlmv70YizzP
K45/LyefEhMH5kF32XzXll4w/4/QpdF6FCOIBk8CgYAY84rMAeJIZ6Os7PUOVTpP
RtbpfPUQivnmWXg4j9FMktcGRwf+jd+gY+0zx154foHbKzfCIU+Knn64WpWdqJb9
xXwj/ADaeC8lQd9LqUwMRysvfyIE9z+/Xky/FGNpoql7xskwe76x34sO+VoPBW3c
xPXAffHX6Z04foGNwjzXeg==
-----END PRIVATE KEY-----
";

    fn config_with_tls(cert: Option<&std::path::Path>, key: Option<&std::path::Path>) -> Config {
        Config {
            db_path: PathBuf::from("/tmp/db.duckdb"),
            otlp_grpc_addr: "127.0.0.1:4317".parse().unwrap(),
            otlp_http_addr: "127.0.0.1:4318".parse().unwrap(),
            metrics_dir: PathBuf::from("metrics"),
            ingest_token: None,
            tls_cert: cert.map(std::path::Path::to_path_buf),
            tls_key: key.map(std::path::Path::to_path_buf),
        }
    }

    #[tokio::test]
    async fn tls_unset_stays_plaintext() {
        assert!(load_tls(&config_with_tls(None, None))
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn tls_missing_files_fail_fast_with_clear_message() {
        let config = config_with_tls(
            Some(std::path::Path::new("/nonexistent/tls.crt")),
            Some(std::path::Path::new("/nonexistent/tls.key")),
        );
        let err = load_tls(&config).await.err().unwrap();
        let msg = format!("{err:#}");
        assert!(msg.contains("KRONIKA_TLS_CERT"), "{msg}");
    }

    #[tokio::test]
    async fn tls_garbage_pem_fails_fast() {
        let dir = tempfile::tempdir().unwrap();
        let cert = dir.path().join("tls.crt");
        let key = dir.path().join("tls.key");
        std::fs::write(&cert, "not a pem").unwrap();
        std::fs::write(&key, "not a pem").unwrap();
        assert!(load_tls(&config_with_tls(Some(&cert), Some(&key)))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn tls_mismatched_key_fails_fast() {
        let dir = tempfile::tempdir().unwrap();
        let cert = dir.path().join("tls.crt");
        let key = dir.path().join("tls.key");
        std::fs::write(&cert, CERT_PEM).unwrap();
        // A valid PEM but the wrong half: cert as key.
        std::fs::write(&key, CERT_PEM).unwrap();
        assert!(load_tls(&config_with_tls(Some(&cert), Some(&key)))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn tls_valid_pair_loads() {
        let dir = tempfile::tempdir().unwrap();
        let cert = dir.path().join("tls.crt");
        let key = dir.path().join("tls.key");
        std::fs::write(&cert, CERT_PEM).unwrap();
        std::fs::write(&key, KEY_PEM).unwrap();
        assert!(load_tls(&config_with_tls(Some(&cert), Some(&key)))
            .await
            .unwrap()
            .is_some());
    }

    /// Regression: shutdown must complete with the lake scheduler enabled.
    /// The scheduler holds an `IngestWriter` clone; before the cancellation
    /// token existed, that clone kept the writer channel open forever and the
    /// drain hung. Mirroring `serve`'s shutdown order (signal → wait →
    /// drain), each wait is timeout-guarded so a regression fails fast
    /// instead of hanging the test run.
    #[tokio::test]
    async fn shutdown_drains_with_lake_scheduler_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("k.duckdb");
        let store = Store::open(&db_path).unwrap();
        let (ingest, writer_task) = IngestWriter::spawn(store.writer().unwrap(), 8);
        let shutdown = tokio_util::sync::CancellationToken::new();
        let lake_task = spawn_lake_scheduler(
            db_path.clone(),
            ingest.clone(),
            tumult_lake::lake::LakeConfig::new(dir.path().join("lake"), 0),
            std::time::Duration::from_secs(3600),
            shutdown.clone(),
        );

        shutdown.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(10), lake_task)
            .await
            .expect("lake scheduler did not stop on shutdown")
            .expect("lake scheduler task panicked");
        drop(ingest);
        tokio::time::timeout(std::time::Duration::from_secs(10), writer_task)
            .await
            .expect("ingest writer drain hung — a sender is still alive")
            .expect("ingest writer task panicked");
    }
}
