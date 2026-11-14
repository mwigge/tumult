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
use tumult_lake::{
    EntityEventKind, EntityEventRow, EntityEvictions, EntityLimits, EntityOutcome, EvictionReason,
    Store, WireForm, ENTITIES_PER_ENVIRONMENT,
};

use super::*;
use crate::writer::EntityPurgeRequest;
use crate::writer::{Batch, IngestWriter};

pub(crate) const TS: u64 = 1_774_980_000_000_000_000;
/// The time of receipt given to conversions: 2100-01-01, ahead of the
/// clock, so that what a test stores is neither expired nor past the
/// retention window whenever the test runs.
const RECEIVED: i64 = 4_102_444_800_000_000_000;

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
    EntityEvents::from_settings(&on())
}

/// The flag set and nothing else.
fn on() -> EntitySettings<'static> {
    EntitySettings {
        flag: Some("1"),
        ..EntitySettings::default()
    }
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

fn with_flag(flag: Option<&str>) -> Option<EntityEvents> {
    EntityEvents::from_settings(&EntitySettings {
        flag,
        ..EntitySettings::default()
    })
}

#[test]
fn the_flag_takes_the_usual_spellings_of_true_and_false() {
    let (logs, _guard) = crate::test_logs::capture();
    for on in [
        "1", "true", "TRUE", "True", "yes", "YES", "on", "On", " 1", "true\n",
    ] {
        assert!(with_flag(Some(on)).is_some(), "{on:?}");
    }
    for off in [
        None,
        Some(""),
        Some("  "),
        Some("0"),
        Some("false"),
        Some("FALSE"),
        Some("no"),
        Some("No"),
        Some("off"),
        Some("OFF"),
    ] {
        assert!(with_flag(off).is_none(), "{off:?}");
    }
    assert_eq!(logs.text(), "", "a recognised value is not worth a line");
}

/// A value that is neither leaves entity events off, and says so: a
/// feature that silently stays off looks like a sender that sends nothing.
#[test]
fn an_unrecognised_flag_value_leaves_the_feature_off_and_is_logged() {
    for odd in ["2", "enabled", "tru", "y"] {
        let (logs, _guard) = crate::test_logs::capture();
        assert!(with_flag(Some(odd)).is_none(), "{odd}");
        let text = logs.text();
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(text.contains("WARN"), "{text}");
        assert!(
            text.contains("config.key=\"KRONIKA_ENTITY_EVENTS\""),
            "{text}"
        );
        assert!(text.contains(&format!("config.value=\"{odd}\"")), "{text}");
    }
}

/// The value is the operator's, but it is still cut and escaped before it
/// is written.
#[test]
fn a_long_unrecognised_flag_value_is_cut_in_the_log() {
    let (logs, _guard) = crate::test_logs::capture();
    let long = "x".repeat(500);
    assert!(with_flag(Some(&long)).is_none());
    let text = logs.text();
    assert!(text.contains(&"x".repeat(64)), "{text}");
    assert!(!text.contains(&"x".repeat(65)), "{text}");
}

fn with_attribute_allow(list: &str) -> Vec<String> {
    EntityEvents::from_settings(&EntitySettings {
        attribute_allow: Some(list),
        ..on()
    })
    .unwrap()
    .options()
    .attribute_allow
    .clone()
}

/// `*` alone, or the start of a key that names no whole namespace, keeps
/// far more than an operator who typed it may have meant: annotations and
/// selectors of every kind. It is honoured, and said at start.
#[test]
fn an_attribute_pattern_without_a_namespace_is_honoured_and_logged() {
    for broad in ["*", "k8s*", "h*", " * "] {
        let (logs, _guard) = crate::test_logs::capture();
        let kept = with_attribute_allow(&format!("k8s.service.selector.*,{broad}"));
        assert_eq!(kept, vec!["k8s.service.selector.*", broad.trim()]);
        let text = logs.text();
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(text.contains("WARN"), "{text}");
        assert!(text.contains("names no namespace"), "{text}");
        assert!(
            text.contains("config.key=\"KRONIKA_ENTITY_ATTRIBUTE_ALLOW\""),
            "{text}"
        );
        assert!(
            text.contains(&format!("config.value=\"{}\"", broad.trim())),
            "{text}"
        );
    }
}

/// A whole key, and the start of one that ends inside a namespace, are
/// what the variable is for.
#[test]
fn an_attribute_pattern_within_a_namespace_is_not_worth_a_line() {
    let (logs, _guard) = crate::test_logs::capture();
    with_attribute_allow("k8s.service.selector.*,k8s.*,schedule,host.labels_json");
    assert_eq!(logs.text(), "");
}

/// With the flag off the list is not read, so nothing is said about it.
#[test]
fn a_broad_attribute_pattern_is_not_logged_with_the_flag_off() {
    let (logs, _guard) = crate::test_logs::capture();
    let off = EntityEvents::from_settings(&EntitySettings {
        attribute_allow: Some("*"),
        ..EntitySettings::default()
    });
    assert!(off.is_none());
    assert_eq!(logs.text(), "");
}

#[test]
fn the_label_allow_list_is_a_comma_separated_list_of_names() {
    let labels = |list: Option<&str>| {
        EntityEvents::from_settings(&EntitySettings {
            label_allow: list,
            ..on()
        })
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
fn the_attribute_allow_list_is_a_comma_separated_list_of_keys() {
    let attributes = |list: Option<&str>| {
        EntityEvents::from_settings(&EntitySettings {
            attribute_allow: list,
            ..on()
        })
        .unwrap()
        .options()
        .attribute_allow
        .clone()
    };
    assert!(
        attributes(None).is_empty(),
        "nothing more is kept by default"
    );
    assert_eq!(
        attributes(Some("k8s.service.annotation.owner, k8s.pod.label.* ,")),
        vec!["k8s.service.annotation.owner", "k8s.pod.label.*"]
    );
}

/// A label and an annotation in a description reach the store only when
/// the operator named them.
#[tokio::test]
async fn a_stored_description_holds_only_listed_keys() {
    let labelled = || {
        let mut export = host_export(false);
        let record = &mut export.resource_logs[0].scope_logs[0].log_records[0];
        record
            .attributes
            .retain(|kv| kv.key != "entity.description");
        record.attributes.push(kv(
            "entity.description",
            map(vec![
                text("host.name", "node-a"),
                text("k8s.node.label.team", "payments"),
                text("k8s.node.label.rack", "r1"),
                text("k8s.node.annotation.owner", "ops@example.test"),
                text("k8s.node.annotation.config", "{\"password\":\"hunter2\"}"),
            ]),
        ));
        export
    };
    let description = |store: &Store| -> serde_json::Value {
        let stored = rows(store, ENTITIES);
        serde_json::from_str(stored[0]["description"].as_str().unwrap()).unwrap()
    };

    let filtered = tumult_otlp::entity_event_keys_filtered_total();
    let (_dir, store, ingest) = ingest_with(enabled());
    let batch = ingest.logs_batch(&labelled(), RECEIVED);
    ingest.write(batch).await.unwrap();
    assert_eq!(description(&store), json!({"host.name": "node-a"}));
    assert!(tumult_otlp::entity_event_keys_filtered_total() >= filtered + 4);

    let named = EntityEvents::from_settings(&EntitySettings {
        label_allow: Some("team"),
        attribute_allow: Some("k8s.node.annotation.owner"),
        ..on()
    });
    let (_dir, store, ingest) = ingest_with(named);
    let batch = ingest.logs_batch(&labelled(), RECEIVED);
    ingest.write(batch).await.unwrap();
    assert_eq!(
        description(&store),
        json!({
            "host.name": "node-a",
            "k8s.node.label.team": "payments",
            "k8s.node.annotation.owner": "ops@example.test",
        })
    );
}

#[test]
fn the_filtered_keys_counter_is_exported() {
    let mut text = String::new();
    render_prometheus(&mut text);
    assert!(
        text.contains("# TYPE tumultd_entity_event_keys_filtered_total counter"),
        "{text}"
    );
    assert!(
        text.contains("\ntumultd_entity_event_keys_filtered_total "),
        "{text}"
    );
}

#[test]
fn the_observer_names_the_ingest_token_without_revealing_it() {
    let observer = |token: Option<&str>| {
        EntityEvents::from_settings(&EntitySettings {
            ingest_token: token,
            ..on()
        })
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
    let Batch::LogsAndEntities { logs, entities, .. } =
        ingest.logs_batch(&host_export(true), RECEIVED)
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

/// The observer of a row changes when an ingest token is introduced or
/// removed. A delete that arrives after such a change is a delete like any
/// other.
#[tokio::test]
async fn a_delete_is_accepted_after_the_token_setting_changed() {
    let (_dir, store, ingest) = ingest_with(enabled());
    let reported = ingest.logs_batch(&host_export(false), RECEIVED);
    ingest.write(reported).await.unwrap();

    let with_token = EntityEvents::from_settings(&EntitySettings {
        ingest_token: Some("kro_secret"),
        ..on()
    });
    let mut export = host_export(false);
    let record = &mut export.resource_logs[0].scope_logs[0].log_records[0];
    record.event_name = "entity.delete".into();
    record.time_unix_nano = TS + 1;
    let deleted = logs_batch(with_token.as_ref(), &export, RECEIVED + 1);
    ingest.write(deleted).await.unwrap();

    let entity = rows(&store, "SELECT observer, liveness FROM entities_current");
    assert_eq!(entity.len(), 1);
    assert_eq!(entity[0]["liveness"], "gone");
    assert_eq!(entity[0]["observer"], "anonymous");
}

#[test]
fn every_outcome_is_counted_under_its_own_name() {
    let before = outcome_count(EntityOutcome::DeleteOutOfOrder);
    let opened = outcome_count(EntityOutcome::Opened);
    note_outcomes(&[
        EntityOutcome::Opened,
        EntityOutcome::DeleteOutOfOrder,
        EntityOutcome::DeleteOutOfOrder,
    ]);
    // Other tests of this binary store entities too: floors.
    assert!(outcome_count(EntityOutcome::DeleteOutOfOrder) >= before + 2);
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
        "tumultd_entity_events_total{outcome=\"entity_limit\"} ",
        "tumultd_entity_versions_pruned_total{reason=\"version_cap\"} ",
        "tumultd_entity_versions_pruned_total{reason=\"retention\"} ",
        "# TYPE tumultd_entity_versions gauge",
        "\ntumultd_entity_versions ",
        "tumultd_entity_events_dropped_total{reason=\"no_identity\"} ",
        "tumultd_entity_events_dropped_total{reason=\"malformed\"} ",
        "tumultd_entity_events_dropped_total{reason=\"export_limit\"} ",
    ] {
        assert!(text.contains(line), "{line}: {text}");
    }
    assert!(
        !text.contains("observer"),
        "no outcome is decided by the observer: {text}"
    );
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

fn limits_for(max_per_environment: Option<&str>) -> EntityLimits {
    EntityEvents::from_settings(&EntitySettings {
        max_per_environment,
        ..on()
    })
    .unwrap()
    .limits()
}

#[test]
fn the_entity_cap_is_a_positive_integer() {
    for unset in [None, Some(""), Some("  ")] {
        assert_eq!(limits_for(unset), EntityLimits::default(), "{unset:?}");
    }
    assert_eq!(
        EntityLimits::default().entities_per_environment,
        ENTITIES_PER_ENVIRONMENT
    );
    let three = limits_for(Some(" 3 "));
    assert_eq!(three.entities_per_environment, 3);
    assert_eq!(
        three.entities_total, 12,
        "the lake holds four times as many"
    );
    assert_eq!(
        three.versions_per_entity,
        EntityLimits::default().versions_per_entity
    );
}

#[test]
fn an_unusable_entity_cap_keeps_the_default_and_is_logged() {
    let (logs, _guard) = crate::test_logs::capture();
    assert_eq!(limits_for(Some("12")).entities_per_environment, 12);
    assert_eq!(logs.text(), "", "a usable value is not worth a line");
    for unusable in ["0", "-1", "many", "1e3"] {
        assert_eq!(
            limits_for(Some(unusable)),
            EntityLimits::default(),
            "{unusable:?}"
        );
    }
    let text = logs.text();
    assert_eq!(text.matches("WARN").count(), 4, "{text}");
    assert!(
        text.contains("config.key=\"KRONIKA_ENTITY_MAX_PER_ENVIRONMENT\""),
        "{text}"
    );
    assert!(text.contains("config.value=\"many\""), "{text}");
}

/// An entity beyond the cap is refused and counted; the export that named
/// it is accepted and its other records are stored.
#[tokio::test]
async fn an_entity_beyond_the_cap_is_refused_and_the_rest_of_the_export_is_stored() {
    let capped = EntityEvents::from_settings(&EntitySettings {
        max_per_environment: Some("1"),
        ..on()
    });
    let (_dir, store, ingest) = ingest_with(capped);
    let batch = ingest.logs_batch(&host_export(false), RECEIVED);
    ingest.write(batch).await.unwrap();

    let mut other = host_export(false);
    other.resource_logs[0].resource.as_mut().unwrap().attributes[0] = text("host.id", "another");
    let before = outcome_count(EntityOutcome::EntityLimit);
    let batch = ingest.logs_batch(&other, RECEIVED);
    ingest.write(batch).await.unwrap();

    assert!(outcome_count(EntityOutcome::EntityLimit) > before);
    let entities = rows(&store, ENTITIES);
    assert_eq!(entities.len(), 1, "{entities:?}");
    assert_eq!(entities[0]["host_id"], "9f2c");
    assert_eq!(rows(&store, LOGS).len(), 2);
}

#[test]
fn refused_entities_are_logged_at_a_bounded_rate() {
    let (logs, _guard) = crate::test_logs::capture();
    let limiter = LogLimiter::new(1, std::time::Duration::from_secs(600));
    warn_refused(&limiter, 0);
    assert_eq!(logs.text(), "", "nothing refused, nothing said");
    for _ in 0..5 {
        warn_refused(&limiter, 3);
    }
    let text = logs.text();
    assert_eq!(text.matches("WARN").count(), 1, "{text}");
    assert!(text.contains("entity.event.refused=3"), "{text}");
    assert!(
        text.contains("config.key=\"KRONIKA_ENTITY_MAX_PER_ENVIRONMENT\""),
        "{text}"
    );
}

#[test]
fn entities_past_the_window_are_looked_for_once_per_interval() {
    let last = AtomicI64::new(0);
    let now = 1_774_980_000_000_000_000;
    assert!(prune_due(&last, now), "never looked");
    assert!(!prune_due(&last, now + 1));
    assert!(!prune_due(&last, now + PRUNE_EVERY_NS - 1));
    assert!(prune_due(&last, now + PRUNE_EVERY_NS));
    assert!(!prune_due(&last, now + PRUNE_EVERY_NS + 1));
}

fn host_event(kind: EntityEventKind, id: &str, at_ns: i64) -> EntityEventRow {
    EntityEventRow {
        kind,
        ts_ns: at_ns,
        received_at_ns: at_ns,
        entity_type: "host".into(),
        entity_id: vec![("host.id".into(), id.into())],
        description: json!({}),
        relationships: json!([]),
        interval_s: Some(0),
        delete_reason: None,
        wire_form: WireForm::Spec,
        observer: "anonymous".into(),
        schema_url: None,
        environment: String::new(),
    }
}

fn gone(id: &str, at_ns: i64) -> [EntityEventRow; 2] {
    [
        host_event(EntityEventKind::State, id, at_ns),
        host_event(EntityEventKind::Delete, id, at_ns + 1),
    ]
}

#[test]
fn housekeeping_removes_entities_that_left_the_retention_window() {
    assert_eq!(RETENTION_DAYS, 30);
    let dir = tempfile::TempDir::new().unwrap();
    let store = Store::open(&dir.path().join("k.duckdb")).unwrap();
    let writer = store.writer().unwrap();
    let now = crate::now_ns();
    let day = 86_400_000_000_000;
    let mut events = Vec::new();
    events.extend(gone("left-31-days-ago", now - 31 * day));
    events.extend(gone("left-29-days-ago", now - 29 * day));
    events.push(host_event(
        EntityEventKind::State,
        "old-and-live",
        now - 400 * day,
    ));
    let limits = EntityLimits::default();
    writer.apply_entity_events(&events, &limits).unwrap();

    let before = versions_pruned(Pruned::Retention);
    let last = AtomicI64::new(0);
    housekeeping(&writer, &last, now);
    assert!(versions_pruned(Pruned::Retention) > before);
    assert_eq!(writer.entity_version_count().unwrap(), 2);

    // Not due again yet: what leaves the window now waits.
    writer
        .apply_entity_events(&gone("left-40-days-ago", now - 40 * day), &limits)
        .unwrap();
    housekeeping(&writer, &last, now + 1);
    assert_eq!(writer.entity_version_count().unwrap(), 3);
    housekeeping(&writer, &last, now + PRUNE_EVERY_NS);
    assert_eq!(writer.entity_version_count().unwrap(), 2);
    drop(writer);
    let left = rows(
        &store,
        "SELECT entity_id['host.id'] AS id FROM entity_versions ORDER BY id",
    );
    assert_eq!(left[0]["id"], "left-29-days-ago");
    assert_eq!(left[1]["id"], "old-and-live");
}

#[test]
fn rows_pruned_for_the_version_cap_are_counted() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = Store::open(&dir.path().join("k.duckdb")).unwrap();
    let writer = store.writer().unwrap();
    let before = versions_pruned(Pruned::VersionCap);
    after_write(
        &writer,
        &tumult_lake::EntityApplied {
            outcomes: vec![EntityOutcome::Opened],
            versions_pruned: 4,
            ..tumult_lake::EntityApplied::default()
        },
    );
    assert!(versions_pruned(Pruned::VersionCap) >= before + 4);
}

/// At the cap an entity nobody reports any more makes room for a new one:
/// the export is stored whole and the eviction is counted by its reason.
#[tokio::test]
async fn at_the_cap_a_silent_entity_makes_room_and_the_eviction_is_counted() {
    /// 2026-01-01: more than a day ago whenever the test runs.
    const LONG_AGO: i64 = 1_767_225_600_000_000_000;
    let capped = EntityEvents::from_settings(&EntitySettings {
        max_per_environment: Some("1"),
        ..on()
    });
    let (_dir, store, ingest) = ingest_with(capped);
    let batch = ingest.logs_batch(&host_export(false), LONG_AGO);
    ingest.write(batch).await.unwrap();

    let mut other = host_export(false);
    other.resource_logs[0].resource.as_mut().unwrap().attributes[0] = text("host.id", "another");
    let before: u64 = EvictionReason::ALL.into_iter().map(entities_evicted).sum();
    let batch = ingest.logs_batch(&other, RECEIVED);
    ingest.write(batch).await.unwrap();

    let after: u64 = EvictionReason::ALL.into_iter().map(entities_evicted).sum();
    assert!(after > before);
    let entities = rows(&store, ENTITIES);
    assert_eq!(entities.len(), 1, "{entities:?}");
    assert_eq!(entities[0]["host_id"], "another");
}

#[test]
fn evictions_are_counted_by_reason_and_exported() {
    let before = EvictionReason::ALL.map(entities_evicted);
    add_evicted(&EntityEvictions {
        gone: 1,
        expired: 2,
        unreported: 3,
    });
    let after = EvictionReason::ALL.map(entities_evicted);
    // Another test of this binary evicts too: floors.
    assert!(after[0] > before[0]);
    assert!(after[1] >= before[1] + 2);
    assert!(after[2] >= before[2] + 3);
    let mut text = String::new();
    render_prometheus(&mut text);
    assert!(
        text.contains("# TYPE tumultd_entity_evictions_total counter"),
        "{text}"
    );
    for reason in ["gone", "expired", "unreported"] {
        let line = format!("tumultd_entity_evictions_total{{reason=\"{reason}\"}} ");
        assert!(text.contains(&line), "{line}: {text}");
    }
}

#[test]
fn evictions_are_logged_at_a_bounded_rate() {
    let (logs, _guard) = crate::test_logs::capture();
    let limiter = LogLimiter::new(1, std::time::Duration::from_secs(600));
    log_evicted(&limiter, &EntityEvictions::default());
    assert_eq!(logs.text(), "", "nothing evicted, nothing said");
    let evicted = EntityEvictions {
        gone: 1,
        expired: 2,
        unreported: 3,
    };
    for _ in 0..5 {
        log_evicted(&limiter, &evicted);
    }
    let text = logs.text();
    assert_eq!(text.matches("INFO").count(), 1, "{text}");
    assert!(text.contains("entity.evicted.gone=1"), "{text}");
    assert!(text.contains("entity.evicted.expired=2"), "{text}");
    assert!(text.contains("entity.evicted.unreported=3"), "{text}");
}

/// An operator's purge goes through the writer task like every write, and
/// says in the log who removed what.
#[tokio::test]
async fn a_purge_runs_on_the_writer_and_is_written_to_the_log() {
    let (_dir, store, ingest) = ingest_with(enabled());
    let batch = ingest.logs_batch(&host_export(false), RECEIVED);
    ingest.write(batch).await.unwrap();
    assert_eq!(rows(&store, ENTITIES).len(), 1);

    let (logs, _guard) = crate::test_logs::capture();
    let other_type = EntityPurgeRequest {
        environment: String::new(),
        entity_type: Some("k8s.pod".into()),
        by: "ada".into(),
    };
    let purged = ingest.purge_entities(other_type).await.unwrap();
    assert_eq!((purged.entities, purged.versions), (0, 0));
    assert_eq!(
        rows(&store, ENTITIES).len(),
        1,
        "another type is left alone"
    );

    let hosts = EntityPurgeRequest {
        environment: String::new(),
        entity_type: Some("host".into()),
        by: "ada".into(),
    };
    let purged = ingest.purge_entities(hosts).await.unwrap();
    assert_eq!((purged.entities, purged.versions), (1, 1));
    assert_eq!(rows(&store, ENTITIES).len(), 0);
    assert_eq!(rows(&store, LOGS).len(), 1, "log records are not entities");

    let text = logs.text();
    assert_eq!(
        text.matches("INFO").count(),
        2,
        "one line per purge: {text}"
    );
    assert!(text.contains("user.name=\"ada\""), "{text}");
    assert!(text.contains("entity.type=\"host\""), "{text}");
    assert!(text.contains("deployment.environment.name=\"\""), "{text}");
    assert!(text.contains("entity.purged.entities=1"), "{text}");
    assert!(text.contains("entity.purged.versions=1"), "{text}");
}

#[tokio::test]
async fn a_purge_fails_when_the_writer_task_is_gone() {
    let request = EntityPurgeRequest {
        environment: "prod".into(),
        entity_type: None,
        by: "ada".into(),
    };
    let refused = IngestWriter::stopped_for_test()
        .purge_entities(request)
        .await;
    assert!(
        matches!(&refused, Err(crate::IngestError::Channel(_))),
        "{refused:?}"
    );
}
