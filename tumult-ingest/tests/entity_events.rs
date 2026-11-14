//! Entity events over OTLP/HTTP, through the public surface of the crate:
//! what the daemon stores with `KRONIKA_ENTITY_EVENTS` set and what it
//! stores without.

use opentelemetry_proto::tonic::collector::logs::v1::{
    ExportLogsServiceRequest, ExportLogsServiceResponse,
};
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{
    AnyValue, InstrumentationScope, KeyValue, KeyValueList,
};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message;
use tumult_ingest::{http, EntityEvents, EntitySettings, IngestWriter};
use tumult_lake::Store;

const TS: u64 = 1_774_980_000_000_000_000;
const TOKEN: &str = "kro_secret-ingest-token";

fn kv(key: &str, value: Value) -> KeyValue {
    KeyValue {
        key: key.into(),
        value: Some(AnyValue { value: Some(value) }),
        key_strindex: 0,
    }
}

fn text(key: &str, value: &str) -> KeyValue {
    kv(key, Value::StringValue(value.into()))
}

fn pod_id() -> Value {
    Value::KvlistValue(KeyValueList {
        values: vec![text("k8s.pod.uid", "u-1")],
    })
}

fn legacy(event_type: &str, ts: u64, mut attributes: Vec<KeyValue>) -> LogRecord {
    attributes.push(text("otel.entity.event.type", event_type));
    attributes.push(text("otel.entity.type", "k8s.pod"));
    attributes.push(kv("otel.entity.id", pod_id()));
    LogRecord {
        time_unix_nano: ts,
        attributes,
        ..LogRecord::default()
    }
}

fn legacy_state(ts: u64) -> LogRecord {
    legacy(
        "entity_state",
        ts,
        vec![kv("otel.entity.interval", Value::IntValue(10_000))],
    )
}

fn spec_delete(ts: u64) -> LogRecord {
    LogRecord {
        time_unix_nano: ts,
        event_name: "entity.delete".into(),
        attributes: vec![
            text("entity.type", "k8s.pod"),
            kv("entity.id", pod_id()),
            text("entity.delete.reason", "scaled_down"),
        ],
        ..LogRecord::default()
    }
}

fn ordinary() -> LogRecord {
    LogRecord {
        time_unix_nano: TS,
        severity_text: "INFO".into(),
        body: Some(AnyValue {
            value: Some(Value::StringValue("experiment started".into())),
        }),
        ..LogRecord::default()
    }
}

fn export(records: Vec<LogRecord>) -> ExportLogsServiceRequest {
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: vec![
                    text("service.name", "otelcol"),
                    text("deployment.environment.name", "prod"),
                ],
                dropped_attributes_count: 0,
                entity_refs: vec![],
            }),
            scope_logs: vec![ScopeLogs {
                scope: Some(InstrumentationScope::default()),
                log_records: records,
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    }
}

struct Served {
    _dir: tempfile::TempDir,
    store: Store,
    addr: std::net::SocketAddr,
    _server: tokio::task::JoinHandle<()>,
}

/// The OTLP/HTTP listener of a daemon whose `KRONIKA_ENTITY_EVENTS` is
/// `flag`, with the ingest token set.
async fn serve(flag: Option<&str>) -> Served {
    serve_with(EntitySettings {
        flag,
        ingest_token: Some(TOKEN),
        ..EntitySettings::default()
    })
    .await
}

async fn serve_with(settings: EntitySettings<'_>) -> Served {
    let dir = tempfile::TempDir::new().unwrap();
    let store = Store::open(&dir.path().join("k.duckdb")).unwrap();
    let (ingest, _task) = IngestWriter::spawn(store.writer().unwrap(), 16);
    let ingest = ingest.with_entity_events(EntityEvents::from_settings(&settings));
    let app = http::router_with_token(ingest, Some(TOKEN.into()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Served {
        _dir: dir,
        store,
        addr,
        _server: server,
    }
}

impl Served {
    /// Send `records` as one export; the answer of the daemon.
    async fn post(&self, records: Vec<LogRecord>) -> ExportLogsServiceResponse {
        let response = reqwest::Client::new()
            .post(format!("http://{}/v1/logs", self.addr))
            .header("content-type", "application/x-protobuf")
            .header("authorization", format!("Bearer {TOKEN}"))
            .body(export(records).encode_to_vec())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        ExportLogsServiceResponse::decode(response.bytes().await.unwrap()).unwrap()
    }

    fn rows(&self, sql: &str) -> Vec<serde_json::Value> {
        self.store
            .read_only()
            .unwrap()
            .query_json_rows(sql)
            .unwrap()
    }

    fn entities(&self) -> Vec<serde_json::Value> {
        self.rows(
            "SELECT entity_key, environment, wire_form, observer, interval_s, \
             valid_from_ns, last_seen_ns, delete_reason, liveness \
             FROM entities_current",
        )
    }

    /// Every stored column of every log record.
    fn logs(&self) -> Vec<serde_json::Value> {
        self.rows(
            "SELECT ts_ns, severity_text, body, trace_id, span_id, service_name, \
             CAST(log_attrs AS VARCHAR) AS log_attrs, \
             CAST(resource_attrs AS VARCHAR) AS resource_attrs, event_name, \
             CAST(scope_attrs AS VARCHAR) AS scope_attrs \
             FROM logs ORDER BY ts_ns, body",
        )
    }
}

#[tokio::test]
async fn heartbeats_over_http_keep_one_row_and_never_reach_logs() {
    let served = serve(Some("1")).await;
    for beat in 0..3 {
        served
            .post(vec![legacy_state(TS + beat * 10_000_000_000), ordinary()])
            .await;
    }
    let versions = served.rows("SELECT count(*) AS n FROM entity_versions");
    assert_eq!(versions[0]["n"], 1, "identical heartbeats open no new row");
    let entities = served.entities();
    assert_eq!(entities.len(), 1, "{entities:?}");
    let entity = &entities[0];
    assert_eq!(entity["entity_key"], "k8s.pod|k8s.pod.uid=u-1");
    assert_eq!(entity["environment"], "prod");
    assert_eq!(entity["wire_form"], "legacy");
    assert_eq!(entity["interval_s"], 10);
    assert_eq!(entity["valid_from_ns"], TS);
    assert_eq!(entity["last_seen_ns"], TS + 20_000_000_000);
    let observer = entity["observer"].as_str().unwrap();
    assert!(!observer.contains("kro_secret"), "{observer}");
    assert_ne!(observer, "anonymous");

    let logs = served.logs();
    assert_eq!(logs.len(), 3, "only the ordinary records: {logs:?}");
    assert!(logs.iter().all(|l| l["body"] == "experiment started"));
}

#[tokio::test]
async fn a_delete_over_http_marks_the_entity_gone() {
    let served = serve(Some("1")).await;
    served.post(vec![legacy_state(TS)]).await;
    served.post(vec![spec_delete(TS + 1_000_000_000)]).await;
    let entities = served.entities();
    assert_eq!(entities.len(), 1, "{entities:?}");
    assert_eq!(entities[0]["liveness"], "gone");
    assert_eq!(entities[0]["delete_reason"], "scaled_down");
    assert!(served.logs().is_empty());
}

/// With the flag off the records are stored as the log records they are
/// on the wire, and no entity row appears.
#[tokio::test]
async fn without_the_flag_entity_events_are_stored_as_logs() {
    for flag in [None, Some("0")] {
        let served = serve(flag).await;
        served
            .post(vec![legacy_state(TS), spec_delete(TS + 1), ordinary()])
            .await;
        assert!(served.entities().is_empty());
        let logs = served.logs();
        assert_eq!(logs.len(), 3, "{logs:?}");
        assert_eq!(logs[2]["event_name"], "entity.delete");
        let legacy_attrs = logs[0]["log_attrs"].as_str().unwrap();
        assert!(
            legacy_attrs.contains("otel.entity.event.type"),
            "{legacy_attrs}"
        );
    }
}

/// The flag changes nothing for an export that carries no entity event:
/// every stored column is the same with it and without it.
#[tokio::test]
async fn ordinary_logs_are_stored_the_same_with_and_without_the_flag() {
    let off = serve(None).await;
    let on = serve(Some("1")).await;
    let mut attributed = ordinary();
    attributed.attributes = vec![text("k", "v")];
    attributed.trace_id = vec![0xab; 16];
    attributed.event_name = "experiment.started".into();
    for served in [&off, &on] {
        served.post(vec![ordinary(), attributed.clone()]).await;
    }
    assert_eq!(off.logs().len(), 2);
    assert_eq!(off.logs(), on.logs());
    assert!(on.entities().is_empty());
}

/// A legacy state event that names no id; a pod has nothing to fall back
/// on, so it cannot become a row.
fn without_identity() -> LogRecord {
    LogRecord {
        time_unix_nano: TS,
        attributes: vec![
            text("otel.entity.event.type", "entity_state"),
            text("otel.entity.type", "k8s.pod"),
        ],
        ..LogRecord::default()
    }
}

fn of_unknown_kind() -> LogRecord {
    LogRecord {
        time_unix_nano: TS,
        attributes: vec![
            text("otel.entity.event.type", "entity_renamed"),
            text("otel.entity.type", "k8s.pod"),
            kv("otel.entity.id", pod_id()),
        ],
        ..LogRecord::default()
    }
}

/// An export is accepted whatever its entity events are, so the answer is
/// where a sender learns that some were not stored: how many, and why.
#[tokio::test]
async fn entity_events_that_are_not_stored_are_reported_to_the_sender() {
    let served = serve(Some("1")).await;
    let clean = served.post(vec![legacy_state(TS), ordinary()]).await;
    assert_eq!(clean.partial_success, None, "nothing was left out");

    let answer = served
        .post(vec![
            legacy_state(TS + 1),
            without_identity(),
            ordinary(),
            of_unknown_kind(),
            without_identity(),
        ])
        .await;
    let partial = answer.partial_success.expect("three events were dropped");
    assert_eq!(partial.rejected_log_records, 3);
    assert_eq!(
        partial.error_message,
        "entity events not stored: unknown_kind=1, no_identity=2"
    );
    assert_eq!(
        served.entities().len(),
        1,
        "the rest of the export is stored"
    );
    assert_eq!(served.logs().len(), 2);
}

/// An entity refused for lack of room is not stored either, and the
/// sender is told the same way.
#[tokio::test]
async fn an_entity_refused_at_the_limit_is_reported_to_the_sender() {
    let served = serve_with(EntitySettings {
        flag: Some("1"),
        ingest_token: Some(TOKEN),
        max_per_environment: Some("1"),
        ..EntitySettings::default()
    })
    .await;
    // Received now, so the first entity is live and cannot make room.
    let now = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    )
    .unwrap();
    assert_eq!(
        served.post(vec![legacy_state(now)]).await.partial_success,
        None
    );

    let mut second = legacy_state(now);
    second.attributes.retain(|kv| kv.key != "otel.entity.id");
    second.attributes.push(kv(
        "otel.entity.id",
        Value::KvlistValue(KeyValueList {
            values: vec![text("k8s.pod.uid", "u-2")],
        }),
    ));
    let answer = served.post(vec![second, without_identity()]).await;
    let partial = answer.partial_success.expect("one refused, one dropped");
    assert_eq!(partial.rejected_log_records, 2);
    assert_eq!(
        partial.error_message,
        "entity events not stored: no_identity=1, entity_limit=1"
    );
    assert_eq!(served.entities().len(), 1);
}

/// With the flag off every record is stored as a log record: nothing is
/// left out and nothing is reported.
#[tokio::test]
async fn without_the_flag_nothing_is_reported_as_rejected() {
    let served = serve(None).await;
    let answer = served
        .post(vec![without_identity(), of_unknown_kind(), ordinary()])
        .await;
    assert_eq!(answer.partial_success, None);
    assert_eq!(served.logs().len(), 3);
}
