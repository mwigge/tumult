// Imported from kronika (Apache-2.0, same author). Pedantic lints are
// scoped to tumult-native crates: this crate predates the pedantic gate and
// carries intentional patterns it flags (timestamp/score casts, f64
// comparisons). CI still applies -D warnings to it.
#![allow(clippy::pedantic)]

//! `tumultd` — the kronika daemon.
//!
//! Default command (`serve`): open the store, spawn the single-writer
//! channel, and run the OTLP/gRPC (`:4317`) and OTLP/HTTP (`:4318`) ingest
//! servers until SIGTERM/SIGINT. The HTTP server also exposes the ops
//! endpoints — `GET /healthz` (liveness: writer channel + store probe),
//! `GET /readyz` (readiness: migrations applied, supervisor ticking) and
//! `GET /metrics` (daemon SLIs, Prometheus text) — plus
//! `GET /report?metric=<name>`, which renders a metric report from
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

mod admin;
mod lake_jobs;
mod ops;
mod reports;
mod serve;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

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
        Command::Serve => serve::serve().await,
        Command::Import { file, label } => admin::import(file, label),
        Command::Report { metric, out } => reports::report(metric, out),
        Command::CreateAdmin { username, db } => admin::create_admin(&username, db),
    }
}

/// Shared test support: environment variables are process-global, so every
/// test that sets or removes one (across all modules of this binary) must
/// hold this lock to keep those tests sequential.
#[cfg(test)]
pub(crate) mod test_support {
    pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
}
