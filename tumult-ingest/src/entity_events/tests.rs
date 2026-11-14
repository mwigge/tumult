//! The flag, the observer name and the route of a logs export, with the
//! fixtures the carrier tests (gRPC, OTel-Arrow) share.

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{
    AnyValue, InstrumentationScope, KeyValue, KeyValueList,
};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::resource::v1::Resource;
use serde_json::json;
use tumult_lake::{EntityOutcome, Store};

use super::*;
use crate::writer::{Batch, IngestWriter};

pub(crate) const TS: u64 = 1_774_980_000_000_000_000;
const RECEIVED: i64 = 1_774_980_000_500_000_000;

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

fn map(values: Vec<KeyValue>) -> Value {
    Value::KvlistValue(KeyValueList { values })
}

/// The `host` state event of `host_metrics`, in either wire form: a type,
/// a description map, an interval and no id.
fn host_state(legacy: bool) -> LogRecord {
    let description = map(vec![text("host.name", "node-a"), text("os.type", "linux")]);
    if legacy {
        return LogRecord {
            time_unix_nano: TS,
            attributes: vec![
                text("otel.entity.event.type", "entity_state"),
                text("otel.entity.type", "host"),
                kv("otel.entity.attributes", description),
                kv("otel.entity.interval", Value::IntValue(10_000)),
            ],
            ..LogRecord::default()
        };
    }
    LogRecord {
        time_unix_nano: TS,
        event_name: "entity.state".into(),
        attributes: vec![
            text("entity.type", "host"),
            kv("entity.description", description),
            kv("entity.report.interval", Value::IntValue(10)),
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

/// What a Collector with `host_metrics` and `resourcedetection` exports,
/// next to an ordinary log record of another scope.
pub(crate) fn host_export(legacy: bool) -> ExportLogsServiceRequest {
    let marked = InstrumentationScope {
        name: "hostmetricsreceiver".into(),
        attributes: vec![kv("otel.entity.event_as_log", Value::BoolValue(true))],
        ..InstrumentationScope::default()
    };
    let plain = InstrumentationScope {
        name: "app".into(),
        ..InstrumentationScope::default()
    };
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: vec![text("host.id", "9f2c"), text("service.name", "otelcol")],
                dropped_attributes_count: 0,
                entity_refs: vec![],
            }),
            scope_logs: vec![
                ScopeLogs {
                    scope: Some(marked),
                    log_records: vec![host_state(legacy)],
                    schema_url: String::new(),
                },
                ScopeLogs {
                    scope: Some(plain),
                    log_records: vec![ordinary()],
                    schema_url: String::new(),
                },
            ],
            schema_url: String::new(),
        }],
    }
}

/// The flag set, no label allowed, no ingest token.
pub(crate) fn enabled() -> Option<EntityEvents> {
    EntityEvents::from_settings(Some("1"), None, None)
}

/// A store with its writer task, entity events on or off.
pub(crate) fn ingest_with(
    entities: Option<EntityEvents>,
) -> (tempfile::TempDir, Store, IngestWriter) {
    let dir = tempfile::TempDir::new().unwrap();
    let store = Store::open(&dir.path().join("k.duckdb")).unwrap();
    let (ingest, _task) = IngestWriter::spawn(store.writer().unwrap(), 8);
    (dir, store, ingest.with_entity_events(entities))
}

pub(crate) fn rows(store: &Store, sql: &str) -> Vec<serde_json::Value> {
    store.read_only().unwrap().query_json_rows(sql).unwrap()
}

pub(crate) const ENTITIES: &str = "SELECT entity_type, entity_id['host.id'] AS host_id, \
     wire_form, observer, interval_s, CAST(description AS VARCHAR) AS description \
     FROM entity_versions ORDER BY valid_from_ns";
pub(crate) const LOGS: &str = "SELECT body, event_name, service_name FROM logs ORDER BY body";

/// The one host entity of [`host_export`] and its ordinary record, and
/// nothing else.
pub(crate) fn assert_host_entity_and_one_log(store: &Store, wire_form: &str) {
    let entities = rows(store, ENTITIES);
    assert_eq!(entities.len(), 1, "{entities:?}");
    assert_eq!(entities[0]["entity_type"], "host");
    assert_eq!(entities[0]["host_id"], "9f2c");
    assert_eq!(entities[0]["wire_form"], wire_form);
    assert_eq!(entities[0]["observer"], "anonymous");
    assert_eq!(entities[0]["interval_s"], 10);
    let description: serde_json::Value =
        serde_json::from_str(entities[0]["description"].as_str().unwrap()).unwrap();
    assert_eq!(
        description,
        json!({"host.name": "node-a", "os.type": "linux"})
    );
    let logs = rows(store, LOGS);
    assert_eq!(logs.len(), 1, "an entity event is not also a log: {logs:?}");
    assert_eq!(logs[0]["body"], "experiment started");
}

/// Both records in `logs`, none in `entity_versions`: what the daemon did
/// before it knew entity events.
pub(crate) fn assert_two_logs_and_no_entity(store: &Store) {
    assert!(rows(store, ENTITIES).is_empty());
    let logs = rows(store, LOGS);
    assert_eq!(logs.len(), 2, "{logs:?}");
    assert_eq!(logs[0]["body"], "");
    assert_eq!(logs[1]["body"], "experiment started");
}

#[test]
fn the_flag_is_off_unless_set_to_one_or_true() {
    for off in [
        None,
        Some(""),
        Some("0"),
        Some("false"),
        Some("yes"),
        Some(" 1"),
    ] {
        assert!(
            EntityEvents::from_settings(off, None, None).is_none(),
            "{off:?}"
        );
    }
    for on in ["1", "true"] {
        assert!(
            EntityEvents::from_settings(Some(on), None, None).is_some(),
            "{on}"
        );
    }
}

#[test]
fn the_label_allow_list_is_a_comma_separated_list_of_names() {
    let labels = |list: Option<&str>| {
        EntityEvents::from_settings(Some("1"), list, None)
            .unwrap()
            .options()
            .label_allow
            .clone()
    };
    assert!(labels(None).is_empty(), "no label is kept by default");
    assert!(labels(Some(" , ")).is_empty());
    assert_eq!(
        labels(Some("app.kubernetes.io/name, team ,,tier")),
        vec!["app.kubernetes.io/name", "team", "tier"]
    );
}

#[test]
fn the_observer_names_the_ingest_token_without_revealing_it() {
    let observer = |token: Option<&str>| {
        EntityEvents::from_settings(Some("1"), None, token)
            .unwrap()
            .options()
            .observer
            .clone()
    };
    assert_eq!(observer(None), "anonymous");
    let named = observer(Some("kro_secret-ingest-token"));
    assert_ne!(named, "anonymous");
    assert!(!named.contains("kro_secret"), "{named}");
    // One shared token is one observer: rotating it must not orphan the
    // entities reported under the old one.
    assert_eq!(named, observer(Some("kro_other-token")));
}

#[tokio::test]
async fn without_the_flag_a_logs_export_is_the_batch_it_always_was() {
    let (_dir, _store, ingest) = ingest_with(None);
    let export = host_export(false);
    let Batch::Logs(logs) = ingest.logs_batch(&export, RECEIVED) else {
        panic!("not a logs batch");
    };
    assert_eq!(
        format!("{logs:?}"),
        format!("{:?}", tumult_otlp::logs_request_to_rows(&export, RECEIVED))
    );
    assert_eq!(logs.len(), 2);
}

#[tokio::test]
async fn with_the_flag_an_export_without_entity_events_is_still_a_logs_batch() {
    let (_dir, _store, ingest) = ingest_with(enabled());
    let mut export = host_export(false);
    export.resource_logs[0].scope_logs.remove(0);
    let Batch::Logs(logs) = ingest.logs_batch(&export, RECEIVED) else {
        panic!("not a logs batch");
    };
    assert_eq!(
        format!("{logs:?}"),
        format!("{:?}", tumult_otlp::logs_request_to_rows(&export, RECEIVED))
    );
}

#[tokio::test]
async fn with_the_flag_entity_events_leave_the_logs_batch() {
    let (_dir, _store, ingest) = ingest_with(enabled());
    let Batch::LogsAndEntities { logs, entities } = ingest.logs_batch(&host_export(true), RECEIVED)
    else {
        panic!("not a logs-and-entities batch");
    };
    assert_eq!(logs.len(), 1);
    assert_eq!(entities.len(), 1);
    assert_eq!(entities[0].observer, "anonymous");
}

#[tokio::test]
async fn a_written_batch_stores_the_entity_and_the_log_together() {
    for (legacy, wire_form) in [(false, "spec"), (true, "legacy")] {
        let (_dir, store, ingest) = ingest_with(enabled());
        let batch = ingest.logs_batch(&host_export(legacy), RECEIVED);
        ingest.write(batch).await.unwrap();
        assert_host_entity_and_one_log(&store, wire_form);
    }
}

#[test]
fn every_outcome_is_counted_under_its_own_name() {
    let before = outcome_count(EntityOutcome::DeleteForeignObserver);
    let opened = outcome_count(EntityOutcome::Opened);
    note_outcomes(&[
        EntityOutcome::Opened,
        EntityOutcome::DeleteForeignObserver,
        EntityOutcome::DeleteForeignObserver,
    ]);
    // Other tests of this binary store entities too: floors.
    assert!(outcome_count(EntityOutcome::DeleteForeignObserver) >= before + 2);
    assert!(outcome_count(EntityOutcome::Opened) > opened);

    let mut text = String::new();
    render_prometheus(&mut text);
    assert!(
        text.contains("# TYPE tumultd_entity_events_total counter"),
        "{text}"
    );
    assert!(
        text.contains("# TYPE tumultd_entity_events_dropped_total counter"),
        "{text}"
    );
    for line in [
        "tumultd_entity_events_total{outcome=\"opened\"} ",
        "tumultd_entity_events_total{outcome=\"refreshed\"} ",
        "tumultd_entity_events_total{outcome=\"tombstoned\"} ",
        "tumultd_entity_events_total{outcome=\"stale_state\"} ",
        "tumultd_entity_events_total{outcome=\"delete_unknown\"} ",
        "tumultd_entity_events_total{outcome=\"delete_repeated\"} ",
        "tumultd_entity_events_total{outcome=\"delete_out_of_order\"} ",
        "tumultd_entity_events_total{outcome=\"delete_foreign_observer\"} ",
        "tumultd_entity_events_dropped_total{reason=\"no_identity\"} ",
        "tumultd_entity_events_dropped_total{reason=\"malformed\"} ",
    ] {
        assert!(text.contains(line), "{line}: {text}");
    }
}

#[tokio::test]
async fn an_event_without_identity_is_dropped_counted_and_not_logged_as_a_record() {
    let (_dir, store, ingest) = ingest_with(enabled());
    let mut export = host_export(false);
    // No host.id on the resource: nothing identifies the host.
    export.resource_logs[0]
        .resource
        .as_mut()
        .unwrap()
        .attributes
        .retain(|kv| kv.key != "host.id");
    let before = tumult_otlp::entity_events_dropped(tumult_otlp::EntityDrop::NoIdentity);
    let batch = ingest.logs_batch(&export, RECEIVED);
    ingest.write(batch).await.unwrap();
    assert!(
        tumult_otlp::entity_events_dropped(tumult_otlp::EntityDrop::NoIdentity) > before,
        "the drop is counted"
    );
    assert!(rows(&store, ENTITIES).is_empty());
    let logs = rows(&store, LOGS);
    assert_eq!(logs.len(), 1, "{logs:?}");
    assert_eq!(logs[0]["body"], "experiment started");
}
