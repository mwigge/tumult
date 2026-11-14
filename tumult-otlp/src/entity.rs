//! Entity events: log records that carry the state or the deletion of an
//! entity.
//!
//! Two wire forms exist. The specification form names the event in the
//! record's `EventName` (`entity.state`, `entity.delete`) and uses the
//! `entity.*` attributes, with the report interval in seconds. The legacy
//! form, which Collector contrib still writes by default, names the event
//! in the attribute `otel.entity.event.type` (`entity_state`,
//! `entity_delete`) and uses the `otel.entity.*` attributes, with the
//! interval in milliseconds. Both become one [`EntityEventRow`].
//!
//! What a sender puts in an event is a claim about an entity, and its size
//! and shape are the sender's choice: every part is bounded here, and an
//! event that is too large, of the wrong shape or without identity is
//! counted and dropped. It is never stored as a log record instead.

use std::sync::atomic::{AtomicU64, Ordering};

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{any_value, AnyValue, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ScopeLogs};
use opentelemetry_proto::tonic::resource::v1::Resource;
use serde_json::{Map, Value as Json};
use tumult_core::compliance::truncated;
use tumult_lake::{EntityEventKind, EntityEventRow, LogRow, WireForm, ENTITY_INTERVAL_MAX_S};

use crate::common::{self, ResourceCtx, WarnLimit};
use crate::logs;

/// Scope attribute a Collector sets on the scope that carries entity
/// events.
const SCOPE_MARKER: &str = "otel.entity.event_as_log";
/// Legacy attribute that names the event.
const LEGACY_EVENT_TYPE: &str = "otel.entity.event.type";

/// Longest entity type kept, in bytes.
pub const MAX_TYPE_LEN: usize = 128;
/// Longest key or value of one identifying attribute, in bytes.
pub const MAX_ID_PART_LEN: usize = 256;
/// Most identifying attributes of one entity.
pub const MAX_ID_PAIRS: usize = 16;
/// Largest description and relationships of one event, as JSON bytes.
pub const MAX_STATE_BYTES: usize = 64 * 1024;
/// Deepest nesting of maps and arrays in a description or a relationship.
pub const MAX_DEPTH: usize = 8;
/// Longest delete reason kept, in bytes; a longer one is cut.
pub const MAX_REASON_LEN: usize = 256;

/// Seconds between two warnings about dropped entity events.
const DROP_WARN_INTERVAL_S: u64 = 60;

/// Entity types whose identity may be read from the resource when the
/// event carries none, with the resource attributes that identify them.
/// The resource of a Collector describes the Collector, so this holds only
/// the type for which that is also the entity: `host_metrics` reports the
/// `host` it runs on without an id and `resourcedetection` puts `host.id`
/// on the resource.
const RESOURCE_IDENTITY: &[(&str, &[&str])] = &[("host", &["host.id"])];

/// What the ingest path knows about the sender and the operator's choices.
#[derive(Debug, Clone, Default)]
pub struct EntityOptions {
    /// Who reported the events: a name for the ingest token, never the
    /// token itself.
    pub observer: String,
    /// Label names kept from `k8s.<kind>.label.<name>` description keys.
    /// Every other label is removed before the event is stored.
    pub label_allow: Vec<String>,
}

/// Why an entity event was not stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityDrop {
    /// Marked as an entity event but neither a state nor a delete.
    UnknownKind,
    /// No entity type, or an empty one.
    NoType,
    /// No identifying attributes on the event and none usable on the
    /// resource.
    NoIdentity,
    /// A part of the event has the wrong shape.
    Malformed,
    /// A part of the event is larger than this module keeps.
    Oversized,
}

impl EntityDrop {
    /// Every reason, in the order of the counters.
    pub const ALL: [Self; 5] = [
        Self::UnknownKind,
        Self::NoType,
        Self::NoIdentity,
        Self::Malformed,
        Self::Oversized,
    ];

    /// The reason as it appears in logs and metrics.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnknownKind => "unknown_kind",
            Self::NoType => "no_type",
            Self::NoIdentity => "no_identity",
            Self::Malformed => "malformed",
            Self::Oversized => "oversized",
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::UnknownKind => 0,
            Self::NoType => 1,
            Self::NoIdentity => 2,
            Self::Malformed => 3,
            Self::Oversized => 4,
        }
    }
}

/// Dropped entity events per reason, with the limit on the warning about
/// them.
pub(crate) struct EntityDrops {
    counts: [AtomicU64; 5],
    warn: WarnLimit,
}

impl EntityDrops {
    pub(crate) const fn new() -> Self {
        Self {
            counts: [
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
            ],
            warn: WarnLimit::new(),
        }
    }

    pub(crate) fn count(&self, reason: EntityDrop) -> u64 {
        self.counts[reason.index()].load(Ordering::Relaxed)
    }

    pub(crate) fn total(&self) -> u64 {
        EntityDrop::ALL.iter().map(|r| self.count(*r)).sum()
    }
}

/// What this process dropped.
static DROPS: EntityDrops = EntityDrops::new();

/// Entity events dropped since start, all reasons together.
#[must_use]
pub fn entity_events_dropped_total() -> u64 {
    DROPS.total()
}

/// Entity events dropped since start for one reason.
#[must_use]
pub fn entity_events_dropped(reason: EntityDrop) -> u64 {
    DROPS.count(reason)
}

/// One logs export, divided into what goes to `logs` and what goes to
/// `entity_versions`.
#[derive(Debug, Default)]
pub struct LogsSplit {
    /// The ordinary records, as [`crate::logs_request_to_rows`] gives them.
    pub logs: Vec<LogRow>,
    /// The entity events, in the order received.
    pub entities: Vec<EntityEventRow>,
    /// One entry per entity event that was dropped.
    pub dropped: Vec<EntityDrop>,
}

/// Divide a logs export into log rows and entity events (pure apart from
/// the drop counter and its warning). `received_at_ns` is when the export
/// reached kronika; an event never carries a later time than that.
#[must_use]
pub fn split_entity_events(
    request: &ExportLogsServiceRequest,
    received_at_ns: i64,
    options: &EntityOptions,
) -> LogsSplit {
    split_with(request, received_at_ns, options, &DROPS)
}

/// [`split_entity_events`] counting into `tally`.
pub(crate) fn split_with(
    request: &ExportLogsServiceRequest,
    received_at_ns: i64,
    options: &EntityOptions,
    tally: &EntityDrops,
) -> LogsSplit {
    let mut split = LogsSplit::default();
    for resource_logs in &request.resource_logs {
        let resource = ResourceCtx::from_resource(resource_logs.resource.as_ref());
        for scope_logs in &resource_logs.scope_logs {
            let scope_attrs = logs::scope_attrs(scope_logs);
            let marked = scope_is_marked(scope_logs);
            let source = Source {
                resource: resource_logs.resource.as_ref(),
                environment: resource.target_environment.as_deref().unwrap_or(""),
                schema_url: schema_url(&scope_logs.schema_url, &resource_logs.schema_url),
                received_at_ns,
            };
            for record in &scope_logs.log_records {
                if !marked && !is_entity_event(record) {
                    split.logs.push(logs::log_row(
                        record,
                        &resource,
                        &scope_attrs,
                        received_at_ns,
                    ));
                    continue;
                }
                match normalise(record, &source, options) {
                    Ok(row) => split.entities.push(row),
                    Err(reason) => {
                        note_drop(reason, record, tally);
                        split.dropped.push(reason);
                    }
                }
            }
        }
    }
    split
}

/// Where a record came from: what its resource and scope say.
struct Source<'a> {
    resource: Option<&'a Resource>,
    environment: &'a str,
    schema_url: Option<&'a str>,
    received_at_ns: i64,
}

/// The scope's schema URL, else the resource's, else none.
fn schema_url<'a>(scope: &'a str, resource: &'a str) -> Option<&'a str> {
    [scope, resource].into_iter().find(|url| !url.is_empty())
}

/// Whether the scope says that all its records are entity events. The
/// marker is a boolean; a sender that writes it as text is accepted too.
fn scope_is_marked(scope_logs: &ScopeLogs) -> bool {
    let Some(scope) = scope_logs.scope.as_ref() else {
        return false;
    };
    match common::attr_str(&scope.attributes, SCOPE_MARKER).and_then(|v| v.value.as_ref()) {
        Some(any_value::Value::BoolValue(marked)) => *marked,
        Some(any_value::Value::StringValue(text)) => text == "true",
        _ => false,
    }
}

/// Whether the record itself says that it is an entity event.
fn is_entity_event(record: &LogRecord) -> bool {
    spec_kind(&record.event_name).is_some()
        || record
            .attributes
            .iter()
            .any(|kv| kv.key == LEGACY_EVENT_TYPE)
}

fn spec_kind(event_name: &str) -> Option<EntityEventKind> {
    match event_name {
        "entity.state" => Some(EntityEventKind::State),
        "entity.delete" => Some(EntityEventKind::Delete),
        _ => None,
    }
}

fn legacy_kind(record: &LogRecord) -> Option<EntityEventKind> {
    match text_attr(&record.attributes, LEGACY_EVENT_TYPE)? {
        "entity_state" => Some(EntityEventKind::State),
        "entity_delete" => Some(EntityEventKind::Delete),
        _ => None,
    }
}

/// The attribute names of one wire form.
struct Keys {
    form: WireForm,
    entity_type: &'static str,
    id: &'static str,
    description: &'static str,
    interval: &'static str,
    /// Milliseconds in the legacy form, seconds in the specification form.
    interval_in_ms: bool,
    relationships: Option<&'static str>,
    delete_reason: Option<&'static str>,
}

const SPEC: Keys = Keys {
    form: WireForm::Spec,
    entity_type: "entity.type",
    id: "entity.id",
    description: "entity.description",
    interval: "entity.report.interval",
    interval_in_ms: false,
    relationships: Some("entity.relationships"),
    delete_reason: Some("entity.delete.reason"),
};

const LEGACY: Keys = Keys {
    form: WireForm::Legacy,
    entity_type: "otel.entity.type",
    id: "otel.entity.id",
    description: "otel.entity.attributes",
    interval: "otel.entity.interval",
    interval_in_ms: true,
    relationships: None,
    delete_reason: None,
};

/// What the record is and in which form it is written. The event name
/// decides before the legacy attribute does.
fn kind_and_keys(record: &LogRecord) -> Result<(EntityEventKind, &'static Keys), EntityDrop> {
    if let Some(kind) = spec_kind(&record.event_name) {
        return Ok((kind, &SPEC));
    }
    legacy_kind(record)
        .map(|kind| (kind, &LEGACY))
        .ok_or(EntityDrop::UnknownKind)
}

/// One record as a row, or the reason it cannot be one.
fn normalise(
    record: &LogRecord,
    source: &Source<'_>,
    options: &EntityOptions,
) -> Result<EntityEventRow, EntityDrop> {
    let (kind, keys) = kind_and_keys(record)?;
    let attrs = record.attributes.as_slice();
    let entity_type = entity_type(attrs, keys)?;
    let entity_id = identity(attrs, keys, entity_type, source.resource)?;
    let description = description(attrs, keys, &options.label_allow)?;
    let relationships = relationships(attrs, keys)?;
    check_state_size(&description, &relationships)?;
    Ok(EntityEventRow {
        kind,
        ts_ns: logs::log_ts(record, source.received_at_ns).min(source.received_at_ns),
        entity_type: entity_type.to_string(),
        entity_id,
        description,
        relationships,
        interval_s: interval_s(attrs, keys)?,
        delete_reason: delete_reason(attrs, keys),
        wire_form: keys.form,
        observer: options.observer.clone(),
        schema_url: source.schema_url.map(str::to_string),
        environment: source.environment.to_string(),
    })
}

fn value_of<'a>(attrs: &'a [KeyValue], key: &str) -> Option<&'a any_value::Value> {
    common::attr_str(attrs, key).and_then(|v| v.value.as_ref())
}

/// A string attribute; `None` when absent or of another type.
fn text_attr<'a>(attrs: &'a [KeyValue], key: &str) -> Option<&'a str> {
    match value_of(attrs, key)? {
        any_value::Value::StringValue(text) => Some(text),
        _ => None,
    }
}

fn entity_type<'a>(attrs: &'a [KeyValue], keys: &Keys) -> Result<&'a str, EntityDrop> {
    let entity_type = match value_of(attrs, keys.entity_type) {
        None => return Err(EntityDrop::NoType),
        Some(any_value::Value::StringValue(text)) => text.as_str(),
        Some(_) => return Err(EntityDrop::Malformed),
    };
    if entity_type.is_empty() {
        return Err(EntityDrop::NoType);
    }
    if entity_type.len() > MAX_TYPE_LEN {
        return Err(EntityDrop::Oversized);
    }
    Ok(entity_type)
}

/// The identifying attributes, sorted by key: those of the event, or for
/// an event without any those of the resource (see
/// [`RESOURCE_IDENTITY`]).
fn identity(
    attrs: &[KeyValue],
    keys: &Keys,
    entity_type: &str,
    resource: Option<&Resource>,
) -> Result<Vec<(String, String)>, EntityDrop> {
    let own = match value_of(attrs, keys.id) {
        None => &[][..],
        Some(any_value::Value::KvlistValue(map)) => map.values.as_slice(),
        Some(_) => return Err(EntityDrop::Malformed),
    };
    let mut pairs = if own.is_empty() {
        resource_identity(entity_type, resource).ok_or(EntityDrop::NoIdentity)?
    } else {
        own_identity(own)?
    };
    for (key, value) in &pairs {
        if key.len() > MAX_ID_PART_LEN || value.len() > MAX_ID_PART_LEN {
            return Err(EntityDrop::Oversized);
        }
    }
    pairs.sort();
    if pairs.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err(EntityDrop::Malformed);
    }
    Ok(pairs)
}

/// The id map of an event as text pairs. A value is a scalar; a nested
/// value, an empty one or an empty key cannot identify anything.
fn own_identity(own: &[KeyValue]) -> Result<Vec<(String, String)>, EntityDrop> {
    if own.len() > MAX_ID_PAIRS {
        return Err(EntityDrop::Oversized);
    }
    own.iter()
        .map(|kv| {
            let value = id_text(kv.value.as_ref())?;
            if kv.key.is_empty() || value.is_empty() {
                return Err(EntityDrop::Malformed);
            }
            Ok((kv.key.clone(), value))
        })
        .collect()
}

fn id_text(value: Option<&AnyValue>) -> Result<String, EntityDrop> {
    let value = value.ok_or(EntityDrop::Malformed)?;
    match value.value {
        Some(
            any_value::Value::StringValue(_)
            | any_value::Value::BoolValue(_)
            | any_value::Value::IntValue(_)
            | any_value::Value::DoubleValue(_)
            | any_value::Value::BytesValue(_),
        ) => Ok(common::any_value_to_string(value)),
        _ => Err(EntityDrop::Malformed),
    }
}

/// The identity the resource gives an entity of this type: every listed
/// attribute, present and not empty.
fn resource_identity(
    entity_type: &str,
    resource: Option<&Resource>,
) -> Option<Vec<(String, String)>> {
    let (_, id_keys) = RESOURCE_IDENTITY
        .iter()
        .find(|(known, _)| *known == entity_type)?;
    let attrs = resource?.attributes.as_slice();
    id_keys
        .iter()
        .map(|key| {
            let value = text_attr(attrs, key).filter(|value| !value.is_empty())?;
            Some(((*key).to_string(), value.to_string()))
        })
        .collect()
}

/// The label name of a `k8s.<kind>.label.<name>` key, `None` for any
/// other key.
pub(crate) fn label_name(key: &str) -> Option<&str> {
    let (kind, name) = key.strip_prefix("k8s.")?.split_once(".label.")?;
    if kind.is_empty() || kind.contains('.') || name.is_empty() {
        return None;
    }
    Some(name)
}

/// Whether a description key is stored: every key that is not a label,
/// and the labels the operator listed.
fn key_is_kept(key: &str, label_allow: &[String]) -> bool {
    label_name(key).is_none_or(|name| label_allow.iter().any(|allowed| allowed == name))
}

/// The descriptive attributes as a JSON object, `{}` when absent.
fn description(
    attrs: &[KeyValue],
    keys: &Keys,
    label_allow: &[String],
) -> Result<Json, EntityDrop> {
    let entries = match value_of(attrs, keys.description) {
        None => return Ok(Json::Object(Map::new())),
        Some(any_value::Value::KvlistValue(map)) => map.values.as_slice(),
        Some(_) => return Err(EntityDrop::Malformed),
    };
    let kept: Vec<&KeyValue> = entries
        .iter()
        .filter(|kv| key_is_kept(&kv.key, label_allow))
        .collect();
    object_json(kept.into_iter(), 1)
}

/// The relationships as a JSON array of objects, `[]` when absent or in
/// the legacy form, which has none.
fn relationships(attrs: &[KeyValue], keys: &Keys) -> Result<Json, EntityDrop> {
    let Some(value) = keys.relationships.and_then(|key| value_of(attrs, key)) else {
        return Ok(Json::Array(Vec::new()));
    };
    let any_value::Value::ArrayValue(array) = value else {
        return Err(EntityDrop::Malformed);
    };
    array
        .values
        .iter()
        .map(|item| match item.value.as_ref() {
            Some(any_value::Value::KvlistValue(map)) => object_json(map.values.iter(), 2),
            _ => Err(EntityDrop::Malformed),
        })
        .collect::<Result<Vec<Json>, EntityDrop>>()
        .map(Json::Array)
}

/// A key-value list as a JSON object that sits `depth` levels deep.
fn object_json<'a>(
    entries: impl Iterator<Item = &'a KeyValue>,
    depth: usize,
) -> Result<Json, EntityDrop> {
    if depth > MAX_DEPTH {
        return Err(EntityDrop::Malformed);
    }
    let mut object = Map::new();
    for kv in entries {
        object.insert(kv.key.clone(), value_json(kv.value.as_ref(), depth)?);
    }
    Ok(Json::Object(object))
}

/// One value with its type kept; `depth` is the level of its container.
/// Bytes become hex, a double JSON cannot hold becomes its text, and an
/// unset value becomes null.
fn value_json(value: Option<&AnyValue>, depth: usize) -> Result<Json, EntityDrop> {
    let Some(value) = value.and_then(|v| v.value.as_ref()) else {
        return Ok(Json::Null);
    };
    Ok(match value {
        any_value::Value::StringValue(text) => Json::String(text.clone()),
        any_value::Value::BoolValue(flag) => Json::Bool(*flag),
        any_value::Value::IntValue(int) => Json::from(*int),
        any_value::Value::DoubleValue(double) => serde_json::Number::from_f64(*double)
            .map_or_else(|| Json::String(double.to_string()), Json::Number),
        any_value::Value::BytesValue(bytes) => Json::String(common::hex_encode(bytes)),
        any_value::Value::ArrayValue(array) => {
            if depth + 1 > MAX_DEPTH {
                return Err(EntityDrop::Malformed);
            }
            let items: Result<Vec<Json>, EntityDrop> = array
                .values
                .iter()
                .map(|item| value_json(Some(item), depth + 1))
                .collect();
            Json::Array(items?)
        }
        any_value::Value::KvlistValue(map) => object_json(map.values.iter(), depth + 1)?,
        // An interned string cannot be resolved without the string table.
        any_value::Value::StringValueStrindex(_) => Json::Null,
    })
}

fn check_state_size(description: &Json, relationships: &Json) -> Result<(), EntityDrop> {
    let bytes = description.to_string().len() + relationships.to_string().len();
    if bytes > MAX_STATE_BYTES {
        return Err(EntityDrop::Oversized);
    }
    Ok(())
}

/// The report interval in whole seconds, 0 when the event has none. A
/// legacy interval is in milliseconds and is rounded up, so an entity
/// that reports more often than once a second still counts as periodic.
fn interval_s(attrs: &[KeyValue], keys: &Keys) -> Result<i64, EntityDrop> {
    let seconds = match value_of(attrs, keys.interval) {
        None => return Ok(0),
        Some(any_value::Value::IntValue(raw)) if keys.interval_in_ms => ms_to_s(*raw),
        Some(any_value::Value::IntValue(raw)) => *raw,
        Some(any_value::Value::DoubleValue(raw)) if raw.is_finite() => {
            let seconds = if keys.interval_in_ms {
                raw / 1_000.0
            } else {
                *raw
            };
            // Bounded before the cast, so the cast cannot overflow.
            seconds.ceil().clamp(0.0, ENTITY_INTERVAL_MAX_S as f64) as i64
        }
        Some(_) => return Err(EntityDrop::Malformed),
    };
    Ok(seconds.clamp(0, ENTITY_INTERVAL_MAX_S))
}

fn ms_to_s(ms: i64) -> i64 {
    if ms <= 0 {
        return 0;
    }
    ms / 1_000 + i64::from(ms % 1_000 != 0)
}

/// The reason of a delete, cut to [`MAX_REASON_LEN`]; the legacy form has
/// none.
fn delete_reason(attrs: &[KeyValue], keys: &Keys) -> Option<String> {
    let reason = common::attr_string(attrs, keys.delete_reason?)?;
    if reason.is_empty() {
        return None;
    }
    Some(truncated(&reason, MAX_REASON_LEN).to_string())
}

/// The entity type a dropped record names in either form, cut to what a
/// type can be long.
fn claimed_type(record: &LogRecord) -> &str {
    let attrs = record.attributes.as_slice();
    let claimed = text_attr(attrs, SPEC.entity_type)
        .or_else(|| text_attr(attrs, LEGACY.entity_type))
        .unwrap_or("");
    truncated(claimed, MAX_TYPE_LEN)
}

/// Count a dropped event and, at most once per interval, say so.
fn note_drop(reason: EntityDrop, record: &LogRecord, tally: &EntityDrops) {
    tally.counts[reason.index()].fetch_add(1, Ordering::Relaxed);
    let Some(suppressed) = tally.warn.allow(common::now_s(), DROP_WARN_INTERVAL_S) else {
        return;
    };
    // `?` escapes what the sender put in the type.
    tracing::warn!(
        name: "otlp.entity_event.dropped",
        {
            entity.type = ?claimed_type(record),
            entity.event.drop_reason = reason.as_str(),
            entity.event.dropped_total = tally.total(),
            log.suppressed = suppressed,
        },
        "entity event dropped"
    );
}

#[cfg(test)]
mod tests;
