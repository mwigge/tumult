//! What an event may be large and what its identity may contain.

use super::*;

/// One state event of `record_type` and `id`, from a resource in
/// `environment`, under a scope with `schema_url`.
fn drop_of(environment: &str, schema_url: &str, record: LogRecord) -> Option<EntityDrop> {
    let mut export = request(
        resource(&[("deployment.environment.name", environment)]),
        marked_scope(),
        vec![record],
    );
    export.resource_logs[0].scope_logs[0].schema_url = schema_url.into();
    let tally = EntityDrops::new();
    let split = split_with(&export, RECEIVED, &options(), &tally);
    assert_eq!(split.entities.len() + split.dropped.len(), 1);
    split.dropped.first().copied()
}

fn pod(entity_type: &str, id_key: &str, id_value: &str) -> LogRecord {
    spec_record(
        "entity.state",
        vec![
            text("entity.type", entity_type),
            kv("entity.id", map(vec![text(id_key, id_value)])),
        ],
    )
}

fn plain_pod() -> LogRecord {
    pod("k8s.pod", "k8s.pod.uid", "u-1")
}

fn described(description: Value) -> LogRecord {
    spec_record(
        "entity.state",
        vec![
            text("entity.type", "k8s.pod"),
            kv("entity.id", pod_id()),
            kv("entity.description", description),
        ],
    )
}

/// The environment is chosen by the sender and is part of what an entity
/// is tracked by, so it is bounded like the rest of the identity.
#[test]
fn an_environment_longer_than_the_limit_is_oversized() {
    let longest = "e".repeat(MAX_ENVIRONMENT_LEN);
    assert_eq!(drop_of(&longest, SCHEMA, plain_pod()), None);
    assert_eq!(
        drop_of(&format!("{longest}e"), SCHEMA, plain_pod()),
        Some(EntityDrop::Oversized)
    );
}

#[test]
fn a_schema_url_longer_than_the_limit_is_oversized() {
    let longest = "s".repeat(MAX_SCHEMA_URL_LEN);
    assert_eq!(drop_of("prod", &longest, plain_pod()), None);
    assert_eq!(
        drop_of("prod", &format!("{longest}s"), plain_pod()),
        Some(EntityDrop::Oversized)
    );
}

/// A control character in what names an entity cannot be typed into a URL
/// or read in a log line, and NUL ends a string in more than one place
/// between here and a client.
#[test]
fn a_control_character_in_what_names_an_entity_is_malformed() {
    let malformed = Some(EntityDrop::Malformed);
    for record in [
        pod("k8s\0pod", "k8s.pod.uid", "u-1"),
        pod("k8s.pod", "k8s.pod\0uid", "u-1"),
        pod("k8s.pod", "k8s.pod.uid", "u\0-1"),
        pod("k8s.pod", "k8s.pod.uid", "u-1\n"),
        pod("k8s.pod", "k8s.pod.uid", "u\u{7f}1"),
        pod("k8s.pod", "k8s.pod.uid", "u\u{85}1"),
    ] {
        assert_eq!(drop_of("prod", SCHEMA, record), malformed);
    }
    assert_eq!(drop_of("pr\0od", SCHEMA, plain_pod()), malformed);
    assert_eq!(
        drop_of("prod", "https://x/\u{1b}[2J", plain_pod()),
        malformed
    );
}

/// Quotes and backslashes are ordinary text: the identity keeps them as
/// sent.
#[test]
fn a_quote_and_a_backslash_in_an_id_are_kept_as_sent() {
    let id = r#"a"b\c'd"#;
    let rows = entities_of(prod(), vec![pod("k8s.pod", "k8s.pod.uid", id)]);
    assert_eq!(
        rows[0].entity_id,
        vec![("k8s.pod.uid".to_string(), id.to_string())]
    );
}

/// Many small values stay under the byte limit and are still too many.
#[test]
fn more_values_than_the_limit_are_oversized() {
    let entries = |n: usize| {
        map((0..n)
            .map(|i| kv(&format!("test.{i}"), Value::IntValue(1)))
            .collect())
    };
    let tally = EntityDrops::new();
    let export = request(prod(), marked_scope(), vec![described(entries(1000))]);
    let split = split_with(&export, RECEIVED, &options(), &tally);
    assert_eq!(split.dropped, vec![]);
    assert_eq!(
        drops_of(prod(), vec![described(entries(MAX_STATE_ELEMENTS + 1))]),
        vec![EntityDrop::Oversized]
    );
    let long_list = map(vec![kv(
        "test.list",
        list(vec![Value::IntValue(1); MAX_STATE_ELEMENTS + 1]),
    )]);
    assert_eq!(
        drops_of(prod(), vec![described(long_list)]),
        vec![EntityDrop::Oversized]
    );
}

/// Keys that are removed count as values too: an event cannot make the
/// daemon walk an unbounded description by filling it with keys nothing
/// lists.
#[test]
fn removed_keys_count_towards_the_limit() {
    let unlisted = map((0..=MAX_STATE_ELEMENTS)
        .map(|i| text(&format!("k8s.pod.label.l{i}"), "v"))
        .collect());
    assert_eq!(
        drops_of(prod(), vec![described(unlisted)]),
        vec![EntityDrop::Oversized]
    );
}

/// The budget is charged while a value is converted, and the conversion
/// stops at the value that exceeds it.
#[test]
fn the_budget_stops_the_conversion_at_the_value_that_exceeds_it() {
    let mut budget = Budget {
        bytes: 16,
        elements: 3,
    };
    let small = any(Value::StringValue("abcd".into()));
    assert_eq!(value_json(Some(&small), 1, &mut budget), Ok(json!("abcd")));
    assert_eq!(budget.elements, 2);
    assert_eq!(budget.bytes, 10);
    let large = any(Value::StringValue("abcdefghijk".into()));
    assert_eq!(
        value_json(Some(&large), 1, &mut budget),
        Err(EntityDrop::Oversized)
    );
    let mut few = Budget {
        bytes: 1024,
        elements: 2,
    };
    let three = any(list(vec![Value::IntValue(1); 3]));
    assert_eq!(
        value_json(Some(&three), 1, &mut few),
        Err(EntityDrop::Oversized)
    );
}

/// An export stores a bounded number of entity events. The ones beyond it
/// are dropped and counted; the ordinary records of the export are kept
/// wherever they stand.
#[test]
fn entity_events_beyond_the_export_limit_are_dropped_and_counted() {
    assert_eq!(MAX_EVENTS_PER_EXPORT, 10_000);
    let records: Vec<LogRecord> = (0..MAX_EVENTS_PER_EXPORT + 3)
        .map(|n| pod("k8s.pod", "k8s.pod.uid", &format!("u-{n}")))
        .collect();
    let mut export = request(resource(&[]), marked_scope(), records);
    export.resource_logs[0].scope_logs.push(ScopeLogs {
        scope: None,
        log_records: vec![LogRecord {
            time_unix_nano: TS,
            ..LogRecord::default()
        }],
        schema_url: String::new(),
    });
    let tally = EntityDrops::new();
    let split = split_with(&export, RECEIVED, &options(), &tally);
    assert_eq!(split.entities.len(), MAX_EVENTS_PER_EXPORT);
    assert_eq!(
        split.entities.last().unwrap().entity_id[0].1,
        format!("u-{}", MAX_EVENTS_PER_EXPORT - 1),
        "the first ones are the ones kept"
    );
    assert_eq!(split.dropped, vec![EntityDrop::ExportLimit; 3]);
    assert_eq!(tally.count(EntityDrop::ExportLimit), 3);
    assert_eq!(EntityDrop::ExportLimit.as_str(), "export_limit");
    assert_eq!(split.logs.len(), 1, "a log record after them is stored");
}

/// An event that is dropped for another reason takes no place under the
/// limit.
#[test]
fn only_stored_events_count_towards_the_export_limit() {
    let mut records = vec![spec_record("entity.state", vec![]); 5];
    records.extend(
        (0..MAX_EVENTS_PER_EXPORT).map(|n| pod("k8s.pod", "k8s.pod.uid", &format!("u-{n}"))),
    );
    let export = request(resource(&[]), marked_scope(), records);
    let tally = EntityDrops::new();
    let split = split_with(&export, RECEIVED, &options(), &tally);
    assert_eq!(split.entities.len(), MAX_EVENTS_PER_EXPORT);
    assert_eq!(tally.count(EntityDrop::ExportLimit), 0);
    assert_eq!(tally.count(EntityDrop::NoType), 5);
}

/// Format characters (Unicode `Cf`) are not seen when a name is read: a
/// bidi override shows it reversed and a zero-width character makes two
/// names look like one. What names an entity holds none.
#[test]
fn a_format_character_in_what_names_an_entity_is_malformed() {
    let malformed = Some(EntityDrop::Malformed);
    for hidden in [
        '\u{ad}',
        '\u{200b}',
        '\u{200d}',
        '\u{200f}',
        '\u{202e}',
        '\u{2066}',
        '\u{feff}',
        '\u{e0041}',
    ] {
        for record in [
            pod(&format!("k8s{hidden}pod"), "k8s.pod.uid", "u-1"),
            pod("k8s.pod", &format!("k8s.pod{hidden}uid"), "u-1"),
            pod("k8s.pod", "k8s.pod.uid", &format!("u{hidden}1")),
        ] {
            assert_eq!(drop_of("prod", SCHEMA, record), malformed, "{hidden:?}");
        }
    }
    // Letters outside ASCII are names like any other.
    assert_eq!(
        drop_of("prod", SCHEMA, pod("k8s.pod", "k8s.pod.uid", "bjørn-東京")),
        None
    );
}

fn related(relationship_type: &str) -> LogRecord {
    spec_record(
        "entity.state",
        vec![
            text("entity.type", "k8s.pod"),
            kv("entity.id", pod_id()),
            kv(
                "entity.relationships",
                list(vec![map(vec![
                    text("relationship.type", relationship_type),
                    text("entity.type", "k8s.node"),
                    kv("entity.id", map(vec![text("k8s.node.uid", "n-1")])),
                ])]),
            ),
        ],
    )
}

/// The type of a relationship is a name like the type of an entity.
#[test]
fn a_control_or_format_character_in_a_relationship_type_is_malformed() {
    assert_eq!(drop_of("prod", SCHEMA, related("runs_on")), None);
    for hidden in ["runs\0on", "runs_on\n", "runs\u{202e}on", "\u{200b}runs_on"] {
        assert_eq!(
            drop_of("prod", SCHEMA, related(hidden)),
            Some(EntityDrop::Malformed),
            "{hidden:?}"
        );
    }
}

/// A delete is applied whatever its reason holds: the characters a name
/// may not hold are taken out of the reason and the delete stays.
#[test]
fn a_delete_reason_loses_its_control_and_format_characters_and_still_deletes() {
    let delete = |reason: &str| {
        spec_record(
            "entity.delete",
            vec![
                text("entity.type", "k8s.pod"),
                kv("entity.id", pod_id()),
                text("entity.delete.reason", reason),
            ],
        )
    };
    let rows = entities_of(
        prod(),
        vec![
            delete("scaled\0 down\n"),
            delete("\u{202e}evicted\u{200b}"),
            delete("\0\u{feff}"),
        ],
    );
    let reasons: Vec<Option<&str>> = rows
        .iter()
        .map(|row| row.delete_reason.as_deref())
        .collect();
    assert_eq!(reasons, vec![Some("scaled down"), Some("evicted"), None]);
    assert!(rows.iter().all(|row| row.kind == EntityEventKind::Delete));
}
