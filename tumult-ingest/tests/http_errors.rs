// Imported from kronika. Pedantic lints are scoped to tumult-native
// crates; this file predates the pedantic gate (see crate lib.rs).
#![allow(clippy::pedantic)]

//! Error-path tests for the OTLP/HTTP endpoints: malformed protobuf bodies
//! are 400s, a dead writer channel is a 500, and the metrics endpoint
//! round-trips an empty export request.

use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message;
use tumult_ingest::{http, IngestWriter};
use tumult_lake::Store;

async fn serve(ingest: IngestWriter) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, http::router(ingest)).await.unwrap();
    });
    (format!("http://{addr}"), server)
}

#[tokio::test]
async fn malformed_protobuf_is_a_client_error_on_every_v1_route() {
    let tmp = tempfile::TempDir::new().unwrap();
    let store = Store::open(&tmp.path().join("kronika.duckdb")).unwrap();
    let (ingest, _task) = IngestWriter::spawn(store.writer().unwrap(), 4);
    let (base, server) = serve(ingest).await;

    let client = reqwest::Client::new();
    for route in ["/v1/traces", "/v1/metrics", "/v1/logs"] {
        let response = client
            .post(format!("{base}{route}"))
            .header("content-type", "application/x-protobuf")
            .body(vec![0xff, 0xff, 0xff, 0x0f])
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{route}");
        let body = response.text().await.unwrap();
        assert!(body.contains("decode"), "{route}: {body}");
    }
    server.abort();
}

#[tokio::test]
async fn metrics_export_round_trips() {
    let tmp = tempfile::TempDir::new().unwrap();
    let store = Store::open(&tmp.path().join("kronika.duckdb")).unwrap();
    let (ingest, _task) = IngestWriter::spawn(store.writer().unwrap(), 4);
    let (base, server) = serve(ingest).await;

    let response = reqwest::Client::new()
        .post(format!("{base}/v1/metrics"))
        .header("content-type", "application/x-protobuf")
        .body(ExportMetricsServiceRequest::default().encode_to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body = response.text().await.unwrap();
    assert_eq!(body, "0 data points ingested");
    server.abort();
}

#[tokio::test]
async fn dead_writer_channel_is_a_server_error_not_a_hang() {
    let tmp = tempfile::TempDir::new().unwrap();
    let store = Store::open(&tmp.path().join("kronika.duckdb")).unwrap();
    let (ingest, writer_task) = IngestWriter::spawn(store.writer().unwrap(), 4);
    let (base, server) = serve(ingest).await;

    // Kill the writer task and wait until it is really gone (the channel
    // receiver drops with it), so the next export cannot even be queued.
    writer_task.abort();
    let _ = writer_task.await;

    let response = reqwest::Client::new()
        .post(format!("{base}/v1/traces"))
        .header("content-type", "application/x-protobuf")
        .body(ExportTraceServiceRequest::default().encode_to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 500);
    let body = response.text().await.unwrap();
    assert!(body.contains("writer task stopped"), "{body}");
    server.abort();
}
