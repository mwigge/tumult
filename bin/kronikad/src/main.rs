//! `kronikad` — the kronika daemon.
//!
//! Default command (`serve`): open the store, spawn the single-writer
//! channel, and run the OTLP/gRPC (`:4317`) and OTLP/HTTP (`:4318`, with
//! `GET /healthz`) ingest servers until SIGTERM/SIGINT.
//!
//! Subcommands:
//! * `kronikad import <file>` — manual CSV / tumult journal JSON import.
//! * `kronikad report --metric <name>` — print an HTML report to stdout.

use std::path::PathBuf;

use anyhow::{Context, Result};
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
    /// Print an HTML report for a semantic metric to stdout.
    Report {
        /// Metric name from the metrics directory (e.g. hypothesis_pass_rate).
        #[arg(long)]
        metric: String,
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
        Command::Report { metric } => report(metric),
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

    // OTLP/HTTP server (smedja exporter target) + health endpoint.
    let http_addr = config.otlp_http_addr;
    let http_server = tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(http_addr).await?;
        let result = axum::serve(listener, kronika_ingest::http::router(ingest))
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

fn report(metric: String) -> Result<()> {
    let config = Config::from_env().map_err(anyhow::Error::msg)?;
    let defs = kronika_metrics::load_dir(&config.metrics_dir)
        .with_context(|| format!("load metrics from {}", config.metrics_dir.display()))?;
    let def = defs.iter().find(|d| d.name == metric).with_context(|| {
        format!(
            "metric {metric:?} not found; available: {}",
            defs.iter()
                .map(|d| d.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;

    // Read-only handle: does not collide with a running daemon's writer.
    let store = Store::at(&config.db_path);
    let reader = store.read_only().context("open store read-only")?;
    let report = kronika_report::build_report(
        &reader,
        std::slice::from_ref(def),
        &format!("kronika — {metric}"),
        None,
    )?;
    print!("{}", kronika_report::render_html(&report));
    Ok(())
}
