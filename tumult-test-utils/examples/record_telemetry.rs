//! Records the telemetry and the journals the tests keep in `recorded`.
//!
//! The `tumult` binary runs each experiment of `recorded/experiments` on a
//! lab of its own: a directory with a state file that the steps of the
//! experiment read and write with `sh`. The binary sends its traces, metrics
//! and logs to this tool over OTLP, as it sends them to the daemon or to a
//! collector, and this tool writes each request as one line of JSON.
//!
//! ```text
//! cargo build -p tumult-cli
//! cargo run -p tumult-test-utils --features recorded --example record_telemetry -- target/debug/tumult
//! ```
//!
//! A new recording has new ids and new times, so the reports kept with the
//! tests change with it: run those tests with `TUMULT_BLESS=1` and read the
//! difference before you commit it.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use opentelemetry_proto::tonic::collector::logs::v1::logs_service_server::{
    LogsService, LogsServiceServer,
};
use opentelemetry_proto::tonic::collector::logs::v1::{
    ExportLogsServiceRequest, ExportLogsServiceResponse,
};
use opentelemetry_proto::tonic::collector::metrics::v1::metrics_service_server::{
    MetricsService, MetricsServiceServer,
};
use opentelemetry_proto::tonic::collector::metrics::v1::{
    ExportMetricsServiceRequest, ExportMetricsServiceResponse,
};
use opentelemetry_proto::tonic::collector::trace::v1::trace_service_server::{
    TraceService, TraceServiceServer,
};
use opentelemetry_proto::tonic::collector::trace::v1::{
    ExportTraceServiceRequest, ExportTraceServiceResponse,
};
use tonic::transport::server::TcpIncoming;
use tonic::{Request, Response, Status};
use tumult_core::types::ExperimentStatus;
use tumult_test_utils::recorded;

type Failure = Box<dyn std::error::Error + Send + Sync>;

/// One run of the recording.
struct Run {
    /// The experiment in `recorded/experiments`, without `.toon`.
    experiment: &'static str,
    /// Files the lab has before the run, next to its state file.
    lab_files: &'static [&'static str],
    /// Whether the run sends telemetry. A run that does not leaves only its
    /// journal, as a run on a machine without a collector does.
    telemetry: bool,
    /// How the run must end for the recording to be the one the tests expect.
    ends: ExperimentStatus,
}

/// The runs, in their order: a run that holds, a run that finds a weakness,
/// a weakness that is found and then fixed (the search gets a replica), and
/// a run that sent no telemetry.
const RUNS: &[Run] = &[
    Run {
        experiment: "checkout-cache-restart",
        lab_files: &[],
        telemetry: true,
        ends: ExperimentStatus::Completed,
    },
    Run {
        experiment: "payments-slow-ledger",
        lab_files: &[],
        telemetry: true,
        ends: ExperimentStatus::Deviated,
    },
    Run {
        experiment: "search-node-loss",
        lab_files: &[],
        telemetry: true,
        ends: ExperimentStatus::Deviated,
    },
    Run {
        experiment: "search-node-loss",
        lab_files: &["replica"],
        telemetry: true,
        ends: ExperimentStatus::Completed,
    },
    Run {
        experiment: "billing-export-restart",
        lab_files: &[],
        telemetry: false,
        ends: ExperimentStatus::Completed,
    },
];

/// The requests the binary sent, in the order they came in.
#[derive(Clone, Default)]
struct Received {
    traces: Arc<Mutex<Vec<ExportTraceServiceRequest>>>,
    metrics: Arc<Mutex<Vec<ExportMetricsServiceRequest>>>,
    logs: Arc<Mutex<Vec<ExportLogsServiceRequest>>>,
}

/// Keep `request` in `kept`.
fn keep<T>(kept: &Mutex<Vec<T>>, request: T) -> Result<(), Status> {
    let mut kept = kept
        .lock()
        .map_err(|_| Status::internal("the recorder failed"))?;
    kept.push(request);
    Ok(())
}

#[tonic::async_trait]
impl TraceService for Received {
    async fn export(
        &self,
        request: Request<ExportTraceServiceRequest>,
    ) -> Result<Response<ExportTraceServiceResponse>, Status> {
        keep(&self.traces, request.into_inner())?;
        Ok(Response::new(ExportTraceServiceResponse {
            partial_success: None,
        }))
    }
}

#[tonic::async_trait]
impl MetricsService for Received {
    async fn export(
        &self,
        request: Request<ExportMetricsServiceRequest>,
    ) -> Result<Response<ExportMetricsServiceResponse>, Status> {
        keep(&self.metrics, request.into_inner())?;
        Ok(Response::new(ExportMetricsServiceResponse {
            partial_success: None,
        }))
    }
}

#[tonic::async_trait]
impl LogsService for Received {
    async fn export(
        &self,
        request: Request<ExportLogsServiceRequest>,
    ) -> Result<Response<ExportLogsServiceResponse>, Status> {
        keep(&self.logs, request.into_inner())?;
        Ok(Response::new(ExportLogsServiceResponse {
            partial_success: None,
        }))
    }
}

/// Run `run` with the binary `tumult` on a lab of its own and keep its
/// journal as run `number` of the recording.
fn record(tumult: &Path, endpoint: &str, number: usize, run: &Run) -> Result<(), Failure> {
    let lab = tempfile::tempdir()?;
    std::fs::write(lab.path().join("state"), "ok\n")?;
    for file in run.lab_files {
        std::fs::write(lab.path().join(file), "")?;
    }
    let definition = recorded::dir()
        .join(recorded::EXPERIMENTS)
        .join(format!("{}.toon", run.experiment));
    std::fs::copy(&definition, lab.path().join("experiment.toon"))?;

    let mut command = Command::new(tumult);
    command
        .args([
            "run",
            "experiment.toon",
            "--journal-path",
            "journal.toon",
            "--no-ingest",
        ])
        .current_dir(lab.path())
        .env("HOME", lab.path())
        .env("TUMULT_LAKE_PATH", lab.path().join("lake.duckdb"))
        // The run logs who started it; the recording names nobody.
        .env("USER", "operator")
        .env_remove("LOGNAME")
        .env_remove("RUST_LOG")
        .env_remove("TUMULT_LOG_FORMAT")
        .env_remove("TUMULT_ENVIRONMENT")
        .env_remove("TUMULT_DAEMON_URL")
        .env_remove("OTEL_SERVICE_NAME")
        .env_remove("OTEL_RESOURCE_ATTRIBUTES")
        .env_remove("OTEL_EXPORTER_OTLP_HEADERS")
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT");
    if run.telemetry {
        command.env("OTEL_EXPORTER_OTLP_ENDPOINT", endpoint);
    }
    // A run that deviates ends with a failure code; the journal says how
    // the run ended.
    let output = command.output()?;
    let journal =
        tumult_core::journal::read_journal(&lab.path().join("journal.toon")).map_err(|e| {
            format!(
                "{}: no journal ({e}): {}",
                run.experiment,
                String::from_utf8_lossy(&output.stderr)
            )
        })?;
    if journal.status != run.ends {
        return Err(format!(
            "{}: the run ended {} and must end {}",
            run.experiment, journal.status, run.ends
        )
        .into());
    }
    let kept_at = recorded::dir()
        .join(recorded::JOURNALS)
        .join(recorded::journal_file(number, run.experiment));
    std::fs::copy(lab.path().join("journal.toon"), kept_at)?;
    Ok(())
}

/// Run every experiment of the recording, one after the other.
fn record_all(tumult: &Path, endpoint: &str) -> Result<(), Failure> {
    std::fs::create_dir_all(recorded::dir().join(recorded::JOURNALS))?;
    for (index, run) in RUNS.iter().enumerate() {
        record(tumult, endpoint, index + 1, run)?;
    }
    Ok(())
}

/// Write `requests` to the file `name` of the recording, one a line.
fn write_lines(name: &str, requests: Vec<serde_json::Result<String>>) -> Result<(), Failure> {
    let lines = requests.into_iter().collect::<Result<Vec<String>, _>>()?;
    let mut text = lines.join("\n");
    text.push('\n');
    std::fs::write(recorded::dir().join(name), text)?;
    Ok(())
}

/// The requests in `kept`, each as one line of JSON.
fn json_lines<T: serde::Serialize>(kept: &Mutex<Vec<T>>) -> Vec<serde_json::Result<String>> {
    let kept = kept
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    kept.iter().map(serde_json::to_string).collect()
}

#[tokio::main]
async fn main() -> Result<(), Failure> {
    let tumult: PathBuf = std::env::args_os()
        .nth(1)
        .ok_or("usage: record_telemetry <the tumult binary>")?
        .into();
    let tumult = std::fs::canonicalize(tumult)?;

    let received = Received::default();
    let incoming = TcpIncoming::bind(([127, 0, 0, 1], 0).into())?;
    let endpoint = format!("http://{}", incoming.local_addr()?);
    let server = tonic::transport::Server::builder()
        .add_service(TraceServiceServer::new(received.clone()))
        .add_service(MetricsServiceServer::new(received.clone()))
        .add_service(LogsServiceServer::new(received.clone()))
        .serve_with_incoming(incoming);
    let server = tokio::spawn(server);

    // Each run sends what it has before it ends, so when the last run is
    // over the recording is complete.
    tokio::task::spawn_blocking(move || record_all(&tumult, &endpoint)).await??;
    server.abort();

    write_lines(recorded::TRACES, json_lines(&received.traces))?;
    write_lines(recorded::METRICS, json_lines(&received.metrics))?;
    write_lines(recorded::LOGS, json_lines(&received.logs))?;
    Ok(())
}
