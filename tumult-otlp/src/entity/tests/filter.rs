//! What of a description and of the relationships is stored.
//!
//! The `k8s_cluster` fixtures are built from the source of Collector
//! contrib v0.165.0, not from a running cluster:
//!
//! * `pkg/experimentalmetricmetadata/entity_events.go` gives both wire
//!   forms: the attribute names, the event name and the scope marker;
//! * `receiver/k8sclusterreceiver/internal/metadata/entities.go`
//!   (`GetEntityEvents`) shows that a state event carries the metadata of
//!   an object as a flat map of strings, and that the id is one string;
//! * `internal/metadata/metadata.go` (`GetGenericMetadata`) and the
//!   builders under `internal/{pod,container,service,cronjob}` give the
//!   keys used below.

use super::*;

fn strings(pairs: &[(&str, &str)]) -> Value {
    map(pairs.iter().map(|(key, value)| text(key, value)).collect())
}

/// A state event of `entity_type` in the specification form.
fn spec_of(entity_type: &str, id_key: &str, description: Value) -> LogRecord {
    spec_record(
        "entity.state",
        vec![
            text("entity.type", entity_type),
            kv("entity.id", map(vec![text(id_key, "u-1")])),
            kv("entity.description", description),
            kv("entity.report.interval", Value::IntValue(300)),
        ],
    )
}

/// The same event in the legacy form.
fn legacy_of(entity_type: &str, id_key: &str, description: Value) -> LogRecord {
    legacy_record(
        "entity_state",
        vec![
            text("otel.entity.type", entity_type),
            kv("otel.entity.id", map(vec![text(id_key, "u-1")])),
            kv("otel.entity.attributes", description),
            kv("otel.entity.interval", Value::IntValue(300_000)),
        ],
    )
}

/// Split one record with `options`; the stored description and how many
/// keys were filtered.
fn filtered_with(options: &EntityOptions, record: LogRecord) -> (serde_json::Value, u64) {
    let tally = EntityDrops::new();
    let mut split = split_with(
        &request(prod(), marked_scope(), vec![record]),
        RECEIVED,
        options,
        &tally,
    );
    assert_eq!(tally.total(), 0, "{:?}", split.dropped);
    assert_eq!(tally.filtered(), split.filtered);
    (split.entities.remove(0).description, split.filtered)
}

/// The description of one event in both wire forms under the default
/// options: the two forms must agree.
fn default_description(
    entity_type: &str,
    id_key: &str,
    description: &Value,
) -> (serde_json::Value, u64) {
    let none = EntityOptions {
        observer: "tok:test".into(),
        ..EntityOptions::default()
    };
    let spec = filtered_with(&none, spec_of(entity_type, id_key, description.clone()));
    let legacy = filtered_with(&none, legacy_of(entity_type, id_key, description.clone()));
    assert_eq!(spec, legacy, "both wire forms are filtered alike");
    spec
}

/// `internal/service/service.go`: selector, labels and annotations of a
/// Service are all sent, each under its own prefix.
fn service_attributes() -> Value {
    strings(&[
        ("k8s.namespace.name", "shop"),
        ("k8s.service.name", "checkout"),
        ("k8s.service.creation_timestamp", "2026-03-01T10:00:00Z"),
        ("k8s.service.selector.app", "checkout"),
        ("k8s.service.label.team", "payments"),
        ("k8s.service.label.cost-centre", "4411"),
        ("k8s.service.annotation.owner", "payments@example.test"),
        (
            "k8s.service.annotation.example.test/config",
            r#"{"env":[{"name":"DB_PASSWORD","value":"hunter2"}]}"#,
        ),
    ])
}

#[test]
fn a_service_keeps_no_label_selector_or_annotation_by_default() {
    let (description, filtered) =
        default_description("k8s.service", "k8s.service.uid", &service_attributes());
    assert_eq!(
        description,
        json!({
            "k8s.namespace.name": "shop",
            "k8s.service.name": "checkout",
            "k8s.service.creation_timestamp": "2026-03-01T10:00:00Z",
        })
    );
    assert_eq!(filtered, 5);
}

/// `internal/pod/pods.go` with `GetGenericMetadata`: labels, the owner
/// reference, the workload and one empty-valued `k8s.service.<name>` key
/// per Service that selects the pod.
#[test]
fn a_pod_keeps_its_names_owner_and_phase() {
    let sent = strings(&[
        ("k8s.pod.name", "checkout-7d9-abc"),
        ("k8s.namespace.name", "shop"),
        ("k8s.node.name", "node-a"),
        ("k8s.pod.phase", "Running"),
        ("k8s.pod.status_reason", "Evicted"),
        ("pod.creation_timestamp", "2026-03-01T10:00:00Z"),
        ("k8s.workload.kind", "Pod"),
        ("k8s.workload.name", "checkout-7d9-abc"),
        ("k8s.replicaset.name", "checkout-7d9"),
        ("k8s.replicaset.uid", "rs-1"),
        ("k8s.pod.label.app", "checkout"),
        ("k8s.pod.label.pod-template-hash", "7d9"),
        ("k8s.service.checkout", ""),
    ]);
    let (description, filtered) = default_description("k8s.pod", "k8s.pod.uid", &sent);
    assert_eq!(
        description,
        json!({
            "k8s.pod.name": "checkout-7d9-abc",
            "k8s.namespace.name": "shop",
            "k8s.node.name": "node-a",
            "k8s.pod.phase": "Running",
            "k8s.pod.status_reason": "Evicted",
            "pod.creation_timestamp": "2026-03-01T10:00:00Z",
            "k8s.workload.kind": "Pod",
            "k8s.workload.name": "checkout-7d9-abc",
            "k8s.replicaset.name": "checkout-7d9",
            "k8s.replicaset.uid": "rs-1",
        })
    );
    assert_eq!(filtered, 3);
}

/// `internal/container/containers.go`: nothing sender-named, all kept.
#[test]
fn a_container_keeps_everything_the_receiver_sends() {
    let sent = strings(&[
        ("container.status", "running"),
        ("container.status.reason", "Completed"),
        ("container.creation_timestamp", "2026-03-01T10:00:02Z"),
        ("k8s.container.name", "checkout"),
        ("container.image.name", "registry.example.test/checkout"),
        ("container.image.tag", "1.4.2"),
        ("k8s.pod.name", "checkout-7d9-abc"),
        ("k8s.pod.uid", "u-1"),
        ("k8s.namespace.name", "shop"),
        ("k8s.node.name", "node-a"),
    ]);
    let (description, filtered) = default_description("container", "container.id", &sent);
    assert_eq!(description.as_object().unwrap().len(), 10);
    assert_eq!(description["container.image.tag"], "1.4.2");
    assert_eq!(filtered, 0);
}

/// `internal/cronjob/cronjobs.go` writes two keys without any prefix.
#[test]
fn a_cronjob_keeps_its_bare_keys() {
    let sent = strings(&[
        ("schedule", "*/5 * * * *"),
        ("concurrency_policy", "Forbid"),
        ("cronjob.creation_timestamp", "2026-03-01T10:00:00Z"),
        ("k8s.cronjob.label.team", "payments"),
    ]);
    let (description, filtered) = default_description("k8s.cronjob", "k8s.cronjob.uid", &sent);
    assert_eq!(
        description,
        json!({
            "schedule": "*/5 * * * *",
            "concurrency_policy": "Forbid",
            "cronjob.creation_timestamp": "2026-03-01T10:00:00Z",
        })
    );
    assert_eq!(filtered, 1);
}

/// Shapes no receiver read here sends, but any sender can.
#[test]
fn a_key_that_is_not_listed_is_never_kept_by_default() {
    let manifest = r#"{"spec":{"containers":[{"env":[{"name":"TOKEN","value":"s3cret"}]}]}}"#;
    let sent = map(vec![
        text("os.type", "linux"),
        text(
            "k8s.pod.annotation.kubectl.kubernetes.io/last-applied-configuration",
            manifest,
        ),
        text("k8s.pod.labels.app", "checkout"),
        text("k8s.pod.annotations.owner", "x"),
        text("container.label.maintainer", "x"),
        text("app", "checkout"),
        text("team", "payments"),
        // Under a kept prefix, but a label all the same.
        text("host.label.rack", "r1"),
        text("service.labels.tier", "1"),
        text("deployment.annotation.note", "x"),
        // Nested maps: under an unlisted key and under a listed one.
        kv("labels", map(vec![text("app", "checkout")])),
        kv(
            "host.name",
            map(vec![kv("labels", map(vec![text("app", "checkout")]))]),
        ),
        kv("host.ip", list(vec![map(vec![text("annotation", "x")])])),
    ]);
    let (description, filtered) = default_description("host", "host.id", &sent);
    assert_eq!(description, json!({"os.type": "linux"}));
    assert_eq!(filtered, 12);
}

#[test]
fn a_listed_key_keeps_scalars_and_lists_of_scalars() {
    let sent = map(vec![
        kv("host.cpu.count", Value::IntValue(8)),
        kv(
            "host.ip",
            list(vec![
                Value::StringValue("10.0.0.4".into()),
                Value::StringValue("fe80::1".into()),
            ]),
        ),
        kv("service.criticality", Value::StringValue("critical".into())),
        text("cloud.region", "eu-north-1"),
        text("deployment.environment.name", "prod"),
        text("ict.entity.tier", "1"),
        text("k8s.node.condition_ready", "true"),
    ]);
    let (description, filtered) = default_description("host", "host.id", &sent);
    assert_eq!(
        description,
        json!({
            "host.cpu.count": 8,
            "host.ip": ["10.0.0.4", "fe80::1"],
            "service.criticality": "critical",
            "cloud.region": "eu-north-1",
            "deployment.environment.name": "prod",
            "ict.entity.tier": "1",
            "k8s.node.condition_ready": "true",
        })
    );
    assert_eq!(filtered, 0);
}

#[test]
fn the_operator_names_the_labels_and_the_other_keys_to_keep() {
    let allow = EntityOptions {
        observer: "tok:test".into(),
        label_allow: vec!["team".into()],
        attribute_allow: vec![
            "k8s.service.annotation.owner".into(),
            "k8s.service.selector.*".into(),
        ],
    };
    let record = spec_of("k8s.service", "k8s.service.uid", service_attributes());
    let (description, filtered) = filtered_with(&allow, record);
    assert_eq!(
        description,
        json!({
            "k8s.namespace.name": "shop",
            "k8s.service.name": "checkout",
            "k8s.service.creation_timestamp": "2026-03-01T10:00:00Z",
            "k8s.service.label.team": "payments",
            "k8s.service.annotation.owner": "payments@example.test",
            "k8s.service.selector.app": "checkout",
        })
    );
    assert_eq!(filtered, 2, "the other label and the other annotation");
}

/// A label name allows the label, not an annotation of the same name.
#[test]
fn a_label_name_does_not_allow_an_annotation() {
    let allow = EntityOptions {
        observer: "tok:test".into(),
        label_allow: vec!["owner".into()],
        attribute_allow: vec![],
    };
    let record = spec_of("k8s.service", "k8s.service.uid", service_attributes());
    let (description, _) = filtered_with(&allow, record);
    assert!(description.get("k8s.service.annotation.owner").is_none());
}

#[test]
fn a_key_the_operator_named_keeps_a_nested_value() {
    let allow = EntityOptions {
        observer: "tok:test".into(),
        label_allow: vec![],
        attribute_allow: vec!["limits".into()],
    };
    let sent = map(vec![
        kv("limits", map(vec![kv("cpu", Value::DoubleValue(0.5))])),
        kv("requests", map(vec![kv("cpu", Value::DoubleValue(0.1))])),
    ]);
    let (description, filtered) = filtered_with(&allow, spec_of("k8s.pod", "k8s.pod.uid", sent));
    assert_eq!(description, json!({"limits": {"cpu": 0.5}}));
    assert_eq!(filtered, 1);
}

fn related(relationships: Value) -> LogRecord {
    let mut record = spec_state();
    record
        .attributes
        .push(kv("entity.relationships", relationships));
    record
}

/// Only the type and the identity of the target are stored. The
/// specification names the type `relationship.type` in one place and
/// `type` in another; both are read.
#[test]
fn a_relationship_is_reduced_to_its_type_and_its_target() {
    let sent = list(vec![
        map(vec![
            text("relationship.type", "scheduled_on"),
            text("entity.type", "k8s.node"),
            kv("entity.id", map(vec![text("k8s.node.uid", "n-1")])),
            kv("labels", map(vec![text("app", "checkout")])),
            text("note", "free text"),
        ]),
        map(vec![
            text("type", "part_of"),
            text("entity.type", "ict.entity"),
            kv("entity.id", map(vec![text("ict.entity.id", "cif-0042")])),
        ]),
    ]);
    let tally = EntityDrops::new();
    let split = split_with(
        &request(prod(), marked_scope(), vec![related(sent)]),
        RECEIVED,
        &options(),
        &tally,
    );
    assert_eq!(
        split.entities[0].relationships,
        json!([
            {
                "relationship.type": "scheduled_on",
                "entity.type": "k8s.node",
                "entity.id": {"k8s.node.uid": "n-1"},
            },
            {
                "relationship.type": "part_of",
                "entity.type": "ict.entity",
                "entity.id": {"ict.entity.id": "cif-0042"},
            },
        ])
    );
    assert_eq!(split.filtered, 2);
    assert_eq!(tally.filtered(), 2);
}

#[test]
fn a_relationship_without_a_type_or_a_target_is_malformed() {
    let target = || kv("entity.id", map(vec![text("k8s.node.uid", "n-1")]));
    let no_type = map(vec![text("entity.type", "k8s.node"), target()]);
    let no_target_type = map(vec![text("relationship.type", "runs_on"), target()]);
    let no_target_id = map(vec![
        text("relationship.type", "runs_on"),
        text("entity.type", "k8s.node"),
    ]);
    let nested_target_id = map(vec![
        text("relationship.type", "runs_on"),
        text("entity.type", "k8s.node"),
        kv(
            "entity.id",
            map(vec![kv("k8s.node.uid", map(vec![text("a", "b")]))]),
        ),
    ]);
    let records = [no_type, no_target_type, no_target_id, nested_target_id]
        .into_iter()
        .map(|item| related(list(vec![item])))
        .collect();
    assert_eq!(drops_of(prod(), records), vec![EntityDrop::Malformed; 4]);
}

#[test]
fn more_relationships_than_the_limit_are_oversized() {
    let one = || {
        map(vec![
            text("relationship.type", "runs_on"),
            text("entity.type", "k8s.node"),
            kv("entity.id", map(vec![text("k8s.node.uid", "n-1")])),
        ])
    };
    let many = list((0..=MAX_RELATIONSHIPS).map(|_| one()).collect());
    assert_eq!(
        drops_of(prod(), vec![related(many)]),
        vec![EntityDrop::Oversized]
    );
}

#[test]
fn filtered_keys_are_counted_for_the_process() {
    let before = entity_event_keys_filtered_total();
    let export = request(
        prod(),
        marked_scope(),
        vec![spec_of(
            "k8s.service",
            "k8s.service.uid",
            service_attributes(),
        )],
    );
    let split = split_entity_events(&export, RECEIVED, &EntityOptions::default());
    assert_eq!(split.filtered, 5);
    // Other tests of this binary count into it too: a floor.
    assert!(entity_event_keys_filtered_total() >= before + 5);
}

#[test]
fn a_kept_key_rule_matches_what_its_pattern_says() {
    assert!(KeptKey::Exact("schedule").matches("schedule"));
    assert!(!KeptKey::Exact("schedule").matches("schedule.x"));
    assert!(KeptKey::Prefix("host.").matches("host.name"));
    assert!(!KeptKey::Prefix("host.").matches("host."));
    assert!(!KeptKey::Prefix("host.").matches("hostile"));
    assert!(KeptKey::K8sField("name").matches("k8s.pod.name"));
    assert!(!KeptKey::K8sField("name").matches("k8s.pod.label.name"));
    assert!(!KeptKey::K8sField("name").matches("k8s..name"));
    assert!(!KeptKey::K8sField("name").matches("pod.name"));
    assert!(KeptKey::KindField("creation_timestamp").matches("pod.creation_timestamp"));
    assert!(!KeptKey::KindField("creation_timestamp").matches("a.b.creation_timestamp"));
    assert!(!KeptKey::KindField("creation_timestamp").matches(".creation_timestamp"));
    assert_eq!(KeptKey::Prefix("host.").pattern(), "host.*");
    assert_eq!(KeptKey::K8sField("uid").pattern(), "k8s.<kind>.uid");
    assert_eq!(
        KeptKey::KindField("creation_timestamp").pattern(),
        "<kind>.creation_timestamp"
    );
}

/// The guide shows the same table as the code.
#[test]
fn the_guide_lists_every_kept_key() {
    let guide = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../docs/guides/entity-events.md"
    ));
    for rule in KEPT_DESCRIPTION_KEYS {
        let cell = format!("`{}`", rule.pattern());
        assert!(guide.contains(&cell), "the guide does not list {cell}");
    }
}
