// Imported from kronika. Pedantic lints are scoped to tumult-native
// crates; this file predates the pedantic gate (see crate lib.rs).
#![allow(clippy::pedantic)]

//! Integration test: with an ingest token configured, `/v1/*` requires
//! `Authorization: Bearer <token>` (401 otherwise) while `/healthz` stays
//! open; with no token the routes stay open.

use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message;
use tumult_ingest::{http, IngestWriter};
use tumult_lake::Store;

/// Keep-alive handles for a running test server: the caller holds these for
/// the duration of the test so the store and writer task stay alive.
struct TestServer {
    base: String,
    server: tokio::task::JoinHandle<()>,
    _tmp: tempfile::TempDir,
    _writer_task: tokio::task::JoinHandle<()>,
}

async fn serve(token: Option<String>) -> TestServer {
    let tmp = tempfile::TempDir::new().unwrap();
    let store = Store::open(&tmp.path().join("kronika.duckdb")).unwrap();
    let writer = store.writer().unwrap();
    let (ingest, writer_task) = IngestWriter::spawn(writer, 16);

    let app = http::router_with_token(ingest, token);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    TestServer {
        base: format!("http://{addr}"),
        server,
        _tmp: tmp,
        _writer_task: writer_task,
    }
}

fn traces_body() -> Vec<u8> {
    ExportTraceServiceRequest {
        resource_spans: vec![],
    }
    .encode_to_vec()
}

#[tokio::test]
async fn v1_routes_require_bearer_when_token_configured() {
    let srv = serve(Some("kro_secret".into())).await;
    let client = reqwest::Client::new();

    // No header → 401 with the JSON error body.
    let response = client
        .post(format!("{}/v1/traces", srv.base))
        .header("content-type", "application/x-protobuf")
        .body(traces_body())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["error"], "unauthorized");

    // Wrong token → 401.
    let response = client
        .post(format!("{}/v1/traces", srv.base))
        .header("authorization", "Bearer kro_wrong")
        .header("content-type", "application/x-protobuf")
        .body(traces_body())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);

    // Correct token → 200.
    let response = client
        .post(format!("{}/v1/traces", srv.base))
        .header("authorization", "Bearer kro_secret")
        .header("content-type", "application/x-protobuf")
        .body(traces_body())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    // /healthz stays open without a token.
    let health = client
        .get(format!("{}/healthz", srv.base))
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), 200);

    srv.server.abort();
}

#[tokio::test]
async fn v1_routes_stay_open_without_token() {
    let srv = serve(None).await;
    let response = reqwest::Client::new()
        .post(format!("{}/v1/traces", srv.base))
        .header("content-type", "application/x-protobuf")
        .body(traces_body())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    srv.server.abort();
}
