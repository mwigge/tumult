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

/// Regression for the epoch-0 log timestamp bug: tumult's log records arrive
/// with `time_unix_nano` unset. A record with a real 2026 timestamp must keep
/// it through ingest → store; a record with no timestamp must get kronika's
/// receipt time (never 0/1970).
#[tokio::test]
async fn http_post_logs_preserves_or_assigns_timestamps() {
    use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
    use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};

    const REAL_2026: u64 = 1_785_268_000_000_000_000;
    let request = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: vec![kv("service.name", Value::StringValue("tumult".into()))],
                dropped_attributes_count: 0,
                entity_refs: vec![],
            }),
            scope_logs: vec![ScopeLogs {
                scope: None,
                log_records: vec![
                    LogRecord {
                        time_unix_nano: REAL_2026,
                        severity_text: "INFO".into(),
                        body: Some(AnyValue {
                            value: Some(Value::StringValue("experiment.started".into())),
                        }),
                        ..LogRecord::default()
                    },
                    LogRecord {
                        // tumult's shape: neither time nor observed time set.
                        severity_text: "INFO".into(),
                        body: Some(AnyValue {
                            value: Some(Value::StringValue("experiment.completed".into())),
                        }),
                        ..LogRecord::default()
                    },
                ],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };

    let tmp = tempfile::TempDir::new().unwrap();
    let store = Store::open(&tmp.path().join("kronika.duckdb")).unwrap();
    let writer = store.writer().unwrap();
    let (ingest, writer_task) = IngestWriter::spawn(writer, 16);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, http::router(ingest)).await.unwrap();
    });

    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as i64;
    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/logs"))
        .header("content-type", "application/x-protobuf")
        .body(request.encode_to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    // The write travels a channel; give the writer task a beat to land it.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let reader = store.read_only().unwrap();
    let rows = reader
        .query_json_rows("SELECT ts_ns, body FROM logs ORDER BY ts_ns")
        .unwrap();
    assert_eq!(rows.len(), 2);
    // Real 2026 timestamp survives verbatim.
    assert_eq!(
        rows[0]["ts_ns"],
        serde_json::json!(REAL_2026),
        "real timestamp must survive ingest"
    );
    // Untimestamped record got a receipt time, not epoch 0.
    let assigned = rows[1]["ts_ns"].as_i64().unwrap();
    assert!(
        assigned >= before,
        "expected receipt time >= {before}, got {assigned}"
    );

    server.abort();
    drop(writer_task);
}
