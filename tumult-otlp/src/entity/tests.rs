//! Entity-event normalisation tests, with fixtures for both wire forms as
//! Collector contrib 0.165.0 writes them.

use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{
    AnyValue, ArrayValue, InstrumentationScope, KeyValue, KeyValueList,
};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::resource::v1::Resource;
use serde_json::json;
use tumult_lake::{EntityEventKind, EntityEventRow, WireForm, ENTITY_INTERVAL_MAX_S};

use super::*;

mod bounds;
mod filter;

const TS: u64 = 1_774_980_000_000_000_000;
const RECEIVED: i64 = 1_774_980_000_500_000_000;
const SCHEMA: &str = "https://opentelemetry.io/schemas/1.40.0";

fn any(value: Value) -> AnyValue {
    AnyValue { value: Some(value) }
}

fn kv(key: &str, value: Value) -> KeyValue {
    KeyValue {
        key: key.into(),
        value: Some(any(value)),
        key_strindex: 0,
    }
}

fn text(key: &str, value: &str) -> KeyValue {
    kv(key, Value::StringValue(value.into()))
}

fn map(values: Vec<KeyValue>) -> Value {
    Value::KvlistValue(KeyValueList { values })
}

fn list(values: Vec<Value>) -> Value {
    Value::ArrayValue(ArrayValue {
        values: values.into_iter().map(any).collect(),
    })
}

fn options() -> EntityOptions {
    EntityOptions {
        observer: "tok:test".into(),
        label_allow: vec![],
        // The fixtures use `test.` keys where the kept keys do not matter.
        attribute_allow: vec!["test.*".into()],
    }
}

fn marked_scope() -> Option<InstrumentationScope> {
    Some(InstrumentationScope {
        attributes: vec![kv("otel.entity.event_as_log", Value::BoolValue(true))],
        ..InstrumentationScope::default()
    })
}

fn resource(attrs: &[(&str, &str)]) -> Option<Resource> {
    Some(Resource {
        attributes: attrs.iter().map(|(k, v)| text(k, v)).collect(),
        dropped_attributes_count: 0,
        entity_refs: vec![],
    })
}

/// One resource, one scope.
fn request(
    resource: Option<Resource>,
    scope: Option<InstrumentationScope>,
    records: Vec<LogRecord>,
) -> ExportLogsServiceRequest {
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource,
            scope_logs: vec![ScopeLogs {
                scope,
                log_records: records,
                schema_url: SCHEMA.into(),
            }],
            schema_url: String::new(),
        }],
    }
}

fn pod_id() -> Value {
    map(vec![text("k8s.pod.uid", "u-1")])
}

fn pod_description() -> Value {
    map(vec![
        text("k8s.pod.name", "checkout-7d9"),
        kv("test.restarts", Value::IntValue(3)),
        kv("test.ready", Value::BoolValue(true)),
        kv(
            "test.limits",
            map(vec![kv(
                "cpu",
                list(vec![
                    Value::DoubleValue(0.5),
                    Value::StringValue("m".into()),
                ]),
            )]),
        ),
    ])
}

fn pod_description_json() -> serde_json::Value {
    json!({
        "k8s.pod.name": "checkout-7d9",
        "test.restarts": 3,
        "test.ready": true,
        "test.limits": {"cpu": [0.5, "m"]},
    })
}

fn spec_record(event_name: &str, attributes: Vec<KeyValue>) -> LogRecord {
    LogRecord {
        time_unix_nano: TS,
        event_name: event_name.into(),
        attributes,
        ..LogRecord::default()
    }
}

fn spec_state() -> LogRecord {
    spec_record(
        "entity.state",
        vec![
            text("entity.type", "k8s.pod"),
            kv("entity.id", pod_id()),
            kv("entity.description", pod_description()),
            kv("entity.report.interval", Value::IntValue(300)),
        ],
    )
}

fn legacy_record(event_type: &str, mut attributes: Vec<KeyValue>) -> LogRecord {
    attributes.insert(0, text("otel.entity.event.type", event_type));
    LogRecord {
        time_unix_nano: TS,
        attributes,
        ..LogRecord::default()
    }
}

fn legacy_state() -> LogRecord {
    legacy_record(
        "entity_state",
        vec![
            text("otel.entity.type", "k8s.pod"),
            kv("otel.entity.id", pod_id()),
            kv("otel.entity.attributes", pod_description()),
            kv("otel.entity.interval", Value::IntValue(300_000)),
        ],
    )
}

fn pod_row(wire_form: WireForm) -> EntityEventRow {
    EntityEventRow {
        kind: EntityEventKind::State,
        ts_ns: i64::try_from(TS).unwrap(),
        entity_type: "k8s.pod".into(),
        entity_id: vec![("k8s.pod.uid".into(), "u-1".into())],
        description: pod_description_json(),
        relationships: json!([]),
        interval_s: 300,
        delete_reason: None,
        wire_form,
        observer: "tok:test".into(),
        schema_url: Some(SCHEMA.into()),
        environment: "prod".into(),
    }
}

fn prod() -> Option<Resource> {
    resource(&[("deployment.environment.name", "prod")])
}

/// Split one marked-scope request and expect no drops.
fn entities_of(resource: Option<Resource>, records: Vec<LogRecord>) -> Vec<EntityEventRow> {
    let tally = EntityDrops::new();
    let split = split_with(
        &request(resource, marked_scope(), records),
        RECEIVED,
        &options(),
        &tally,
    );
    assert_eq!(tally.total(), 0, "{:?}", split.dropped);
    assert!(split.logs.is_empty());
    split.entities
}

/// Split one marked-scope request and expect every record dropped.
fn drops_of(resource: Option<Resource>, records: Vec<LogRecord>) -> Vec<EntityDrop> {
    let tally = EntityDrops::new();
    let split = split_with(
        &request(resource, marked_scope(), records),
        RECEIVED,
        &options(),
        &tally,
    );
    assert!(split.entities.is_empty(), "{:?}", split.entities);
    assert!(split.logs.is_empty(), "a dropped entity event is not a log");
    assert_eq!(tally.total(), split.dropped.len() as u64);
    split.dropped
}

#[test]
fn a_spec_state_event_becomes_one_row() {
    assert_eq!(
        entities_of(prod(), vec![spec_state()]),
        vec![pod_row(WireForm::Spec)]
    );
}

#[test]
fn a_legacy_state_event_becomes_the_same_row_with_seconds() {
    assert_eq!(
        entities_of(prod(), vec![legacy_state()]),
        vec![pod_row(WireForm::Legacy)]
    );
}

#[test]
fn spec_relationships_are_kept_as_an_array_of_maps() {
    let mut record = spec_state();
    record.attributes.push(kv(
        "entity.relationships",
        list(vec![map(vec![
            text("relationship.type", "scheduled_on"),
            text("entity.type", "k8s.node"),
            kv("entity.id", map(vec![text("k8s.node.uid", "n-1")])),
        ])]),
    ));
    let rows = entities_of(prod(), vec![record]);
    assert_eq!(
        rows[0].relationships,
        json!([{
            "relationship.type": "scheduled_on",
            "entity.type": "k8s.node",
            "entity.id": {"k8s.node.uid": "n-1"},
        }])
    );
}

#[test]
fn a_legacy_interval_below_a_second_still_reports_periodically() {
    let with_interval = |ms: i64| {
        let mut record = legacy_state();
        record
            .attributes
            .retain(|kv| kv.key != "otel.entity.interval");
        record
            .attributes
            .push(kv("otel.entity.interval", Value::IntValue(ms)));
        entities_of(prod(), vec![record])[0].interval_s
    };
    assert_eq!(with_interval(10_000), 10);
    assert_eq!(with_interval(500), 1);
    assert_eq!(with_interval(1_001), 2);
    assert_eq!(with_interval(0), 0);
    assert_eq!(with_interval(-5), 0);
    assert_eq!(with_interval(i64::MAX), ENTITY_INTERVAL_MAX_S);
}

#[test]
fn an_absent_interval_means_no_periodic_reports() {
    let mut record = spec_state();
    record
        .attributes
        .retain(|kv| kv.key != "entity.report.interval");
    assert_eq!(entities_of(prod(), vec![record])[0].interval_s, 0);
}

#[test]
fn a_spec_delete_keeps_its_reason_and_a_legacy_delete_has_none() {
    let spec = spec_record(
        "entity.delete",
        vec![
            text("entity.type", "k8s.pod"),
            kv("entity.id", pod_id()),
            text("entity.delete.reason", "scaled_down"),
        ],
    );
    let legacy = legacy_record(
        "entity_delete",
        vec![
            text("otel.entity.type", "k8s.pod"),
            kv("otel.entity.id", pod_id()),
        ],
    );
    let rows = entities_of(prod(), vec![spec, legacy]);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].kind, EntityEventKind::Delete);
    assert_eq!(rows[0].wire_form, WireForm::Spec);
    assert_eq!(rows[0].delete_reason.as_deref(), Some("scaled_down"));
    assert_eq!(rows[0].description, json!({}));
    assert_eq!(rows[1].kind, EntityEventKind::Delete);
    assert_eq!(rows[1].wire_form, WireForm::Legacy);
    assert_eq!(rows[1].delete_reason, None);
    assert_eq!(rows[1].entity_id, rows[0].entity_id);
}

/// A delete says that an entity is gone. Whatever else the record carries
/// is not read, so nothing in it can keep the delete from being applied.
#[test]
fn a_delete_reads_only_the_type_the_id_and_the_reason() {
    let spec = spec_record(
        "entity.delete",
        vec![
            text("entity.type", "k8s.pod"),
            kv("entity.id", pod_id()),
            text("entity.delete.reason", "scaled_down"),
            text("entity.description", "not a map"),
            text("entity.report.interval", "soon"),
            kv("entity.relationships", map(vec![])),
        ],
    );
    let legacy = legacy_record(
        "entity_delete",
        vec![
            text("otel.entity.type", "k8s.pod"),
            kv("otel.entity.id", pod_id()),
            text("otel.entity.attributes", "not a map"),
            kv("otel.entity.interval", map(vec![])),
        ],
    );
    let large = spec_record(
        "entity.delete",
        vec![
            text("entity.type", "k8s.pod"),
            kv("entity.id", pod_id()),
            kv(
                "entity.description",
                map(vec![text("test.blob", &"x".repeat(MAX_STATE_BYTES))]),
            ),
            kv("entity.report.interval", Value::IntValue(300)),
        ],
    );
    let rows = entities_of(prod(), vec![spec, legacy, large]);
    assert_eq!(rows.len(), 3);
    for row in &rows {
        assert_eq!(row.kind, EntityEventKind::Delete);
        assert_eq!(row.description, json!({}));
        assert_eq!(row.relationships, json!([]));
        assert_eq!(row.interval_s, 0);
    }
    assert_eq!(rows[0].delete_reason.as_deref(), Some("scaled_down"));
}

/// `host_metrics` sends the `host` entity with a type and no id; the id is
/// on the resource once `resourcedetection` ran.
#[test]
fn an_id_less_host_event_takes_its_identity_from_the_resource() {
    let spec = spec_record(
        "entity.state",
        vec![
            text("entity.type", "host"),
            kv("entity.report.interval", Value::IntValue(10)),
        ],
    );
    let legacy = legacy_record(
        "entity_state",
        vec![
            text("otel.entity.type", "host"),
            kv("otel.entity.interval", Value::IntValue(10_000)),
        ],
    );
    let detected = resource(&[("host.id", "9f2c"), ("host.name", "node-a")]);
    let rows = entities_of(detected, vec![spec, legacy]);
    assert_eq!(rows.len(), 2);
    for row in &rows {
        assert_eq!(row.entity_type, "host");
        assert_eq!(row.entity_id, vec![("host.id".into(), "9f2c".into())]);
        assert_eq!(row.interval_s, 10);
        assert_eq!(row.description, json!({}));
        assert_eq!(row.environment, "");
    }
    assert_eq!(rows[0].wire_form, WireForm::Spec);
    assert_eq!(rows[1].wire_form, WireForm::Legacy);
}

#[test]
fn an_empty_id_map_also_falls_back_to_the_resource() {
    let record = spec_record(
        "entity.state",
        vec![text("entity.type", "host"), kv("entity.id", map(vec![]))],
    );
    let rows = entities_of(resource(&[("host.id", "9f2c")]), vec![record]);
    assert_eq!(rows[0].entity_id, vec![("host.id".into(), "9f2c".into())]);
}

#[test]
fn an_event_without_any_identity_is_dropped_and_counted() {
    let host = || spec_record("entity.state", vec![text("entity.type", "host")]);
    // No resource at all, a resource without host.id, and an empty host.id.
    assert_eq!(drops_of(None, vec![host()]), vec![EntityDrop::NoIdentity]);
    assert_eq!(
        drops_of(resource(&[("host.name", "node-a")]), vec![host()]),
        vec![EntityDrop::NoIdentity]
    );
    assert_eq!(
        drops_of(resource(&[("host.id", "")]), vec![host()]),
        vec![EntityDrop::NoIdentity]
    );
}

/// The resource of a Collector describes the Collector. Only for `host`
/// is that also the entity; a pod event without id must not become the
/// Collector's own pod.
#[test]
fn only_the_host_type_has_a_resource_fallback() {
    let pod = spec_record("entity.state", vec![text("entity.type", "k8s.pod")]);
    assert_eq!(
        drops_of(resource(&[("k8s.pod.uid", "collector-pod")]), vec![pod]),
        vec![EntityDrop::NoIdentity]
    );
}

#[test]
fn an_event_without_a_type_is_dropped() {
    let untyped = spec_record("entity.state", vec![kv("entity.id", pod_id())]);
    let empty = spec_record(
        "entity.state",
        vec![text("entity.type", ""), kv("entity.id", pod_id())],
    );
    let wrong_form = spec_record(
        "entity.state",
        vec![
            text("otel.entity.type", "k8s.pod"),
            kv("entity.id", pod_id()),
        ],
    );
    assert_eq!(
        drops_of(prod(), vec![untyped, empty, wrong_form]),
        vec![EntityDrop::NoType; 3]
    );
}

#[test]
fn a_marked_record_that_is_neither_state_nor_delete_is_dropped() {
    let bare = LogRecord {
        time_unix_nano: TS,
        ..LogRecord::default()
    };
    let unknown = legacy_record("entity_patch", vec![text("otel.entity.type", "host")]);
    assert_eq!(
        drops_of(prod(), vec![bare, unknown]),
        vec![EntityDrop::UnknownKind; 2]
    );
}

/// A marked record of a kind this build does not know (a later
/// `entity.patch`) is dropped like before. The notice about it names the
/// event and has its own limit, so the warnings about other drops cannot
/// hide it.
#[test]
fn an_unknown_kind_is_named_once_per_interval() {
    let patch = LogRecord {
        event_name: "entity.patch".into(),
        ..LogRecord::default()
    };
    let legacy = legacy_record("entity_merge", vec![]);
    let tally = EntityDrops::new();
    assert_eq!(
        unknown_kind_notice(&patch, &tally, 1_000),
        Some(("entity.patch".to_string(), 0))
    );
    assert_eq!(unknown_kind_notice(&patch, &tally, 1_001), None);
    assert_eq!(unknown_kind_notice(&legacy, &tally, 1_059), None);
    assert_eq!(
        unknown_kind_notice(&legacy, &tally, 1_060),
        Some(("entity_merge".to_string(), 2))
    );
}

#[test]
fn the_name_of_an_unknown_kind_is_cut_and_may_be_absent() {
    let tally = EntityDrops::new();
    let long = LogRecord {
        event_name: "e".repeat(MAX_TYPE_LEN + 50),
        ..LogRecord::default()
    };
    assert_eq!(
        unknown_kind_notice(&long, &tally, 1_000),
        Some(("e".repeat(MAX_TYPE_LEN), 0))
    );
    let bare = LogRecord::default();
    assert_eq!(
        unknown_kind_notice(&bare, &tally, 2_000),
        Some((String::new(), 0))
    );
}

/// Drops for other reasons use the other limit: they neither spend the
/// notice nor are held back by it.
#[test]
fn other_drops_do_not_spend_the_unknown_kind_notice() {
    let tally = EntityDrops::new();
    let untyped = spec_record("entity.state", vec![kv("entity.id", pod_id())]);
    for _ in 0..3 {
        note_drop(EntityDrop::NoType, &untyped, &tally);
    }
    let patch = LogRecord {
        event_name: "entity.patch".into(),
        ..LogRecord::default()
    };
    assert_eq!(
        unknown_kind_notice(&patch, &tally, common::now_s()),
        Some(("entity.patch".to_string(), 0))
    );
}

#[test]
fn values_of_the_wrong_shape_are_dropped_as_malformed() {
    let id_is_text = spec_record(
        "entity.state",
        vec![text("entity.type", "k8s.pod"), text("entity.id", "u-1")],
    );
    let nested_id = spec_record(
        "entity.state",
        vec![
            text("entity.type", "k8s.pod"),
            kv("entity.id", map(vec![kv("k8s.pod.uid", pod_id())])),
        ],
    );
    let description_is_text = spec_record(
        "entity.state",
        vec![
            text("entity.type", "k8s.pod"),
            kv("entity.id", pod_id()),
            text("entity.description", "x"),
        ],
    );
    let relationships_is_map = spec_record(
        "entity.state",
        vec![
            text("entity.type", "k8s.pod"),
            kv("entity.id", pod_id()),
            kv("entity.relationships", map(vec![])),
        ],
    );
    assert_eq!(
        drops_of(
            prod(),
            vec![
                id_is_text,
                nested_id,
                description_is_text,
                relationships_is_map
            ]
        ),
        vec![EntityDrop::Malformed; 4]
    );
}

#[test]
fn oversized_events_are_dropped() {
    let long_type = spec_record(
        "entity.state",
        vec![
            text("entity.type", &"t".repeat(MAX_TYPE_LEN + 1)),
            kv("entity.id", pod_id()),
        ],
    );
    let long_id_value = spec_record(
        "entity.state",
        vec![
            text("entity.type", "k8s.pod"),
            kv(
                "entity.id",
                map(vec![text("k8s.pod.uid", &"u".repeat(MAX_ID_PART_LEN + 1))]),
            ),
        ],
    );
    let many_ids = spec_record(
        "entity.state",
        vec![
            text("entity.type", "k8s.pod"),
            kv(
                "entity.id",
                map((0..=MAX_ID_PAIRS)
                    .map(|i| text(&format!("k{i}"), "v"))
                    .collect()),
            ),
        ],
    );
    let big_description = spec_record(
        "entity.state",
        vec![
            text("entity.type", "k8s.pod"),
            kv("entity.id", pod_id()),
            kv(
                "entity.description",
                map(vec![text("test.blob", &"x".repeat(MAX_STATE_BYTES))]),
            ),
        ],
    );
    assert_eq!(
        drops_of(
            prod(),
            vec![long_type, long_id_value, many_ids, big_description]
        ),
        vec![EntityDrop::Oversized; 4]
    );
}

#[test]
fn a_description_nested_too_deep_is_malformed() {
    let mut value = Value::IntValue(1);
    for _ in 0..=MAX_DEPTH {
        value = map(vec![kv("test.n", value)]);
    }
    let record = spec_record(
        "entity.state",
        vec![
            text("entity.type", "k8s.pod"),
            kv("entity.id", pod_id()),
            kv("entity.description", value),
        ],
    );
    assert_eq!(drops_of(prod(), vec![record]), vec![EntityDrop::Malformed]);
}

#[test]
fn description_values_keep_their_types() {
    let record = spec_record(
        "entity.state",
        vec![
            text("entity.type", "k8s.pod"),
            kv("entity.id", pod_id()),
            kv(
                "entity.description",
                map(vec![
                    kv("test.bytes", Value::BytesValue(vec![0x01, 0xff])),
                    kv("test.nan", Value::DoubleValue(f64::NAN)),
                    kv("test.ratio", Value::DoubleValue(0.25)),
                    kv("test.count", Value::IntValue(-2)),
                    KeyValue {
                        key: "test.unset".into(),
                        value: None,
                        key_strindex: 0,
                    },
                ]),
            ),
        ],
    );
    assert_eq!(
        entities_of(prod(), vec![record])[0].description,
        json!({
            "test.bytes": "01ff",
            "test.nan": "NaN",
            "test.ratio": 0.25,
            "test.count": -2,
            "test.unset": null,
        })
    );
}

/// The key of an entity is text. Were `1` and `"1"`, or `true` and
/// `"true"`, both accepted they would name the same entity, so only a
/// string identifies one.
#[test]
fn only_a_string_identifies_an_entity() {
    let with_id = |value: Value| {
        spec_record(
            "entity.state",
            vec![
                text("entity.type", "process"),
                kv(
                    "entity.id",
                    map(vec![kv("process.pid", value), text("host.id", "9f2c")]),
                ),
            ],
        )
    };
    assert_eq!(
        entities_of(prod(), vec![with_id(Value::StringValue("1".into()))])[0].entity_id,
        vec![
            ("host.id".to_string(), "9f2c".to_string()),
            ("process.pid".to_string(), "1".to_string()),
        ]
    );
    let others = vec![
        with_id(Value::IntValue(1)),
        with_id(Value::BoolValue(true)),
        with_id(Value::DoubleValue(1.0)),
        with_id(Value::BytesValue(vec![0x31])),
    ];
    assert_eq!(drops_of(prod(), others), vec![EntityDrop::Malformed; 4]);
}

/// The target of a relationship is identified the same way.
#[test]
fn only_a_string_identifies_the_target_of_a_relationship() {
    let record = spec_record(
        "entity.state",
        vec![
            text("entity.type", "k8s.pod"),
            kv("entity.id", pod_id()),
            kv(
                "entity.relationships",
                list(vec![map(vec![
                    text("relationship.type", "runs_on"),
                    text("entity.type", "process"),
                    kv(
                        "entity.id",
                        map(vec![kv("process.pid", Value::IntValue(1))]),
                    ),
                ])]),
            ),
        ],
    );
    assert_eq!(drops_of(prod(), vec![record]), vec![EntityDrop::Malformed]);
}

fn labelled() -> LogRecord {
    spec_record(
        "entity.state",
        vec![
            text("entity.type", "k8s.pod"),
            kv("entity.id", pod_id()),
            kv(
                "entity.description",
                map(vec![
                    text("k8s.pod.name", "checkout-7d9"),
                    text("k8s.pod.label.app.kubernetes.io/name", "checkout"),
                    text("k8s.pod.label.cost-centre", "4411"),
                    text("k8s.namespace.label.team", "payments"),
                ]),
            ),
        ],
    )
}

#[test]
fn label_keys_are_removed_unless_allowed() {
    let rows = entities_of(prod(), vec![labelled()]);
    assert_eq!(
        rows[0].description,
        json!({"k8s.pod.name": "checkout-7d9"}),
        "no label is kept by default"
    );

    let allow = EntityOptions {
        observer: "tok:test".into(),
        label_allow: vec!["app.kubernetes.io/name".into(), "team".into()],
        attribute_allow: vec![],
    };
    let split = split_with(
        &request(prod(), marked_scope(), vec![labelled()]),
        RECEIVED,
        &allow,
        &EntityDrops::new(),
    );
    assert_eq!(
        split.entities[0].description,
        json!({
            "k8s.pod.name": "checkout-7d9",
            "k8s.pod.label.app.kubernetes.io/name": "checkout",
            "k8s.namespace.label.team": "payments",
        })
    );
}

#[test]
fn a_label_key_is_recognised_by_its_shape() {
    assert_eq!(label_name("k8s.pod.label.app"), Some("app"));
    assert_eq!(label_name("k8s.replicaset.label.a.b/c"), Some("a.b/c"));
    assert_eq!(label_name("k8s.pod.name"), None);
    assert_eq!(label_name("k8s.pod.label."), None);
    assert_eq!(label_name("k8s..label.app"), None);
    assert_eq!(label_name("host.label.app"), None);
}

#[test]
fn each_signal_alone_marks_a_record_as_an_entity_event() {
    let tally = EntityDrops::new();
    // No scope marker: the event name and the legacy attribute suffice.
    let unmarked = split_with(
        &request(prod(), None, vec![spec_state(), legacy_state()]),
        RECEIVED,
        &options(),
        &tally,
    );
    assert_eq!(unmarked.entities.len(), 2);
    assert!(unmarked.logs.is_empty());

    // A marker written as text counts too; `false` does not.
    let as_text = Some(InstrumentationScope {
        attributes: vec![text("otel.entity.event_as_log", "true")],
        ..InstrumentationScope::default()
    });
    let marked = split_with(
        &request(prod(), as_text, vec![LogRecord::default()]),
        RECEIVED,
        &options(),
        &tally,
    );
    assert_eq!(marked.dropped, vec![EntityDrop::UnknownKind]);
    let off = Some(InstrumentationScope {
        attributes: vec![kv("otel.entity.event_as_log", Value::BoolValue(false))],
        ..InstrumentationScope::default()
    });
    let plain = split_with(
        &request(prod(), off, vec![LogRecord::default()]),
        RECEIVED,
        &options(),
        &tally,
    );
    assert_eq!(plain.logs.len(), 1);
    assert!(plain.dropped.is_empty());
}

#[test]
fn ordinary_records_of_the_same_export_stay_logs() {
    let ordinary = LogRecord {
        time_unix_nano: TS,
        severity_text: "INFO".into(),
        body: Some(any(Value::StringValue("experiment started".into()))),
        event_name: "experiment.started".into(),
        ..LogRecord::default()
    };
    let mixed = request(prod(), None, vec![ordinary, spec_state()]);
    let split = split_with(&mixed, RECEIVED, &options(), &EntityDrops::new());
    assert_eq!(split.entities.len(), 1);
    assert_eq!(split.logs.len(), 1);
    assert_eq!(split.logs[0].body, "experiment started");
    assert_eq!(
        split.logs[0].event_name.as_deref(),
        Some("experiment.started")
    );
}

#[test]
fn an_export_without_entity_events_gives_the_rows_the_log_path_gives() {
    let ordinary = |body: &str| LogRecord {
        time_unix_nano: TS,
        severity_text: "WARN".into(),
        body: Some(any(Value::StringValue(body.into()))),
        attributes: vec![text("k", "v")],
        ..LogRecord::default()
    };
    let export = request(
        resource(&[("service.name", "checkout")]),
        None,
        vec![ordinary("a"), ordinary("b")],
    );
    let split = split_with(&export, RECEIVED, &options(), &EntityDrops::new());
    assert!(split.entities.is_empty());
    assert!(split.dropped.is_empty());
    assert_eq!(
        format!("{:?}", split.logs),
        format!("{:?}", crate::logs_request_to_rows(&export, RECEIVED))
    );
}

#[test]
fn the_event_time_never_runs_ahead_of_the_receipt() {
    let mut ahead = spec_state();
    ahead.time_unix_nano = u64::try_from(RECEIVED).unwrap() + 3_600_000_000_000;
    let mut observed_only = spec_state();
    observed_only.time_unix_nano = 0;
    observed_only.observed_time_unix_nano = TS + 1;
    let mut untimed = spec_state();
    untimed.time_unix_nano = 0;
    let rows = entities_of(prod(), vec![ahead, observed_only, untimed]);
    assert_eq!(rows[0].ts_ns, RECEIVED);
    assert_eq!(rows[1].ts_ns, i64::try_from(TS).unwrap() + 1);
    assert_eq!(rows[2].ts_ns, RECEIVED);
}

#[test]
fn the_schema_url_comes_from_the_scope_then_the_resource() {
    let mut export = request(prod(), marked_scope(), vec![spec_state()]);
    export.resource_logs[0].scope_logs[0].schema_url = String::new();
    export.resource_logs[0].schema_url = "https://example.test/resource".into();
    let split = split_with(&export, RECEIVED, &options(), &EntityDrops::new());
    assert_eq!(
        split.entities[0].schema_url.as_deref(),
        Some("https://example.test/resource")
    );
    export.resource_logs[0].schema_url = String::new();
    let split = split_with(&export, RECEIVED, &options(), &EntityDrops::new());
    assert_eq!(split.entities[0].schema_url, None);
}

#[test]
fn a_long_delete_reason_is_cut() {
    let record = spec_record(
        "entity.delete",
        vec![
            text("entity.type", "k8s.pod"),
            kv("entity.id", pod_id()),
            text("entity.delete.reason", &"å".repeat(MAX_REASON_LEN)),
        ],
    );
    let rows = entities_of(prod(), vec![record]);
    let reason = rows[0].delete_reason.as_deref().unwrap();
    assert!(reason.len() <= MAX_REASON_LEN, "{} bytes", reason.len());
    assert!(reason.starts_with("ååå"));
}

#[test]
fn drops_are_counted_per_reason_and_in_the_process_counter() {
    let tally = EntityDrops::new();
    let host = spec_record("entity.state", vec![text("entity.type", "host")]);
    let bare = LogRecord::default();
    let export = request(None, marked_scope(), vec![host, bare]);
    let _ = split_with(&export, RECEIVED, &options(), &tally);
    assert_eq!(tally.total(), 2);
    assert_eq!(tally.count(EntityDrop::NoIdentity), 1);
    assert_eq!(tally.count(EntityDrop::UnknownKind), 1);
    assert_eq!(tally.count(EntityDrop::Malformed), 0);

    let before = entity_events_dropped_total();
    let split = split_entity_events(&export, RECEIVED, &options());
    assert_eq!(split.dropped.len(), 2);
    // Other tests of this binary may count into it too: a floor.
    assert!(entity_events_dropped_total() >= before + 2);
}

#[test]
fn every_drop_reason_has_its_own_name() {
    let names: std::collections::BTreeSet<&str> =
        EntityDrop::ALL.iter().map(|d| d.as_str()).collect();
    assert_eq!(names.len(), EntityDrop::ALL.len());
    assert!(names.contains("no_identity"));
}
