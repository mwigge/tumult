//! Integration test: POST a protobuf `ExportTraceServiceRequest` to the
//! OTLP/HTTP server on an ephemeral port and assert the row lands in the store.

use kronika_ingest::{http, IngestWriter};
use kronika_store::Store;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
use prost::Message;

fn kv(key: &str, value: Value) -> KeyValue {
    KeyValue {
        key: key.into(),
        value: Some(AnyValue { value: Some(value) }),
        key_strindex: 0,
    }
}

fn sample_request() -> ExportTraceServiceRequest {
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(Resource {
                attributes: vec![kv("service.name", Value::StringValue("tumult".into()))],
                dropped_attributes_count: 0,
                entity_refs: vec![],
            }),
            scope_spans: vec![ScopeSpans {
                scope: None,
                spans: vec![Span {
                    trace_id: vec![0xab; 16],
                    span_id: vec![0xcd; 8],
                    name: "resilience.experiment".into(),
                    kind: 1,
                    start_time_unix_nano: 1_774_980_000_000_000_000,
                    end_time_unix_nano: 1_774_980_060_000_000_000,
                    attributes: vec![
                        kv(
                            "resilience.experiment.id",
                            Value::StringValue("exp-http-1".into()),
                        ),
                        kv(
                            "resilience.experiment.name",
                            Value::StringValue("http-ingest".into()),
                        ),
                        kv(
                            "resilience.outcome.status",
                            Value::StringValue("completed".into()),
                        ),
                    ],
                    ..Span::default()
                }],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    }
}

#[tokio::test]
async fn http_post_protobuf_traces_lands_in_store() {
    let tmp = tempfile::TempDir::new().unwrap();
    let store = Store::open(&tmp.path().join("kronika.duckdb")).unwrap();
    let writer = store.writer().unwrap();
    let (ingest, writer_task) = IngestWriter::spawn(writer, 16);

    let app = http::router(ingest);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let response = client
        .post(format!("http://{addr}/v1/traces"))
        .header("content-type", "application/x-protobuf")
        .body(sample_request().encode_to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    // healthz answers too.
    let health = client
        .get(format!("http://{addr}/healthz"))
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), 200);

    let reader = store.read_only().unwrap();
    let runs = reader.experiment_runs().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].experiment_id.as_deref(), Some("exp-http-1"));
    assert_eq!(runs[0].experiment_name.as_deref(), Some("http-ingest"));

    server.abort();
    drop(writer_task);
}
