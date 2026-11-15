//! OTAP round-trip tests: OTLP fixtures encoded into OTel-Arrow by the test
//! encoder, decoded by the receiver and compared row for row with the OTLP
//! path, plus a gRPC-level stream test against the daemon's router.

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{
    AnyValue, ArrayValue, InstrumentationScope, KeyValue, KeyValueList,
};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::metrics::v1::{
    metric::Data, number_data_point, ExponentialHistogram, ExponentialHistogramDataPoint, Gauge,
    Histogram, HistogramDataPoint, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics, Sum,
    Summary, SummaryDataPoint,
};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::span::Event;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span, Status};
use tumult_lake::SpanRow;

use super::proto::arrow_metrics_service_client::ArrowMetricsServiceClient;
use super::proto::arrow_traces_service_client::ArrowTracesServiceClient;
use super::proto::{ArrowPayload, BatchArrowRecords, StatusCode};
use super::test_encoder::Encoder;
use super::{to_logs_request, to_metrics_request, to_trace_request, OtapError, StreamConsumer};
use crate::writer::IngestWriter;

fn kv(key: &str, value: Value) -> KeyValue {
    KeyValue {
        key: key.into(),
        value: Some(AnyValue { value: Some(value) }),
        ..Default::default()
    }
}

fn s(v: &str) -> Value {
    Value::StringValue(v.into())
}

fn resource(attrs: Vec<KeyValue>) -> Option<Resource> {
    Some(Resource {
        attributes: attrs,
        dropped_attributes_count: 0,
        ..Default::default()
    })
}

fn scope(name: &str, version: &str, attrs: Vec<KeyValue>) -> Option<InstrumentationScope> {
    Some(InstrumentationScope {
        name: name.into(),
        version: version.into(),
        attributes: attrs,
        dropped_attributes_count: 0,
    })
}

/// Two resources, two scopes, three spans: promoted attributes on spans and
/// resources, every attribute value type, events with attributes, both
/// status shapes.
pub(super) fn traces_fixture() -> ExportTraceServiceRequest {
    let experiment = Span {
        trace_id: vec![0xab; 16],
        span_id: vec![0x01; 8],
        name: "resilience.experiment".into(),
        kind: 1,
        start_time_unix_nano: 1_774_980_000_000_000_000,
        end_time_unix_nano: 1_774_980_300_000_000_000,
        attributes: vec![
            kv("resilience.experiment.id", s("exp-otap-1")),
            kv("resilience.experiment.title", s("pg-failover")),
            kv("resilience.outcome.status", s("completed")),
            kv("resilience.outcome.hypothesis_met", Value::BoolValue(true)),
            kv(
                "resilience.outcome.recovery_time_s",
                Value::DoubleValue(12.5),
            ),
            kv("resilience.fault.type", s("termination")),
            kv("resilience.fault.plugin", s("kubernetes")),
            kv("resilience.target.system", s("database")),
            kv(
                "resilience.estimate.rationale",
                s("primary restart is fast"),
            ),
            kv("retries", Value::IntValue(3)),
        ],
        events: vec![
            Event {
                time_unix_nano: 1_774_980_010_000_000_000,
                name: "fault.injected".into(),
                attributes: vec![kv("action", s("pod-kill")), kv("count", Value::IntValue(2))],
                dropped_attributes_count: 0,
            },
            Event {
                time_unix_nano: 1_774_980_020_000_000_000,
                name: "audit".into(),
                attributes: vec![kv("actor", s("morgan"))],
                dropped_attributes_count: 0,
            },
        ],
        status: Some(Status {
            code: 1,
            message: String::new(),
        }),
        ..Span::default()
    };
    let action = Span {
        trace_id: vec![0xab; 16],
        span_id: vec![0x02; 8],
        parent_span_id: vec![0x01; 8],
        name: "resilience.action".into(),
        kind: 3,
        start_time_unix_nano: 1_774_980_001_000_000_000,
        end_time_unix_nano: 1_774_980_002_000_000_000,
        attributes: vec![
            kv("resilience.fault.severity", s("high")),
            kv("payload", Value::BytesValue(vec![1, 2, 3])),
            kv(
                "targets",
                Value::ArrayValue(ArrayValue {
                    values: vec![
                        AnyValue {
                            value: Some(s("pg-0")),
                        },
                        AnyValue {
                            value: Some(Value::IntValue(7)),
                        },
                    ],
                }),
            ),
            kv(
                "labels",
                Value::KvlistValue(KeyValueList {
                    values: vec![kv("tier", s("db")), kv("replicas", Value::IntValue(2))],
                }),
            ),
            kv("dry_run", Value::BoolValue(false)),
            kv("resilience.target.environment", s("prod")),
        ],
        status: Some(Status {
            code: 2,
            message: "pod not found".into(),
        }),
        ..Span::default()
    };
    let checkout = Span {
        trace_id: vec![0xcd; 16],
        span_id: vec![0x03; 8],
        name: "GET /checkout".into(),
        kind: 2,
        start_time_unix_nano: 1_774_980_100_000_000_000,
        end_time_unix_nano: 1_774_980_100_050_000_000,
        attributes: vec![
            kv("http.request.method", s("GET")),
            kv("http.response.status_code", Value::IntValue(200)),
        ],
        status: None,
        ..Span::default()
    };
    ExportTraceServiceRequest {
        resource_spans: vec![
            ResourceSpans {
                resource: resource(vec![
                    kv("service.name", s("tumult")),
                    kv("service.version", s("2.22.0")),
                    kv("host.name", s("runner-1")),
                ]),
                scope_spans: vec![ScopeSpans {
                    scope: scope("tumult-cli", "2.22.0", vec![kv("scope.key", s("v"))]),
                    spans: vec![experiment, action],
                    schema_url: "https://opentelemetry.io/schemas/1.26.0".into(),
                }],
                schema_url: String::new(),
            },
            ResourceSpans {
                resource: resource(vec![
                    kv("service.name", s("checkout")),
                    kv("deployment.environment.name", s("staging")),
                    kv("resilience.target.system", s("api")),
                    kv("resilience.target.technology", s("python")),
                    kv("resilience.target.component", s("shop/checkout")),
                ]),
                scope_spans: vec![ScopeSpans {
                    scope: None,
                    spans: vec![checkout],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            },
        ],
    }
}

/// Rows in a carrier-independent shape: sorted by span id, attribute maps
/// sorted by key (the Arrow carrier sorts attribute tables by key, which
/// the lake's MAP columns do not care about).
fn normalised(mut rows: Vec<SpanRow>) -> Vec<String> {
    rows.sort_by(|a, b| a.span_id.cmp(&b.span_id));
    rows.iter_mut()
        .map(|r| {
            r.span_attrs.sort();
            r.resource_attrs.sort();
            format!("{r:?}")
        })
        .collect()
}

fn decode_traces(consumer: &mut StreamConsumer, records: BatchArrowRecords) -> Vec<SpanRow> {
    let batch = consumer.consume(records).unwrap();
    let request = to_trace_request(&batch).unwrap();
    tumult_otlp::trace_request_to_spans(&request)
}

#[test]
fn traces_round_trip_matches_the_otlp_rows() {
    for compress in [true, false] {
        let request = traces_fixture();
        let expected = tumult_otlp::trace_request_to_spans(&request);
        let mut encoder = Encoder::new(compress);
        let mut consumer = StreamConsumer::default();
        let rows = decode_traces(&mut consumer, encoder.traces(&request));

        assert_eq!(rows.len(), 3, "compress={compress}");
        assert_eq!(
            normalised(rows.clone()),
            normalised(expected),
            "compress={compress}"
        );

        // The assertion above is only meaningful if the promoted columns
        // really came through the Arrow carrier.
        let by_name = |name: &str| rows.iter().find(|r| r.span_name == name).unwrap();
        let exp = by_name("resilience.experiment");
        assert_eq!(exp.service_name, "tumult");
        assert_eq!(exp.experiment_id.as_deref(), Some("exp-otap-1"));
        assert_eq!(exp.experiment_name.as_deref(), Some("pg-failover"));
        assert_eq!(exp.outcome_status.as_deref(), Some("completed"));
        assert_eq!(exp.hypothesis_met, Some(true));
        assert_eq!(exp.recovery_time_s, Some(12.5));
        assert_eq!(exp.fault_type.as_deref(), Some("termination"));
        assert_eq!(exp.plugin_name.as_deref(), Some("kubernetes"));
        assert_eq!(exp.target_system.as_deref(), Some("database"));
        assert_eq!(exp.status_code, "Ok");
        assert_eq!(exp.duration_ns, 300_000_000_000);
        assert!(exp.events.contains("fault.injected") && exp.events.contains("pod-kill"));
        let events: serde_json::Value = serde_json::from_str(&exp.events).unwrap();
        assert_eq!(
            events[0]["name"], "fault.injected",
            "events keep time order"
        );
        assert_eq!(events[1]["attributes"]["actor"], "morgan");

        let action = by_name("resilience.action");
        assert_eq!(action.parent_span_id.as_deref(), Some(&"01".repeat(8)[..]));
        assert_eq!(action.status_code, "Error");
        assert_eq!(action.status_message, "pod not found");
        assert_eq!(action.fault_severity.as_deref(), Some("high"));
        assert_eq!(action.target_environment.as_deref(), Some("prod"));
        let attr = |k: &str| {
            action
                .span_attrs
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(attr("payload"), Some("010203"));
        assert_eq!(attr("targets"), Some(r#"["pg-0","7"]"#));
        assert_eq!(attr("labels"), Some(r#"{"tier":"db","replicas":"2"}"#));
        assert_eq!(attr("dry_run"), Some("false"));

        let checkout = by_name("GET /checkout");
        assert_eq!(checkout.service_name, "checkout");
        assert_eq!(checkout.target_system.as_deref(), Some("api"));
        assert_eq!(checkout.target_technology.as_deref(), Some("python"));
        assert_eq!(checkout.target_environment.as_deref(), Some("staging"));
        assert_eq!(checkout.status_code, "Unset");
        assert!(checkout
            .resource_attrs
            .contains(&("resilience.target.component".into(), "shop/checkout".into())));
    }
}

#[test]
fn later_batches_on_a_stream_carry_no_schema_message() {
    let mut encoder = Encoder::new(true);
    let mut consumer = StreamConsumer::default();
    let first = traces_fixture();
    let mut second = traces_fixture();
    second.resource_spans.truncate(1);
    second.resource_spans[0].scope_spans[0].spans.truncate(1);
    second.resource_spans[0].scope_spans[0].spans[0].span_id = vec![0x09; 8];

    let batch1 = encoder.traces(&first);
    let batch2 = encoder.traces(&second);
    assert_ne!(batch1.batch_id, batch2.batch_id);
    // Payload bytes of the second batch are record batches only.
    assert!(batch2.arrow_payloads[0].record.len() < batch1.arrow_payloads[0].record.len());

    assert_eq!(decode_traces(&mut consumer, batch1).len(), 3);
    let rows = decode_traces(&mut consumer, batch2);
    assert_eq!(
        normalised(rows),
        normalised(tumult_otlp::trace_request_to_spans(&second))
    );

    // A fresh consumer cannot start from a continuation payload.
    let orphan = encoder.traces(&second);
    let err = StreamConsumer::default().consume(orphan).unwrap_err();
    assert!(
        matches!(
            err,
            OtapError::Malformed {
                payload: "SPANS",
                ..
            }
        ),
        "{err}"
    );
}

#[test]
fn unknown_payload_types_and_garbage_are_rejected() {
    let mut consumer = StreamConsumer::default();
    let err = consumer
        .consume(BatchArrowRecords {
            batch_id: 1,
            arrow_payloads: vec![ArrowPayload {
                schema_id: "x".into(),
                r#type: 99,
                record: vec![],
            }],
            headers: vec![],
        })
        .unwrap_err();
    assert!(matches!(err, OtapError::UnknownPayloadType(99)), "{err}");

    // The failure above ended that stream; garbage on a new one.
    let err = StreamConsumer::default()
        .consume(BatchArrowRecords {
            batch_id: 2,
            arrow_payloads: vec![ArrowPayload {
                schema_id: "y".into(),
                r#type: super::proto::ArrowPayloadType::Spans as i32,
                record: b"not arrow".to_vec(),
            }],
            headers: vec![],
        })
        .unwrap_err();
    assert!(
        matches!(
            err,
            OtapError::Malformed {
                payload: "SPANS",
                ..
            }
        ),
        "{err}"
    );

    // A traces batch without a SPANS table is a conversion error, not a panic.
    let mut encoder = Encoder::new(false);
    let mut records = encoder.traces(&traces_fixture());
    records.arrow_payloads.remove(0);
    let batch = StreamConsumer::default().consume(records).unwrap();
    assert!(matches!(
        to_trace_request(&batch).unwrap_err(),
        OtapError::MissingTable("SPANS")
    ));
}

pub(super) fn logs_fixture() -> ExportLogsServiceRequest {
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: resource(vec![
                kv("service.name", s("checkout")),
                kv("resilience.target.system", s("api")),
            ]),
            scope_logs: vec![ScopeLogs {
                scope: scope("flask", "3.0", vec![]),
                log_records: vec![
                    LogRecord {
                        time_unix_nano: 1_785_268_000_000_000_000,
                        severity_number: 9,
                        severity_text: "INFO".into(),
                        body: Some(AnyValue {
                            value: Some(s("experiment.started")),
                        }),
                        attributes: vec![
                            kv("run", Value::IntValue(4)),
                            kv("ok", Value::BoolValue(true)),
                        ],
                        trace_id: vec![0xab; 16],
                        span_id: vec![0x01; 8],
                        ..LogRecord::default()
                    },
                    LogRecord {
                        observed_time_unix_nano: 1_785_268_000_000_000_001,
                        severity_text: "WARN".into(),
                        body: Some(AnyValue {
                            value: Some(Value::KvlistValue(KeyValueList {
                                values: vec![kv("code", Value::IntValue(503))],
                            })),
                        }),
                        ..LogRecord::default()
                    },
                    LogRecord {
                        severity_text: "DEBUG".into(),
                        body: None,
                        ..LogRecord::default()
                    },
                ],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    }
}

#[test]
fn logs_round_trip_matches_the_otlp_rows() {
    let request = logs_fixture();
    let now = 1_785_268_999_000_000_000;
    let expected = tumult_otlp::logs_request_to_rows(&request, now);
    let mut encoder = Encoder::new(true);
    let batch = StreamConsumer::default()
        .consume(encoder.logs(&request))
        .unwrap();
    let mut rows = tumult_otlp::logs_request_to_rows(&to_logs_request(&batch).unwrap(), now);
    let mut expected = expected;
    for r in rows.iter_mut().chain(expected.iter_mut()) {
        r.log_attrs.sort();
        r.resource_attrs.sort();
    }
    assert_eq!(format!("{rows:?}"), format!("{expected:?}"));
    assert_eq!(rows[0].body, "experiment.started");
    assert_eq!(rows[0].trace_id.as_deref(), Some(&"ab".repeat(16)[..]));
    assert_eq!(rows[1].body, r#"{"code":"503"}"#);
    assert_eq!(rows[1].ts_ns, 1_785_268_000_000_000_001);
    assert_eq!(
        rows[2].ts_ns, now,
        "no timestamp falls back to receipt time"
    );
    assert_eq!(rows[2].body, "");
}

pub(super) fn metrics_fixture() -> ExportMetricsServiceRequest {
    let point = |t: u64, value: number_data_point::Value, attrs: Vec<KeyValue>| NumberDataPoint {
        attributes: attrs,
        start_time_unix_nano: 1_785_000_000_000_000_000,
        time_unix_nano: t,
        exemplars: vec![],
        flags: 0,
        value: Some(value),
    };
    ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            resource: resource(vec![kv("service.name", s("tumult"))]),
            scope_metrics: vec![ScopeMetrics {
                scope: scope("tumult", "2.22.0", vec![]),
                metrics: vec![
                    Metric {
                        name: "tumult.experiments.total".into(),
                        unit: "1".into(),
                        data: Some(Data::Sum(Sum {
                            data_points: vec![
                                point(
                                    1_785_000_001_000_000_000,
                                    number_data_point::Value::AsInt(3),
                                    vec![
                                        kv("resilience.plugin.name", s("ssh")),
                                        kv("resilience.outcome.status", s("completed")),
                                    ],
                                ),
                                point(
                                    1_785_000_002_000_000_000,
                                    number_data_point::Value::AsInt(1),
                                    vec![kv("resilience.plugin.name", s("ssh"))],
                                ),
                            ],
                            aggregation_temporality: 2,
                            is_monotonic: true,
                        })),
                        ..Metric::default()
                    },
                    Metric {
                        name: "tumult.store.size".into(),
                        description: "lake size".into(),
                        unit: "By".into(),
                        data: Some(Data::Gauge(Gauge {
                            data_points: vec![point(
                                1_785_000_003_000_000_000,
                                number_data_point::Value::AsDouble(0.5),
                                vec![],
                            )],
                        })),
                        ..Metric::default()
                    },
                    Metric {
                        name: "tumult.action.duration".into(),
                        unit: "s".into(),
                        data: Some(Data::Histogram(Histogram {
                            data_points: vec![HistogramDataPoint {
                                attributes: vec![kv("resilience.plugin.name", s("kubernetes"))],
                                start_time_unix_nano: 1_785_000_000_000_000_000,
                                time_unix_nano: 1_785_000_004_000_000_000,
                                count: 2,
                                sum: Some(10.0),
                                bucket_counts: vec![1, 1],
                                explicit_bounds: vec![5.0],
                                exemplars: vec![],
                                flags: 0,
                                min: Some(2.0),
                                max: Some(8.0),
                            }],
                            aggregation_temporality: 2,
                        })),
                        ..Metric::default()
                    },
                    Metric {
                        name: "unsupported.summary".into(),
                        data: Some(Data::Summary(Summary {
                            data_points: vec![SummaryDataPoint::default(); 2],
                        })),
                        ..Metric::default()
                    },
                    Metric {
                        name: "unsupported.exp".into(),
                        data: Some(Data::ExponentialHistogram(ExponentialHistogram {
                            data_points: vec![ExponentialHistogramDataPoint::default()],
                            aggregation_temporality: 1,
                        })),
                        ..Metric::default()
                    },
                ],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    }
}

fn normalised_metrics(mut rows: tumult_otlp::MetricRows) -> String {
    for r in &mut rows.sums {
        r.attrs.sort();
    }
    for r in &mut rows.gauges {
        r.attrs.sort();
    }
    for r in &mut rows.histograms {
        r.attrs.sort();
    }
    format!(
        "rejected={} sums={:?} gauges={:?} histograms={:?}",
        rows.rejected_data_points, rows.sums, rows.gauges, rows.histograms
    )
}

#[test]
fn metrics_round_trip_matches_the_otlp_rows() {
    let request = metrics_fixture();
    let expected = tumult_otlp::metrics_request_to_rows(&request);
    assert_eq!(expected.rejected_data_points, 3);
    let mut encoder = Encoder::new(true);
    let batch = StreamConsumer::default()
        .consume(encoder.metrics(&request))
        .unwrap();
    let decoded = to_metrics_request(&batch).unwrap();
    let rows = tumult_otlp::metrics_request_to_rows(&decoded);
    assert_eq!(rows.sums.len(), 2);
    assert_eq!(rows.gauges.len(), 1);
    assert_eq!(rows.histograms.len(), 1);
    assert_eq!(rows.histograms[0].bucket_counts, vec![1, 1]);
    assert_eq!(normalised_metrics(rows), normalised_metrics(expected));
}

// ---------------------------------------------------------------------------
// gRPC level: a real stream against the router the daemon serves.
// ---------------------------------------------------------------------------

struct Served {
    _dir: tempfile::TempDir,
    store: tumult_lake::Store,
    addr: std::net::SocketAddr,
    _server: tokio::task::JoinHandle<()>,
}

async fn serve(token: Option<&str>) -> Served {
    let dir = tempfile::TempDir::new().unwrap();
    let store = tumult_lake::Store::open(&dir.path().join("k.duckdb")).unwrap();
    let (ingest, _task) = IngestWriter::spawn(store.writer().unwrap(), 16);
    let router = crate::grpc::router_with_token(ingest, token.map(str::to_string));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = tonic::codegen::tokio_stream::wrappers::TcpListenerStream::new(listener);
    let server = tokio::spawn(async move {
        // The writer handle lives inside the router; the task ends with it.
        router.serve_with_incoming(incoming).await.unwrap();
    });
    Served {
        _dir: dir,
        store,
        addr,
        _server: server,
    }
}

async fn connect(
    addr: std::net::SocketAddr,
) -> ArrowTracesServiceClient<tonic::transport::Channel> {
    for _ in 0..50 {
        if let Ok(c) = ArrowTracesServiceClient::connect(format!("http://{addr}")).await {
            return c;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("gRPC server did not come up");
}

#[tokio::test]
async fn arrow_stream_acks_every_batch_and_stores_the_promoted_columns() {
    let served = serve(Some("kro_secret")).await;
    let mut client = connect(served.addr).await;

    let (tx, rx) = tokio::sync::mpsc::channel(8);
    let outbound = tonic::codegen::tokio_stream::wrappers::ReceiverStream::new(rx);
    let mut request = tonic::Request::new(outbound);
    request.metadata_mut().insert(
        "authorization",
        tonic::metadata::MetadataValue::from_static("Bearer kro_secret"),
    );
    let mut acks = client.arrow_traces(request).await.unwrap().into_inner();

    let mut encoder = Encoder::new(true);
    let good = encoder.traces(&traces_fixture());
    let good_id = good.batch_id;
    tx.send(good).await.unwrap();
    let ack = acks.message().await.unwrap().unwrap();
    assert_eq!(
        (ack.batch_id, ack.status_code),
        (good_id, StatusCode::Ok as i32)
    );

    // A continuation batch (no schema message) on the same stream.
    let mut second = traces_fixture();
    second.resource_spans.truncate(1);
    second.resource_spans[0].scope_spans[0].spans.truncate(1);
    second.resource_spans[0].scope_spans[0].spans[0].span_id = vec![0x09; 8];
    let again = encoder.traces(&second);
    let again_id = again.batch_id;
    tx.send(again).await.unwrap();
    let ack = acks.message().await.unwrap().unwrap();
    assert_eq!(
        (ack.batch_id, ack.status_code),
        (again_id, StatusCode::Ok as i32)
    );

    // A broken payload is refused in its own status, and then the stream
    // ends with an error: the schemas and dictionaries it may have carried
    // are lost, so nothing after it could be decoded correctly.
    tx.send(BatchArrowRecords {
        batch_id: 77,
        arrow_payloads: vec![ArrowPayload {
            schema_id: "SPANS-1".into(),
            r#type: super::proto::ArrowPayloadType::Spans as i32,
            record: b"definitely not arrow".to_vec(),
        }],
        headers: vec![],
    })
    .await
    .unwrap();
    let ack = acks.message().await.unwrap().unwrap();
    assert_eq!(ack.batch_id, 77);
    assert_eq!(ack.status_code, StatusCode::InvalidArgument as i32);
    assert!(
        ack.status_message.contains("SPANS"),
        "{}",
        ack.status_message
    );
    let closed = acks.message().await.unwrap_err();
    assert_eq!(closed.code(), tonic::Code::InvalidArgument, "{closed}");
    assert!(
        closed.message().starts_with("otel-arrow decode: "),
        "{closed}"
    );
    // A batch sent into the dead stream is neither decoded nor acked.
    let mut third = second.clone();
    third.resource_spans[0].scope_spans[0].spans[0].span_id = vec![0x0a; 8];
    let _ = tx.send(encoder.traces(&third)).await;
    assert!(!matches!(acks.message().await, Ok(Some(_))));

    // The exporter's reaction is a new stream, which starts from scratch.
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    let outbound = tonic::codegen::tokio_stream::wrappers::ReceiverStream::new(rx);
    let mut request = tonic::Request::new(outbound);
    request.metadata_mut().insert(
        "authorization",
        tonic::metadata::MetadataValue::from_static("Bearer kro_secret"),
    );
    let mut acks = client.arrow_traces(request).await.unwrap().into_inner();
    third.resource_spans[0].scope_spans[0].spans[0].span_id = vec![0x0b; 8];
    tx.send(Encoder::new(true).traces(&third)).await.unwrap();
    let ack = acks.message().await.unwrap().unwrap();
    assert_eq!(ack.status_code, StatusCode::Ok as i32);
    drop(tx);
    assert!(
        acks.message().await.unwrap().is_none(),
        "stream ends with the client"
    );

    let rows = served
        .store
        .read_only()
        .unwrap()
        .query_json_rows(
            "SELECT span_name, service_name, experiment_id, target_system, target_technology, \
             target_environment, outcome_status, hypothesis_met, status_code, \
             span_attrs['resilience.estimate.rationale'] AS rationale \
             FROM spans ORDER BY span_id",
        )
        .unwrap();
    assert_eq!(rows.len(), 5, "{rows:?}");
    assert_eq!(rows[0]["span_name"], "resilience.experiment");
    assert_eq!(rows[0]["service_name"], "tumult");
    assert_eq!(rows[0]["experiment_id"], "exp-otap-1");
    assert_eq!(rows[0]["target_system"], "database");
    assert_eq!(rows[0]["outcome_status"], "completed");
    assert_eq!(rows[0]["hypothesis_met"], true);
    assert_eq!(rows[0]["status_code"], "Ok");
    assert_eq!(rows[0]["rationale"], "primary restart is fast");
    assert_eq!(rows[1]["span_name"], "resilience.action");
    assert_eq!(rows[1]["target_environment"], "prod");
    assert_eq!(rows[2]["service_name"], "checkout");
    assert_eq!(rows[2]["target_system"], "api");
    assert_eq!(rows[2]["target_technology"], "python");
    assert_eq!(rows[2]["target_environment"], "staging");
    assert_eq!(
        rows[3]["experiment_id"], "exp-otap-1",
        "continuation batch stored"
    );
    assert_eq!(
        rows[4]["experiment_id"], "exp-otap-1",
        "batch on the new stream stored, the one sent into the dead stream is not"
    );
}

#[tokio::test]
async fn arrow_stream_requires_the_ingest_token() {
    let served = serve(Some("kro_secret")).await;
    let mut client = connect(served.addr).await;
    let (_tx, rx) = tokio::sync::mpsc::channel::<BatchArrowRecords>(1);
    let outbound = tonic::codegen::tokio_stream::wrappers::ReceiverStream::new(rx);
    let err = client
        .arrow_traces(tonic::Request::new(outbound))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Unauthenticated);

    // No token configured: the stream opens.
    let open = serve(None).await;
    let mut client = connect(open.addr).await;
    let (tx, rx) = tokio::sync::mpsc::channel::<BatchArrowRecords>(1);
    let outbound = tonic::codegen::tokio_stream::wrappers::ReceiverStream::new(rx);
    let mut acks = client
        .arrow_traces(tonic::Request::new(outbound))
        .await
        .unwrap()
        .into_inner();
    let mut encoder = Encoder::new(false);
    tx.send(encoder.traces(&traces_fixture())).await.unwrap();
    assert_eq!(
        acks.message().await.unwrap().unwrap().status_code,
        StatusCode::Ok as i32
    );
}

#[tokio::test]
async fn write_failure_is_reported_as_unavailable_so_the_exporter_retries() {
    let dead = super::service::OtapGrpc::new(IngestWriter::stopped_for_test(), None);
    let router = tonic::transport::Server::builder().add_service(dead.traces());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = tonic::codegen::tokio_stream::wrappers::TcpListenerStream::new(listener);
    tokio::spawn(async move { router.serve_with_incoming(incoming).await.unwrap() });
    let mut client = connect(addr).await;
    let (tx, rx) = tokio::sync::mpsc::channel::<BatchArrowRecords>(1);
    let outbound = tonic::codegen::tokio_stream::wrappers::ReceiverStream::new(rx);
    let mut acks = client
        .arrow_traces(tonic::Request::new(outbound))
        .await
        .unwrap()
        .into_inner();
    let mut encoder = Encoder::new(false);
    tx.send(encoder.traces(&traces_fixture())).await.unwrap();
    let ack = acks.message().await.unwrap().unwrap();
    // UNAVAILABLE is the one code the exporter retries; INTERNAL would make
    // it drop the data and tear the stream down.
    assert_eq!(ack.status_code, StatusCode::Unavailable as i32);
    assert_eq!(ack.status_message, "telemetry write failed");
    // The stream itself is intact.
    tx.send(encoder.traces(&traces_fixture())).await.unwrap();
    let ack = acks.message().await.unwrap().unwrap();
    assert_eq!(ack.status_code, StatusCode::Unavailable as i32);
}

#[tokio::test]
async fn oversized_message_ends_the_stream_with_out_of_range() {
    let served = serve(None).await;
    let mut client = connect(served.addr).await;
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    let outbound = tonic::codegen::tokio_stream::wrappers::ReceiverStream::new(rx);
    let mut acks = client
        .arrow_traces(tonic::Request::new(outbound))
        .await
        .unwrap()
        .into_inner();
    tx.send(BatchArrowRecords {
        batch_id: 1,
        arrow_payloads: vec![ArrowPayload {
            schema_id: "SPANS-1".into(),
            r#type: super::proto::ArrowPayloadType::Spans as i32,
            record: vec![0; super::limits::DEFAULT_MAX_MESSAGE_BYTES + 1],
        }],
        headers: vec![],
    })
    .await
    .unwrap();
    // The read error reaches the client as the stream's status, not as a
    // clean end of stream.
    let closed = acks.message().await.unwrap_err();
    assert_eq!(closed.code(), tonic::Code::OutOfRange, "{closed}");
    assert!(
        closed.message().contains("message length too large"),
        "{closed}"
    );
}

#[tokio::test]
async fn multivariate_batch_is_refused_by_id_and_univariate_metrics_still_land() {
    let served = serve(None).await;
    let mut client = ArrowMetricsServiceClient::connect(format!("http://{}", served.addr))
        .await
        .unwrap();
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    let outbound = tonic::codegen::tokio_stream::wrappers::ReceiverStream::new(rx);
    let mut acks = client
        .arrow_metrics(tonic::Request::new(outbound))
        .await
        .unwrap()
        .into_inner();

    // A well-formed Arrow payload labelled as the multivariate table.
    let mut multivariate = Encoder::new(true).metrics(&metrics_fixture());
    multivariate.batch_id = 41;
    multivariate.arrow_payloads.truncate(1);
    multivariate.arrow_payloads[0].r#type =
        super::proto::ArrowPayloadType::MultivariateMetrics as i32;
    multivariate.arrow_payloads[0].schema_id = "MULTIVARIATE_METRICS-1".into();
    tx.send(multivariate).await.unwrap();
    let ack = acks.message().await.unwrap().unwrap();
    assert_eq!(ack.batch_id, 41);
    assert_eq!(ack.status_code, StatusCode::InvalidArgument as i32);
    assert!(
        ack.status_message.contains("MULTIVARIATE_METRICS"),
        "{}",
        ack.status_message
    );

    // The stream is still open and a univariate batch is stored.
    let mut encoder = Encoder::new(true);
    let good = encoder.metrics(&metrics_fixture());
    let good_id = good.batch_id;
    tx.send(good).await.unwrap();
    let ack = acks.message().await.unwrap().unwrap();
    assert_eq!(
        (ack.batch_id, ack.status_code),
        (good_id, StatusCode::Ok as i32)
    );
    let rows = served
        .store
        .read_only()
        .unwrap()
        .query_json_rows(
            "SELECT (SELECT count(*) FROM metric_sums) AS sums, \
             (SELECT count(*) FROM metric_gauges) AS gauges, \
             (SELECT count(*) FROM metric_histograms) AS histograms",
        )
        .unwrap();
    assert_eq!(rows[0]["sums"], 2, "{rows:?}");
    assert_eq!(rows[0]["gauges"], 1, "{rows:?}");
    assert_eq!(rows[0]["histograms"], 1, "{rows:?}");
}

type GrpcRequest = tonic::codegen::http::Request<tonic::body::Body>;

/// A client channel that renames the `grpc-encoding` of every request, the
/// way the Collector's exporter labels its zstd-compressed messages.
#[derive(Clone)]
struct Relabel(tonic::transport::Channel, &'static str);

impl tonic::codegen::Service<GrpcRequest> for Relabel {
    type Response = <tonic::transport::Channel as tonic::codegen::Service<GrpcRequest>>::Response;
    type Error = tonic::transport::Error;
    type Future = <tonic::transport::Channel as tonic::codegen::Service<GrpcRequest>>::Future;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.0.poll_ready(cx)
    }

    fn call(&mut self, mut req: GrpcRequest) -> Self::Future {
        req.headers_mut().insert(
            "grpc-encoding",
            tonic::codegen::http::HeaderValue::from_static(self.1),
        );
        self.0.call(req)
    }
}

async fn relabelled(
    addr: std::net::SocketAddr,
    encoding: &'static str,
) -> ArrowTracesServiceClient<Relabel> {
    let channel = tonic::transport::Endpoint::from_shared(format!("http://{addr}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    ArrowTracesServiceClient::new(Relabel(channel, encoding))
        .send_compressed(tonic::codegen::CompressionEncoding::Zstd)
}

#[tokio::test]
async fn exporter_default_zstdarrow_grpc_encoding_is_accepted() {
    for encoding in ["zstdarrow1", "zstdarrow5", "zstdarrow9"] {
        let served = serve(None).await;
        let mut client = relabelled(served.addr, encoding).await;
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let outbound = tonic::codegen::tokio_stream::wrappers::ReceiverStream::new(rx);
        let mut acks = client
            .arrow_traces(tonic::Request::new(outbound))
            .await
            .unwrap()
            .into_inner();
        tx.send(Encoder::new(true).traces(&traces_fixture()))
            .await
            .unwrap();
        let ack = acks.message().await.unwrap().unwrap();
        assert_eq!(ack.status_code, StatusCode::Ok as i32, "{encoding}");
        let rows = served
            .store
            .read_only()
            .unwrap()
            .query_json_rows("SELECT count(*) AS n FROM spans")
            .unwrap();
        assert_eq!(rows[0]["n"], 3, "{encoding}: {rows:?}");
    }
}

#[tokio::test]
async fn unknown_grpc_encoding_is_refused_by_name() {
    let served = serve(None).await;
    for encoding in ["zstdarrow0", "zstdarrow10", "snappy"] {
        let mut client = relabelled(served.addr, encoding).await;
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        tx.send(Encoder::new(false).traces(&traces_fixture()))
            .await
            .unwrap();
        let outbound = tonic::codegen::tokio_stream::wrappers::ReceiverStream::new(rx);
        let err = match client.arrow_traces(tonic::Request::new(outbound)).await {
            Err(status) => status,
            Ok(response) => response.into_inner().message().await.unwrap_err(),
        };
        assert_eq!(err.code(), tonic::Code::Unimplemented, "{encoding}");
        assert!(err.message().contains(encoding), "{}", err.message());
    }
}

// ---------------------------------------------------------------------------
// Stream endings: nothing but a clean client close may look like one.
// ---------------------------------------------------------------------------

use super::service::{respond, Convert, StreamContext};
use tonic::codegen::tokio_stream::{self, StreamExt};

fn context(limits: super::OtapLimits, convert: Convert) -> StreamContext {
    StreamContext {
        ingest: IngestWriter::stopped_for_test(),
        limits,
        decodes: super::decode_gate::DecodeGate::new(1),
        signal: "traces",
        convert,
        guards: super::service::Guards::new(&limits),
        slot: None,
    }
}

fn no_rows(_: &super::OtapBatch, _: &IngestWriter) -> Result<crate::writer::Batch, OtapError> {
    Ok(crate::writer::Batch::Spans(Vec::new()))
}

#[tokio::test]
async fn stream_read_error_is_sent_to_the_client() {
    let inbound = tokio_stream::iter(vec![Err(tonic::Status::data_loss("frame cut short"))]);
    let mut out = respond(
        inbound,
        context(super::OtapLimits::default(), no_rows),
        None,
    );
    let status = out.next().await.unwrap().unwrap_err();
    assert_eq!(status.code(), tonic::Code::DataLoss);
    assert_eq!(status.message(), "frame cut short");
    assert!(out.next().await.is_none());
}

#[tokio::test]
async fn panic_while_decoding_is_an_internal_status_and_an_error_log() {
    fn explode(_: &super::OtapBatch, _: &IngestWriter) -> Result<crate::writer::Batch, OtapError> {
        panic!("decoder bug with secret-payload-bytes");
    }
    let (logs, _guard) = crate::test_logs::capture();
    let inbound = tokio_stream::iter(vec![Ok(Encoder::new(false).traces(&traces_fixture()))]);
    let mut out = respond(
        inbound,
        context(super::OtapLimits::default(), explode),
        None,
    );
    let status = out.next().await.unwrap().unwrap_err();
    assert_eq!(status.code(), tonic::Code::Internal);
    assert_eq!(status.message(), "otel-arrow decode task failed");
    assert!(out.next().await.is_none());
    let text = logs.text();
    assert!(
        text.contains("ERROR") && text.contains("OTAP decode task failed"),
        "{text}"
    );
}

#[tokio::test]
async fn silent_stream_is_closed_as_unavailable() {
    let limits = super::OtapLimits {
        stream_idle: std::time::Duration::from_millis(50),
        ..super::OtapLimits::default()
    };
    let inbound = tokio_stream::pending::<Result<BatchArrowRecords, tonic::Status>>();
    let mut out = respond(inbound, context(limits, no_rows), None);
    let status = out.next().await.unwrap().unwrap_err();
    assert_eq!(status.code(), tonic::Code::Unavailable);
    assert!(status.message().contains("idle"), "{status}");
}

async fn serve_with(limits: super::OtapLimits) -> Served {
    let dir = tempfile::TempDir::new().unwrap();
    let store = tumult_lake::Store::open(&dir.path().join("k.duckdb")).unwrap();
    let (ingest, _task) = IngestWriter::spawn(store.writer().unwrap(), 16);
    let otap = super::service::OtapGrpc::with_limits(ingest, None, limits);
    let router = tonic::transport::Server::builder().add_service(otap.traces());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    // The daemon's listener: the per-connection share needs the peer
    // address to come through it.
    let incoming = crate::listener::incoming(
        listener,
        crate::listener::ConnectionLimits::default(),
        crate::listener::Access::Open,
    );
    let server = tokio::spawn(async move {
        router.serve_with_incoming(incoming).await.unwrap();
    });
    Served {
        _dir: dir,
        store,
        addr,
        _server: server,
    }
}

#[tokio::test]
async fn streams_beyond_the_limit_are_refused_until_one_closes() {
    let served = serve_with(super::OtapLimits {
        max_streams: 1,
        ..super::OtapLimits::default()
    })
    .await;
    let mut client = connect(served.addr).await;
    let open = |client: &mut ArrowTracesServiceClient<tonic::transport::Channel>| {
        let (tx, rx) = tokio::sync::mpsc::channel::<BatchArrowRecords>(1);
        let outbound = tonic::codegen::tokio_stream::wrappers::ReceiverStream::new(rx);
        let mut client = client.clone();
        async move { (tx, client.arrow_traces(tonic::Request::new(outbound)).await) }
    };
    let (first_tx, first) = open(&mut client).await;
    let mut first = first.unwrap().into_inner();
    let (_tx, second) = open(&mut client).await;
    let refused = second.unwrap_err();
    assert_eq!(refused.code(), tonic::Code::ResourceExhausted, "{refused}");

    // The first stream still works, and its slot is free once it ends.
    first_tx
        .send(Encoder::new(false).traces(&traces_fixture()))
        .await
        .unwrap();
    assert_eq!(
        first.message().await.unwrap().unwrap().status_code,
        StatusCode::Ok as i32
    );
    drop(first_tx);
    assert!(first.message().await.unwrap().is_none());
    let mut reopened = None;
    for _ in 0..100 {
        let (tx, result) = open(&mut client).await;
        if let Ok(response) = result {
            reopened = Some((tx, response));
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(reopened.is_some(), "the closed stream's slot was not freed");
}

#[tokio::test]
async fn batch_over_a_decode_limit_closes_the_stream_as_resource_exhausted() {
    let served = serve_with(super::OtapLimits {
        max_rows_per_table: 2,
        ..super::OtapLimits::default()
    })
    .await;
    let mut client = connect(served.addr).await;
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    let outbound = tonic::codegen::tokio_stream::wrappers::ReceiverStream::new(rx);
    let mut acks = client
        .arrow_traces(tonic::Request::new(outbound))
        .await
        .unwrap()
        .into_inner();
    // Three spans against a limit of two rows.
    let batch = Encoder::new(false).traces(&traces_fixture());
    let batch_id = batch.batch_id;
    tx.send(batch).await.unwrap();
    let ack = acks.message().await.unwrap().unwrap();
    assert_eq!(
        (ack.batch_id, ack.status_code),
        (batch_id, StatusCode::InvalidArgument as i32)
    );
    assert!(
        ack.status_message
            .contains("rows per table exceeds the limit of 2"),
        "{}",
        ack.status_message
    );
    let closed = acks.message().await.unwrap_err();
    assert_eq!(closed.code(), tonic::Code::ResourceExhausted, "{closed}");
    let rows = served
        .store
        .read_only()
        .unwrap()
        .query_json_rows("SELECT count(*) AS n FROM spans")
        .unwrap();
    assert_eq!(rows[0]["n"], 0, "{rows:?}");
}

#[test]
fn batch_reports_how_many_tables_continued_an_earlier_schema() {
    let mut encoder = Encoder::new(false);
    let mut consumer = StreamConsumer::default();
    let first = consumer.consume(encoder.traces(&traces_fixture())).unwrap();
    assert!(first.payloads() > 1);
    assert_eq!(first.continued_payloads(), 0);
    let second = consumer.consume(encoder.traces(&traces_fixture())).unwrap();
    assert_eq!(second.continued_payloads(), second.payloads());
    assert_eq!(second.payloads(), first.payloads());
}

// ---------------------------------------------------------------------------
// Streams that misbehave over time rather than in one batch.
// ---------------------------------------------------------------------------

fn rejected(_: &super::OtapBatch, _: &IngestWriter) -> Result<crate::writer::Batch, OtapError> {
    Err(OtapError::Unsupported("TEST_ONLY"))
}

fn batches(n: usize) -> Vec<Result<BatchArrowRecords, tonic::Status>> {
    let mut encoder = Encoder::new(false);
    (0..n)
        .map(|_| Ok(encoder.traces(&traces_fixture())))
        .collect()
}

#[tokio::test]
async fn stream_closes_after_too_many_rejected_batches() {
    let limits = super::OtapLimits {
        max_rejected_batches: 2,
        ..super::OtapLimits::default()
    };
    let mut out = respond(
        tokio_stream::iter(batches(5)),
        context(limits, rejected),
        None,
    );
    for _ in 0..2 {
        let ack = out.next().await.unwrap().unwrap();
        assert_eq!(ack.status_code, StatusCode::InvalidArgument as i32);
    }
    let closed = out.next().await.unwrap().unwrap_err();
    assert_eq!(closed.code(), tonic::Code::InvalidArgument, "{closed}");
    assert!(closed.message().contains("rejected batches"), "{closed}");
    assert!(out.next().await.is_none(), "nothing after the close");
}

#[tokio::test]
async fn rejected_batch_warnings_are_rate_limited() {
    let limits = super::OtapLimits {
        max_rejected_batches: 1000,
        ..super::OtapLimits::default()
    };
    let (logs, _guard) = crate::test_logs::capture();
    let mut out = respond(
        tokio_stream::iter(batches(200)),
        context(limits, rejected),
        None,
    );
    let mut acks = 0;
    while let Some(item) = out.next().await {
        item.unwrap();
        acks += 1;
    }
    assert_eq!(acks, 200, "every batch is still answered");
    let text = logs.text();
    let warned = text.matches("OTAP batch rejected").count();
    assert!(
        (1..=20).contains(&warned),
        "{warned} warnings for 200 rejected batches"
    );
}

/// A client that floods the log with one warning does not get to hide
/// the next one of another kind.
#[tokio::test]
async fn a_flood_of_rejected_batches_does_not_hide_an_undecodable_one() {
    let limits = super::OtapLimits {
        max_rejected_batches: 1000,
        ..super::OtapLimits::default()
    };
    let mut stream = batches(200);
    stream.push(Ok(BatchArrowRecords {
        batch_id: 201,
        arrow_payloads: vec![ArrowPayload {
            schema_id: "s".into(),
            r#type: super::proto::ArrowPayloadType::Spans as i32,
            record: b"not arrow ipc".to_vec(),
        }],
        headers: Vec::new(),
    }));
    let (logs, _guard) = crate::test_logs::capture();
    let mut out = respond(tokio_stream::iter(stream), context(limits, rejected), None);
    while out.next().await.is_some() {}
    let text = logs.text();
    let rejected = text.matches("OTAP batch rejected").count();
    assert_eq!(rejected, 10, "the burst went to the flood: {text}");
    assert_eq!(text.matches("OTAP batch undecodable").count(), 1, "{text}");
}

/// `line` without its quoted values: what a reader of `key=value` pairs
/// takes for the keys and the bare values.
fn outside_quotes(line: &str) -> String {
    let (mut out, mut quoted, mut escaped) = (String::new(), false, false);
    for c in line.chars() {
        match (quoted, escaped, c) {
            (true, true, _) => escaped = false,
            (true, false, '\\') => escaped = true,
            (_, false, '"') => quoted = !quoted,
            (false, ..) => out.push(c),
            (true, ..) => {}
        }
    }
    out
}

#[test]
fn text_inside_a_quoted_value_is_not_outside_it() {
    let line = r#"WARN a=1 error="x \" b=2 \\" c=3 note="d=4" e=5"#;
    assert_eq!(outside_quotes(line), "WARN a=1 error= c=3 note= e=5");
}

/// A schema id (or anything else a peer chooses) is logged inside one
/// quoted value: it cannot add `key=value` pairs to the line.
#[tokio::test]
async fn peer_text_in_a_warning_cannot_forge_fields() {
    let schema = arrow::datatypes::Schema::new(vec![arrow::datatypes::Field::new(
        "id",
        arrow::datatypes::DataType::UInt16,
        true,
    )]);
    let forgeries = [
        "x suppressed=99 signal=forged",
        "x\" suppressed=99 signal=\"forged",
        "x\\\" suppressed=99 signal=forged",
    ];
    for schema_id in forgeries {
        let records = BatchArrowRecords {
            batch_id: 7,
            arrow_payloads: vec![ArrowPayload {
                schema_id: schema_id.into(),
                r#type: super::proto::ArrowPayloadType::Spans as i32,
                record: super::craft::schema_message(&schema),
            }],
            headers: Vec::new(),
        };
        let (logs, _guard) = crate::test_logs::capture();
        let mut out = respond(
            tokio_stream::iter(vec![Ok(records)]),
            context(super::OtapLimits::default(), no_rows),
            None,
        );
        let refused = out.next().await.unwrap().unwrap();
        assert_eq!(refused.status_code, StatusCode::InvalidArgument as i32);
        while out.next().await.is_some() {}
        let text = logs.text();
        let line = text
            .lines()
            .find(|line| line.contains("OTAP batch undecodable"))
            .unwrap_or_else(|| panic!("no warning: {text}"));
        assert!(line.contains("signal=\"traces\""), "{line}");
        assert!(line.contains("error=\"SPANS payload for schema"), "{line}");
        let bare = outside_quotes(line);
        assert!(
            !bare.contains("forged") && !bare.contains("suppressed=99"),
            "{schema_id:?} reached the fields: {line}"
        );
        assert!(bare.contains("suppressed=0"), "{line}");
    }
}

#[tokio::test]
async fn acks_nobody_reads_end_the_stream() {
    let limits = super::OtapLimits {
        ack_timeout: std::time::Duration::from_millis(50),
        ..super::OtapLimits::default()
    };
    let slots = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
    let permit = slots.clone().try_acquire_owned().unwrap();
    // Ten batches, then silence; the client never reads a status.
    let inbound = tokio_stream::iter(batches(10)).chain(tokio_stream::pending());
    let out = respond(inbound, context(limits, no_rows), Some(permit));
    let freed = tokio::time::timeout(std::time::Duration::from_secs(5), slots.acquire()).await;
    assert!(
        freed.is_ok(),
        "the stream still holds its slot although nobody reads its acks"
    );
    drop(out);
}

#[tokio::test]
async fn stream_is_closed_at_its_maximum_lifetime() {
    let limits = super::OtapLimits {
        stream_lifetime: std::time::Duration::from_millis(100),
        ..super::OtapLimits::default()
    };
    let inbound = tokio_stream::pending::<Result<BatchArrowRecords, tonic::Status>>();
    let mut out = respond(inbound, context(limits, no_rows), None);
    let closed = tokio::time::timeout(std::time::Duration::from_secs(5), out.next())
        .await
        .expect("the stream outlived its lifetime");
    let status = closed.unwrap().unwrap_err();
    assert_eq!(status.code(), tonic::Code::Unavailable);
    assert!(status.message().contains("lifetime"), "{status}");
}

#[tokio::test]
async fn one_connection_gets_only_its_share_of_the_streams() {
    let served = serve_with(super::OtapLimits {
        max_streams: 4,
        max_streams_per_connection: 1,
        ..super::OtapLimits::default()
    })
    .await;
    let open = |client: &ArrowTracesServiceClient<tonic::transport::Channel>| {
        let (tx, rx) = tokio::sync::mpsc::channel::<BatchArrowRecords>(1);
        let outbound = tonic::codegen::tokio_stream::wrappers::ReceiverStream::new(rx);
        let mut client = client.clone();
        async move { (tx, client.arrow_traces(tonic::Request::new(outbound)).await) }
    };
    let first_connection = connect(served.addr).await;
    let (_first_tx, first) = open(&first_connection).await;
    first.unwrap();
    let (_tx, second) = open(&first_connection).await;
    let refused = second.unwrap_err();
    assert_eq!(refused.code(), tonic::Code::ResourceExhausted, "{refused}");
    assert!(refused.message().contains("connection"), "{refused}");

    // Another connection still gets a stream.
    let second_connection = connect(served.addr).await;
    let (_second_tx, other) = open(&second_connection).await;
    other.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn decode_permit_is_held_until_the_batch_is_written() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = tumult_lake::Store::open(&dir.path().join("k.duckdb")).unwrap();
    let (ingest, _task) = IngestWriter::spawn(store.writer().unwrap(), 16);
    // Stall the writer inside a batch of its own.
    let (release, stalled) = std::sync::mpsc::channel::<()>();
    let (entered, running) = tokio::sync::oneshot::channel::<()>();
    let blocker = ingest.clone();
    let blocked = tokio::spawn(async move {
        blocker
            .write(crate::writer::Batch::Exec(Box::new(move |_| {
                let _ = entered.send(());
                let _ = stalled.recv();
                Ok(())
            })))
            .await
    });
    running.await.unwrap();
    assert_eq!(ingest.queued(), 0, "the writer took the stalling batch");

    let mut shared = context(super::OtapLimits::default(), no_rows);
    shared.ingest = ingest.clone();
    let decodes = shared.decodes.clone();
    let mut out = respond(tokio_stream::iter(batches(1)), shared, None);

    // The batch is decoded, converted and handed to the stalled writer.
    let handed_over = async {
        while ingest.queued() != 1 {
            tokio::task::yield_now().await;
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(30), handed_over)
        .await
        .expect("the converted batch reaches the writer queue");
    assert_eq!(
        decodes.free(),
        0,
        "the decode permit was given back while the batch still waits for the writer"
    );

    release.send(()).unwrap();
    blocked.await.unwrap().unwrap();
    let ack = out.next().await.unwrap().unwrap();
    assert_eq!(ack.status_code, StatusCode::Ok as i32);
    assert!(out.next().await.is_none());
    assert_eq!(decodes.free(), 1, "stored: the permit is back");
}

// ---------------------------------------------------------------------------
// Entity events: the Arrow carrier stores what the OTLP carriers store.
// ---------------------------------------------------------------------------

use crate::entity_events::tests as entity_fixtures;

/// Send the host export as one OTAP logs batch through a stream task and
/// return the store it was written to.
async fn host_export_over_otap(
    entities: Option<crate::entity_events::EntityEvents>,
    legacy: bool,
) -> (tempfile::TempDir, tumult_lake::Store) {
    let (dir, store, ingest) = entity_fixtures::ingest_with(entities);
    let limits = super::OtapLimits::default();
    let context = StreamContext {
        ingest,
        limits,
        decodes: super::decode_gate::DecodeGate::new(1),
        signal: "logs",
        convert: super::service::convert_logs,
        guards: super::service::Guards::new(&limits),
        slot: None,
    };
    let batch = Encoder::new(true).logs(&entity_fixtures::host_export(legacy));
    let mut out = respond(tokio_stream::iter(vec![Ok(batch)]), context, None);
    let ack = out.next().await.unwrap().unwrap();
    assert_eq!(
        ack.status_code,
        StatusCode::Ok as i32,
        "{}",
        ack.status_message
    );
    (dir, store)
}

#[tokio::test]
async fn entity_events_over_otap_become_entity_rows_in_both_wire_forms() {
    for (legacy, wire_form) in [(false, "spec"), (true, "legacy")] {
        let (_dir, store) = host_export_over_otap(entity_fixtures::enabled(), legacy).await;
        entity_fixtures::assert_host_entity_and_one_log(&store, wire_form);
    }
}

#[tokio::test]
async fn without_the_flag_entity_events_over_otap_are_stored_as_logs() {
    for legacy in [false, true] {
        let (_dir, store) = host_export_over_otap(None, legacy).await;
        entity_fixtures::assert_two_logs_and_no_entity(&store);
    }
}
