//! demo-app — the fault-injection target for the Tumult one-command demo.
//!
//! A small orders API over Postgres with OpenTelemetry tracing. Designed to
//! keep serving (degraded, never panicking) while Tumult injects faults into
//! its database and network: when the DB is unreachable `/health` flips to
//! 503 and the data endpoints return 5xx with an error span status, which is
//! exactly what the `SigNoz` dashboards visualize.
//!
//! Baseline load is supplied by the separate `demo-traffic` service (see
//! `demo/traffic`), so this binary contains no self-traffic generator.
//!
//! Environment (defaults match the demo CONTRACT):
//! - `DATABASE_URL`                 (default `postgres://demo:demo@demo-postgres:5432/orders`)
//! - `OTEL_EXPORTER_OTLP_ENDPOINT`  (default `http://tumult-collector:4318`, OTLP HTTP)
//! - `LISTEN_ADDR`                  (default `0.0.0.0:8080`)

mod app;
mod db;
mod telemetry;

use std::sync::Arc;
use std::time::Duration;

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_owned())
}

/// Retry schema bootstrap forever with capped exponential backoff — the demo
/// stack starts in parallel, so the database may not be up yet. The HTTP
/// server serves the whole time (reporting 503 on `/health` until the DB is
/// reachable).
fn spawn_bootstrap(store: Arc<db::PgStore>) {
    tokio::spawn(async move {
        let mut delay = Duration::from_secs(1);
        loop {
            match store.bootstrap().await {
                Ok(()) => {
                    tracing::info!("database schema ready");
                    break;
                }
                Err(e) => {
                    tracing::warn!(error = %e, retry_in = ?delay, "database bootstrap failed; retrying");
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(Duration::from_secs(15));
                }
            }
        }
    });
}

async fn shutdown_signal() {
    if let Err(e) = tokio::signal::ctrl_c().await {
        tracing::error!(error = %e, "failed to listen for shutdown signal");
    }
    tracing::info!("shutdown signal received");
}

#[tokio::main]
async fn main() {
    let database_url = env_or(
        "DATABASE_URL",
        "postgres://demo:demo@demo-postgres:5432/orders",
    );
    let otlp_endpoint = env_or("OTEL_EXPORTER_OTLP_ENDPOINT", "http://tumult-collector:4318");
    let listen_addr = env_or("LISTEN_ADDR", "0.0.0.0:8080");

    let tracer_provider = telemetry::init(&otlp_endpoint);

    let store = match db::PgStore::connect(&database_url) {
        Ok(store) => Arc::new(store),
        Err(e) => {
            tracing::error!(error = %e, "invalid DATABASE_URL");
            std::process::exit(1);
        }
    };
    spawn_bootstrap(Arc::clone(&store));

    let router = app::router(app::AppState { store });

    let listener = match tokio::net::TcpListener::bind(&listen_addr).await {
        Ok(listener) => listener,
        Err(e) => {
            tracing::error!(error = %e, %listen_addr, "failed to bind");
            std::process::exit(1);
        }
    };
    tracing::info!(%listen_addr, "demo-app listening");

    if let Err(e) = axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
    {
        tracing::error!(error = %e, "server error");
    }

    if let Some(provider) = tracer_provider {
        if let Err(e) = provider.shutdown() {
            tracing::warn!(error = %e, "tracer provider shutdown failed");
        }
    }
}
