//! The `tumultd serve` wiring: config, TLS ([`tls`]), the OTLP gRPC/HTTP
//! servers, the background supervisors, and the shutdown drain. The
//! embedded web UI lives in [`ui`], tests in `tests`.

#[cfg(test)]
mod tests;
mod tls;
mod ui;

use anyhow::{Context, Result};
use tumult_ingest::{Config, IngestWriter};
use tumult_lake::Store;

use crate::admin::enforce_bind_guard;
use crate::lake_jobs::{
    lake_interval_from_env, spawn_lake_scheduler, spawn_version_backfill, BACKFILL_PAUSE,
};
use crate::reports::{
    report_interval_from_env, report_router, spawn_report_scheduler, ReportState,
};
use tls::{load_tls, warn_if_plaintext_on_network};
use ui::ui_handler;

/// One listener task and how it ended.
type Listener<E> = tokio::task::JoinHandle<std::result::Result<(), E>>;

async fn stopped<E>(listener: &mut Listener<E>, name: &'static str) -> Result<()>
where
    E: std::error::Error + Send + Sync + 'static,
{
    listener
        .await
        .with_context(|| format!("{name} task panicked"))?
        .context(name)
}

/// Waits until both listeners have stopped. A listener that fails ends
/// the wait at once and stops the other one: a daemon that could not
/// bind its HTTP address must not stay up answering gRPC only.
async fn until_both_stop<G, H>(mut grpc: Listener<G>, mut http: Listener<H>) -> Result<()>
where
    G: std::error::Error + Send + Sync + 'static,
    H: std::error::Error + Send + Sync + 'static,
{
    let outcome = tokio::try_join!(
        stopped(&mut grpc, "gRPC server"),
        stopped(&mut http, "HTTP server"),
    );
    let Err(error) = outcome else {
        return Ok(());
    };
    tracing::error!(error = %format!("{error:#}"), "a listener failed; stopping the daemon");
    grpc.abort();
    http.abort();
    Err(error)
}

/// What runs beside the listeners and holds the ingest writer open.
struct Workers {
    /// Tells the background tasks to stop.
    shutdown: tokio_util::sync::CancellationToken,
    /// The background tasks by name; each holds an `IngestWriter` clone.
    tasks: Vec<(&'static str, tokio::task::JoinHandle<()>)>,
    run_queue: tumult_ingest::RunQueue,
    writer: tokio::task::JoinHandle<()>,
}

impl Workers {
    /// Stop everything and wait until the writer has persisted what was
    /// queued. Order matters: signal the background tasks that hold an
    /// `IngestWriter` clone, wait for them to finish so those clones are
    /// dropped, and only then wait for the writer channel to close: the
    /// drain completes only once every sender is gone.
    async fn drain(self) -> Result<()> {
        self.shutdown.cancel();
        for (name, task) in self.tasks {
            task.await
                .with_context(|| format!("{name} task panicked"))?;
        }
        self.run_queue.shutdown();
        drop(self.run_queue);
        self.writer.await.context("ingest writer task")
    }
}

/// How long a daemon that stops because a listener failed waits for the
/// drain. A failed listener is aborted, not shut down gracefully, so a
/// connection it accepted may live on holding the writer open; a clean
/// stop has no such connection and waits without a limit.
const FAILED_DRAIN_LIMIT: std::time::Duration = std::time::Duration::from_secs(10);

/// The end of the daemon's life, once the public listeners have stopped
/// as `served` says: the loopback listener, then the drain. A listener
/// that failed is still the outcome, but only after the drain: what was
/// queued for the writer is persisted on every way out.
async fn finish<E>(
    served: Result<()>,
    loopback: Option<Listener<E>>,
    workers: Workers,
    failed_drain_limit: std::time::Duration,
) -> Result<()>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let served = match (served, loopback) {
        (Ok(()), Some(mut task)) => stopped(&mut task, "internal gRPC loopback server").await,
        (failed, Some(task)) => {
            task.abort();
            failed
        }
        (served, None) => served,
    };
    let Err(failure) = served else {
        tracing::info!("servers stopped; draining ingest writer");
        return workers.drain().await;
    };
    tracing::info!("a listener failed; draining ingest writer before exit");
    match tokio::time::timeout(failed_drain_limit, workers.drain()).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tracing::error!(error = %format!("{error:#}"), "drain after a listener failure failed");
        }
        Err(_) => tracing::error!(
            limit_ms = failed_drain_limit.as_millis() as u64,
            "ingest writer still held open after a listener failure; exiting without waiting longer"
        ),
    }
    Err(failure)
}

/// Bind one of the daemon's public addresses, naming it when it cannot
/// be had.
async fn bind(addr: std::net::SocketAddr, name: &'static str) -> Result<tokio::net::TcpListener> {
    tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("{name}: bind {addr}"))
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

pub(crate) async fn serve() -> Result<()> {
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

    // Both public addresses are bound here, before the writer, the run
    // queue or any background task exists: a daemon that cannot have its
    // addresses exits having started, and touched, nothing.
    let grpc_listener = bind(config.otlp_grpc_addr, "gRPC server").await?;
    let http_listener = bind(config.otlp_http_addr, "HTTP server").await?;

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
    let grpc_router =
        tumult_ingest::grpc::router_with_token(ingest.clone(), config.ingest_token.clone());
    // Connections are admitted under `KRONIKA_GRPC_MAX_CONNECTIONS` and the
    // per-peer and per-site shares; unary OTLP exports and OTel-Arrow
    // streams share them. With an ingest token a connection must make an
    // authorised call in time and gives way to a newcomer until it has;
    // without one the listener cannot tell clients apart.
    let grpc_limits = tumult_ingest::listener::ConnectionLimits::from_env();
    let grpc_access = match config.ingest_token {
        Some(_) => tumult_ingest::listener::Access::Token,
        None => tumult_ingest::listener::Access::Open,
    };
    let grpc_incoming = tumult_ingest::listener::incoming(grpc_listener, grpc_limits, grpc_access);
    // The listener does the TLS handshake: its deadline ends when the
    // handshake is complete, which only the side doing it knows.
    let grpc_incoming = match tls.as_ref() {
        Some(tls) => grpc_incoming.with_tls(tls.grpc.clone()),
        None => grpc_incoming,
    };
    let grpc_server = tokio::spawn(async move {
        let result = grpc_router
            .serve_with_incoming_shutdown(grpc_incoming, shutdown_signal("grpc"))
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
    )
    .forecast_from_env()
    .map_err(anyhow::Error::msg)?;
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
    // The open gave the backfill of the version rollup a short budget;
    // the rest is done here, a turn at a time, beside the telemetry.
    let backfill_task =
        spawn_version_backfill(ingest.clone(), BACKFILL_PAUSE, background_shutdown.clone());
    // Schedule scheduler: fires due interval schedules through the normal
    // run path every tick. Holds an IngestWriter clone, so it is cancelled
    // and awaited before the writer drain below, like the lake task.
    let schedule_task = tumult_ingest::schedules::spawn_schedule_scheduler(
        config.db_path.clone(),
        ingest.clone(),
        run_queue.clone(),
        tumult_ingest::schedules::tick_from_env(),
        background_shutdown.clone(),
    );
    // Webhook dispatcher: delivers due run-audit events to enabled webhooks
    // every tick. Same shutdown-drain contract as the other background tasks.
    let webhook_task = tumult_ingest::webhooks::spawn_webhook_dispatcher(
        config.db_path.clone(),
        ingest.clone(),
        tumult_ingest::webhooks::tick_from_env(),
        background_shutdown.clone(),
    );
    // GameDay supervisor: advances active campaigns through their
    // experiments as sequential child runs.
    let gameday_task = tumult_ingest::gamedays::spawn_gameday_supervisor(
        config.db_path.clone(),
        ingest.clone(),
        run_queue.clone(),
        tumult_ingest::gamedays::tick_from_env(),
        background_shutdown.clone(),
    );
    // Run retention: sweeps terminal runs (and their audit trails) older
    // than TUMULTD_RUN_RETENTION_DAYS. Same shutdown-drain contract.
    let retention_task = tumult_ingest::retention::spawn_run_retention(
        ingest.clone(),
        tumult_ingest::retention::tick_from_env(),
        tumult_ingest::retention::retention_days_from_env(),
        background_shutdown.clone(),
    );
    let ops_db_path = config.db_path.clone();
    let ops_ingest = ingest.clone();
    let http_server = tokio::spawn(async move {
        // The live /report endpoint rides the API's auth middleware: it
        // renders from the same store, so it follows the same credential and
        // role rules as /api (ROUTE_TABLE gates it at Viewer). The ops
        // endpoints (/healthz, /readyz, /metrics) follow the same pattern.
        let report_auth = axum::middleware::from_fn_with_state(
            api_state.clone(),
            tumult_api::auth::auth_middleware,
        );
        let ops_auth = axum::middleware::from_fn_with_state(
            api_state.clone(),
            tumult_api::auth::auth_middleware,
        );
        let ops_state = crate::ops::OpsState {
            db_path: ops_db_path,
            ingest: ops_ingest,
        };
        let app = tumult_ingest::http::router_with_token(ingest, http_token)
            .merge(report_router(report_state).layer(report_auth))
            .merge(crate::ops::router(ops_state).layer(ops_auth))
            .merge(tumult_api::router(api_state))
            // Everything that is not /v1, /report, /healthz, /readyz, /metrics
            // or /api is the UI.
            .fallback(ui_handler);
        let result = match http_tls {
            Some(tls_config) => {
                let handle = axum_server::Handle::new();
                let shutdown = handle.clone();
                tokio::spawn(async move {
                    shutdown_signal("https").await;
                    shutdown.graceful_shutdown(Some(std::time::Duration::from_secs(10)));
                });
                let result = axum_server::from_tcp_rustls(http_listener.into_std()?, tls_config)
                    .handle(handle)
                    .serve(app.into_make_service_with_connect_info::<std::net::SocketAddr>())
                    .await;
                tracing::info!("HTTPS server future completed: {result:?}");
                result
            }
            None => {
                // ConnectInfo gives /api/auth/login the peer IP for its
                // per-ip|username throttle key.
                let result = axum::serve(
                    http_listener,
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

    let mut tasks = vec![
        ("schedule scheduler", schedule_task),
        ("webhook dispatcher", webhook_task),
        ("gameday supervisor", gameday_task),
        ("run retention", retention_task),
        ("version backfill", backfill_task),
    ];
    tasks.extend(lake_task.map(|task| ("lake scheduler", task)));
    let workers = Workers {
        shutdown: background_shutdown,
        tasks,
        run_queue,
        writer: writer_task,
    };
    // Both servers are down once this returns; their channel clones
    // (moved into the tasks) are dropped.
    let served = until_both_stop(grpc_server, http_server).await;
    finish(served, internal_grpc_server, workers, FAILED_DRAIN_LIMIT).await?;
    tracing::info!("tumultd stopped cleanly");
    Ok(())
}
