use anyhow::{Context, Result};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use tumult_ingest::{Config, IngestWriter};
use tumult_lake::Store;

use crate::admin::enforce_bind_guard;
use crate::lake_jobs::{lake_interval_from_env, spawn_lake_scheduler};
use crate::reports::{
    report_interval_from_env, report_router, spawn_report_scheduler, ReportState,
};

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
    schedule_task
        .await
        .context("schedule scheduler task panicked")?;
    webhook_task
        .await
        .context("webhook dispatcher task panicked")?;
    gameday_task
        .await
        .context("gameday supervisor task panicked")?;
    retention_task
        .await
        .context("run retention task panicked")?;
    run_queue.shutdown();
    drop(run_queue);
    writer_task.await.context("ingest writer task")?;
    tracing::info!("tumultd stopped cleanly");
    Ok(())
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

// ---------------------------------------------------------------------------
// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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

    // -- ui_handler (embedded SPA) --------------------------------------------

    async fn body_bytes(resp: Response) -> Vec<u8> {
        axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec()
    }

    #[tokio::test]
    async fn ui_handler_serves_index_html_at_the_root() {
        let resp = ui_handler(axum::http::Uri::from_static("/")).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()[axum::http::header::CONTENT_TYPE],
            "text/html"
        );
        // Non-fingerprinted files must revalidate.
        assert_eq!(
            resp.headers()[axum::http::header::CACHE_CONTROL],
            "no-cache"
        );
        let index = UiAssets::get("index.html").expect("index.html is embedded");
        assert_eq!(body_bytes(resp).await.as_slice(), &index.data[..]);
    }

    #[tokio::test]
    async fn ui_handler_caches_fingerprinted_assets_forever() {
        let path = UiAssets::iter()
            .find(|p| p.starts_with("_app/immutable/"))
            .expect("fingerprinted assets are embedded")
            .into_owned();
        let uri: axum::http::Uri = format!("/{path}").parse().unwrap();
        let resp = ui_handler(uri).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()[axum::http::header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );
        let file = UiAssets::get(&path).unwrap();
        assert_eq!(body_bytes(resp).await.as_slice(), &file.data[..]);
    }

    #[tokio::test]
    async fn ui_handler_falls_back_to_the_app_shell_for_client_routes() {
        let resp = ui_handler(axum::http::Uri::from_static("/runs/some/client/route")).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()[axum::http::header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
        let shell = UiAssets::get("200.html").expect("200.html app shell is embedded");
        assert_eq!(body_bytes(resp).await.as_slice(), &shell.data[..]);
    }

    // -- serve() end to end ----------------------------------------------------

    /// One metric definition so the live `/report` endpoint has something to
    /// render.
    const METRIC_YAML: &str = r#"
name: experiment_count
description: Count of experiment runs in the window, per experiment.
source_table: spans
time_col: ts_ns
measure:
  type: count
dimensions: [experiment_name]
condition: { column: span_name, equals: "resilience.experiment" }
"#;

    /// A currently-free loopback port (the listener is dropped before the
    /// server binds it).
    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    /// Minimal HTTP/1.0 GET against a loopback server; returns the raw
    /// response (status line, headers and body).
    async fn http_get(port: u16, path: &str) -> std::io::Result<String> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
        stream
            .write_all(format!("GET {path} HTTP/1.0\r\nHost: localhost\r\n\r\n").as_bytes())
            .await?;
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await?;
        Ok(String::from_utf8_lossy(&buf).into_owned())
    }

    /// The whole daemon in-process: bind both listeners on loopback, serve
    /// the health and live-report endpoints, then stop cleanly on SIGTERM.
    /// Holds the env lock for its whole lifetime — the configuration is
    /// process-global. Holding a std mutex guard across awaits is deliberate
    /// here: the guard only ever blocks other env-mutating tests, and this
    /// test's own progress never depends on them.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn serve_binds_serves_and_shuts_down_cleanly_on_sigterm() {
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let metrics_dir = dir.path().join("metrics");
        std::fs::create_dir_all(&metrics_dir).unwrap();
        std::fs::write(metrics_dir.join("experiment_count.yaml"), METRIC_YAML).unwrap();
        let grpc_port = free_port();
        let http_port = free_port();
        std::env::set_var("TUMULT_LAKE_PATH", dir.path().join("lake.duckdb"));
        std::env::set_var("KRONIKA_METRICS_DIR", &metrics_dir);
        std::env::set_var("KRONIKA_OTLP_GRPC_ADDR", format!("127.0.0.1:{grpc_port}"));
        std::env::set_var("KRONIKA_OTLP_HTTP_ADDR", format!("127.0.0.1:{http_port}"));
        std::env::set_var("KRONIKA_LAKE_INTERVAL", "off");
        std::env::remove_var("KRONIKA_INGEST_TOKEN");
        std::env::remove_var("KRONIKA_REPORT_INTERVAL");
        std::env::remove_var("KRONIKA_BOOTSTRAP_ADMIN_PASSWORD");
        std::env::remove_var("KRONIKA_BOOTSTRAP_TOKEN");

        let mut daemon = tokio::spawn(serve());

        // Wait for the HTTP listener, then probe the health and live report
        // endpoints while the daemon holds the store.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let health = loop {
            if let Ok(Ok(resp)) = tokio::time::timeout(
                std::time::Duration::from_millis(500),
                http_get(http_port, "/healthz"),
            )
            .await
            {
                break resp;
            }
            if daemon.is_finished() {
                let outcome = (&mut daemon).await.expect("daemon task panicked");
                panic!("daemon exited before serving: {outcome:?}");
            }
            assert!(
                std::time::Instant::now() < deadline,
                "daemon did not start serving within 30s"
            );
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        };
        assert!(health.contains(" 200 OK"), "{health}");

        // Readiness (migrations applied, supervisor ticking) and the daemon
        // metrics exposition answer on the same listener.
        let ready = http_get(http_port, "/readyz")
            .await
            .expect("readiness request failed");
        assert!(ready.contains(" 200 OK"), "{ready}");
        let metrics = http_get(http_port, "/metrics")
            .await
            .expect("metrics request failed");
        assert!(metrics.contains(" 200 OK"), "{metrics}");
        assert!(
            metrics.contains("tumultd_runs_started_total"),
            "metrics endpoint did not expose daemon SLIs"
        );

        let report = http_get(http_port, "/report?metric=experiment_count")
            .await
            .expect("live report request failed");
        assert!(report.contains(" 200 OK"), "{report}");
        assert!(
            report.contains("Tumult — experiment_count"),
            "live report did not render the metric"
        );

        // SIGTERM to ourselves: the daemon's shutdown handler must drive a
        // clean stop (servers, lake task, writer drain) and return Ok.
        let status = std::process::Command::new("kill")
            .arg("-TERM")
            .arg(std::process::id().to_string())
            .status()
            .unwrap();
        assert!(status.success());
        let result = tokio::time::timeout(std::time::Duration::from_secs(30), daemon)
            .await
            .expect("daemon did not stop within 30s of SIGTERM")
            .expect("daemon task panicked");
        result.expect("daemon returned an error");

        std::env::remove_var("TUMULT_LAKE_PATH");
        std::env::remove_var("KRONIKA_METRICS_DIR");
        std::env::remove_var("KRONIKA_OTLP_GRPC_ADDR");
        std::env::remove_var("KRONIKA_OTLP_HTTP_ADDR");
        std::env::remove_var("KRONIKA_LAKE_INTERVAL");
    }
}
