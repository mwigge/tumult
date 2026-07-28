//! `kronika-demo` — synthetic chaos-experiment generator that speaks real
//! OTLP to a kronikad endpoint (exercising the true ingest path, not direct
//! store writes). Used by the docker demo stack and for local seeding.

mod gen;

use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use opentelemetry_proto::tonic::collector::logs::v1::logs_service_client::LogsServiceClient;
use opentelemetry_proto::tonic::collector::metrics::v1::metrics_service_client::MetricsServiceClient;
use opentelemetry_proto::tonic::collector::trace::v1::trace_service_client::TraceServiceClient;
use tonic::transport::Channel;

use gen::{DemoData, XorShift};

/// Synthetic chaos-experiment generator for kronika demos.
#[derive(Parser)]
#[command(name = "kronika-demo", version, about)]
struct Cli {
    /// OTLP/gRPC endpoint of the kronikad to seed.
    #[arg(long, default_value = "http://localhost:4317")]
    endpoint: String,

    /// Number of experiments to generate (per batch in --loop mode: N/10).
    #[arg(long, default_value_t = 30)]
    experiments: usize,

    /// RNG seed (deterministic; same seed → same demo data).
    #[arg(long, default_value_t = 42)]
    seed: u64,

    /// Keep generating small batches forever (timestamps at generation time).
    #[arg(long = "loop")]
    loop_mode: bool,

    /// Seconds between batches in --loop mode.
    #[arg(long, default_value_t = 60)]
    interval_secs: u64,
}

fn now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

/// Connect with ~30s of retries so `depends_on` in compose stays simple.
/// Exits non-zero when the endpoint never comes up.
async fn connect(endpoint: &str) -> Result<Channel> {
    let channel = Channel::from_shared(endpoint.to_string())
        .with_context(|| format!("invalid endpoint {endpoint:?}"))?;
    let mut attempt = 1u32;
    loop {
        match channel.connect().await {
            Ok(conn) => return Ok(conn),
            Err(e) if attempt >= 30 => {
                anyhow::bail!("endpoint {endpoint} unreachable after {attempt} attempts: {e}");
            }
            Err(e) => {
                eprintln!("waiting for {endpoint} (attempt {attempt}/30): {e}");
                tokio::time::sleep(Duration::from_secs(1)).await;
                attempt += 1;
            }
        }
    }
}

async fn send_batch(channel: &Channel, data: DemoData, label: &str) -> Result<()> {
    let spans = data.traces.resource_spans[0].scope_spans[0].spans.len();
    let mut traces = TraceServiceClient::new(channel.clone());
    traces.export(data.traces).await.context("export traces")?;
    let mut metrics = MetricsServiceClient::new(channel.clone());
    metrics
        .export(data.metrics)
        .await
        .context("export metrics")?;
    let records = data.logs.resource_logs[0].scope_logs[0].log_records.len();
    let mut logs = LogsServiceClient::new(channel.clone());
    logs.export(data.logs).await.context("export logs")?;
    println!(
        "{label}: sent {} experiments ({spans} spans, {records} log records)",
        data.experiments
    );
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let channel = connect(&cli.endpoint).await?;
    let mut rng = XorShift::new(cli.seed);

    if cli.loop_mode {
        let batch_size = (cli.experiments / 10).max(1);
        println!(
            "loop mode: {batch_size} experiments every {}s → {}",
            cli.interval_secs, cli.endpoint
        );
        let mut batch = 0u64;
        loop {
            batch += 1;
            let data = gen::generate(&mut rng, batch_size, now_ns(), false);
            send_batch(&channel, data, &format!("batch {batch}")).await?;
            tokio::time::sleep(Duration::from_secs(cli.interval_secs)).await;
        }
    } else {
        let data = gen::generate(&mut rng, cli.experiments, now_ns(), true);
        send_batch(&channel, data, "seed").await?;
        println!("done (seed={}, spread over the past 14 days)", cli.seed);
    }
    Ok(())
}
