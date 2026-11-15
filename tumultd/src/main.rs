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
//! `0`/`off` disables): portable manifest-selected Parquet snapshots in
//! `KRONIKA_LAKE_DIR` (default `<db dir>/lake`). Automatic retention must
//! remain zero until reports can read cold history. `POST /api/lake/export`
//! triggers the same snapshot job on demand. Operational backups use the
//! separate `tumult store backup` / `restore` commands.
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
mod events;
mod lake_jobs;
mod ops;
mod reports;
mod score_jobs;
mod serve;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tumult_otel::logging;

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

/// Every subcommand logs through the shared structured pipeline: JSON on
/// stderr unless a terminal or `TUMULT_LOG_FORMAT` says otherwise. stdout
/// carries only command output (reports, the one-time admin password).
fn main() -> ExitCode {
    let cli = Cli::parse();
    let service_name = events::service_name();
    if let Err(error) = logging::init(events::log_options(&service_name)) {
        logging::report_init_failure(&service_name, &error);
        return ExitCode::FAILURE;
    }
    // A returned error would be printed unstructured by the runtime; log it
    // as an event instead.
    let outcome = run_to_exit(run(cli), RUNTIME_SHUTDOWN_GRACE)
        .context("start the async runtime")
        .and_then(std::convert::identity);
    if let Err(error) = outcome {
        events::fatal(&error);
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// How long the process waits, once its work is done, for blocking tasks
/// that are still running.
const RUNTIME_SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// Runs `work` on a runtime of its own and leaves that runtime within
/// `grace`. Dropping a runtime waits for its blocking tasks without limit,
/// and one of them can be a telemetry flush against an exporter that hangs:
/// the daemon would have stopped serving and never exit.
fn run_to_exit<F: std::future::Future>(
    work: F,
    grace: std::time::Duration,
) -> std::io::Result<F::Output> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let outcome = runtime.block_on(work);
    runtime.shutdown_timeout(grace);
    Ok(outcome)
}

async fn run(cli: Cli) -> Result<()> {
    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => serve::serve().await,
        Command::Import { file, label } => admin::import(file, label),
        Command::Report { metric, out } => reports::report(metric, out),
        Command::CreateAdmin { username, db } => admin::create_admin(&username, db),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    /// A telemetry flush that hangs was given up on (`flush_within`), but
    /// its thread is still there when the daemon is done.
    #[test]
    fn a_blocking_task_that_hangs_does_not_hold_up_the_exit() {
        let (release, held) = std::sync::mpsc::channel::<()>();
        let work = async move {
            tokio::task::spawn_blocking(move || {
                // Ends when the sender is dropped.
                let _ = held.recv();
            });
            7
        };

        let started = Instant::now();
        let outcome = super::run_to_exit(work, Duration::from_millis(50)).unwrap();

        assert_eq!(outcome, 7);
        assert!(started.elapsed() < Duration::from_secs(5));
        drop(release);
    }
}
