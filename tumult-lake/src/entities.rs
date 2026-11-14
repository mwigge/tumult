//! Entity storage (schema v21): the `entity_versions` table and the rules
//! that fold a stream of entity events into it.
//!
//! An entity event is either the full current state of an entity or its
//! deletion. Senders repeat the state at a fixed interval, so storing every
//! event would grow with time. Instead one row holds one distinct state of
//! one entity, and a repeated state only moves `last_seen_ns` (the sender's
//! event time) and `last_received_ns` (the daemon's time of receipt, from
//! which liveness is counted):
//!
//! * a state with no row yet opens a row;
//! * a state equal to the latest row (same `state_hash` and the same
//!   report interval, or no interval stated) refreshes it;
//! * a state that differs opens a new row when it is newer than the latest
//!   row was last seen, and is ignored otherwise. The interval is part of
//!   the state because liveness is derived from it: were a heartbeat to
//!   rewrite it in place, a sender could turn an entity expired or
//!   never-expiring and leave no row that shows what held before;
//! * a delete sets the tombstone on the latest row when it is not older
//!   than `last_seen_ns`;
//! * a state newer than the tombstone opens a new row: the entity is back.
//!
//! "Newer" is decided by the event time. Events that carry none, or one
//! ahead of the daemon's clock, all get the receipt time of their export,
//! so within one batch a tie is settled by the order of arrival: an event
//! is also newer than the row when an earlier event of the same batch set
//! what the row records and is not later than it. Against a row that an
//! earlier batch stored, an equal time stays "not newer": it cannot be told
//! from a delayed copy.
//!
//! The table is bounded (see [`caps`]): an entity keeps a limited number
//! of rows, an environment and the lake hold a limited number of entities,
//! and entities that are gone or expired leave after a retention window.
//!
//! A batch costs the writer a fixed number of statements whatever it
//! holds (see [`batch`]): the latest rows of the entities it names are read
//! at once, the new rows are inserted at once, the changed rows are updated
//! at once, and the version cap is applied at once.
//!
//! Rows are tracked per `(entity_key, environment)`. The table has no
//! primary key or index (the v5 rule for tables that receive UPDATEs);
//! `valid_from_ns` is strictly increasing per entity by construction (a
//! row that wins a tie starts one nanosecond after the row it supersedes),
//! which keeps `(entity_key, environment, valid_from_ns)` unique.

use std::collections::HashMap;
use std::fmt::Write as _;

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::StoreError;
use crate::{LogRow, Writer};

mod batch;
mod caps;

pub(crate) use caps::EntityBook;
pub use caps::{
    EntityEvictions, EntityLimits, EntityPurge, EvictionReason, ENTITIES_PER_ENVIRONMENT,
    ENTITY_UNREPORTED_AFTER_S, ENTITY_VERSIONS_PER_ENTITY,
};

/// Upper bound on a stored report interval (30 days). It keeps the
/// `entities_current` expiry arithmetic inside `BIGINT` whatever a sender
/// claims.
pub const ENTITY_INTERVAL_MAX_S: i64 = 30 * 86_400;

/// What an entity event says about its entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityEventKind {
    /// The full current state.
    State,
    /// The entity is gone.
    Delete,
}

/// Which of the two wire forms carried the event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireForm {
    /// The OpenTelemetry specification form (`EventName` = `entity.state`).
    Spec,
    /// The Collector's earlier form (`otel.entity.event.type`).
    Legacy,
}

impl WireForm {
    /// The value stored in `entity_versions.wire_form`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Spec => "spec",
            Self::Legacy => "legacy",
        }
    }
}

/// One entity event, normalised from either wire form.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityEventRow {
    pub kind: EntityEventKind,
    pub ts_ns: i64,
    /// When the daemon received the export that carried the event, by the
    /// daemon's clock. Liveness is counted from this, not from `ts_ns`,
    /// which is the sender's clock.
    pub received_at_ns: i64,
    pub entity_type: String,
    /// The identifying attributes.
    pub entity_id: Vec<(String, String)>,
    /// The descriptive attributes, a JSON object.
    pub description: Value,
    /// The relationships, a JSON array kept as sent.
    pub relationships: Value,
    /// Seconds between periodic reports. `Some(0)` says none are sent;
    /// `None` says nothing, and the entity keeps the interval it has (none
    /// when it is new).
    pub interval_s: Option<i64>,
    pub delete_reason: Option<String>,
    pub wire_form: WireForm,
    /// How the event came in: a name for the ingest token that carried it,
    /// or for the lack of one. Stored with the row the event opens. It is a
    /// record only; no rule of this module reads it, because the daemon has
    /// one ingest token and cannot tell senders apart.
    pub observer: String,
    pub schema_url: Option<String>,
    /// The environment the sending resource names; empty when it names none.
    pub environment: String,
}

/// What applying one entity event did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityOutcome {
    /// A new row was opened.
    Opened,
    /// The state matched the latest row.
    Refreshed,
    /// The latest row received its tombstone.
    Tombstoned,
    /// A state that is not newer than what the latest row already records.
    StaleState,
    /// A delete for an entity that has no row.
    DeleteUnknown,
    /// A delete for an entity that is already gone.
    DeleteRepeated,
    /// A delete older than the last state seen.
    DeleteOutOfOrder,
    /// A state for an entity that has no row, refused because the
    /// environment or the lake holds as many entities as
    /// [`EntityLimits`] allows.
    EntityLimit,
}

/// What applying a batch of entity events did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EntityApplied {
    /// One outcome per event, in the order given.
    pub outcomes: Vec<EntityOutcome>,
    /// Rows removed because an entity had more than
    /// [`EntityLimits::versions_per_entity`].
    pub versions_pruned: usize,
    /// Entities removed, with all their rows, to make room for new ones.
    pub evicted: EntityEvictions,
}

impl EntityOutcome {
    /// Whether the event changed nothing.
    #[must_use]
    pub fn is_ignored(self) -> bool {
        !matches!(self, Self::Opened | Self::Refreshed | Self::Tombstoned)
    }
}

/// Append `part` to `out` with the key separators percent-encoded.
fn push_key_part(out: &mut String, part: &str) {
    for c in part.chars() {
        if matches!(c, '%' | '|' | '=' | ',' | '/') {
            // Writing to a String cannot fail.
            let _ = write!(out, "%{:02X}", u32::from(c));
        } else {
            out.push(c);
        }
    }
}

/// The stable key of an entity: its type, `|`, then its identifying
/// attributes as `name=value` pairs sorted by name and joined with `,`.
/// `%`, `|`, `=`, `,` and `/` inside a part are percent-encoded, so two
/// different identities never share a key. Every other character of the
/// type and the id is kept as sent (`?`, `#`, a space, a quote), so a key
/// is not safe to put in a URL as it is: a client percent-encodes the
/// whole key, `%` included, to make it one path segment.
#[must_use]
pub fn entity_key(entity_type: &str, entity_id: &[(String, String)]) -> String {
    let mut pairs: Vec<&(String, String)> = entity_id.iter().collect();
    pairs.sort();
    let mut key = String::new();
    push_key_part(&mut key, entity_type);
    key.push('|');
    for (i, (name, value)) in pairs.into_iter().enumerate() {
        if i > 0 {
            key.push(',');
        }
        push_key_part(&mut key, name);
        key.push('=');
        push_key_part(&mut key, value);
    }
    key
}

/// Serialise `value` with object keys in sorted order, so equal states have
/// equal text whatever order the sender used.
fn push_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                push_canonical(&map[key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                push_canonical(item, out);
            }
            out.push(']');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

fn canonical(value: &Value) -> String {
    let mut out = String::new();
    push_canonical(value, &mut out);
    out
}

/// SHA-256 over the canonical description and relationships.
fn state_hash(description: &str, relationships: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(description.as_bytes());
    hasher.update(b"\n");
    hasher.update(relationships.as_bytes());
    let mut hex = String::with_capacity(64);
    for byte in hasher.finalize() {
        // Writing to a String cannot fail.
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

fn id_object(entity_id: &[(String, String)]) -> Value {
    let pairs = entity_id
        .iter()
        .map(|(name, value)| (name.clone(), Value::String(value.clone())));
    Value::Object(pairs.collect())
}

/// The columns of a row that has not been inserted yet.
struct Unsaved {
    entity_type: String,
    /// The identifying attributes, a JSON object of strings.
    entity_id: Value,
    description: String,
    relationships: String,
    schema_url: Option<String>,
}

/// The latest row of one entity while a batch is applied.
struct Latest {
    valid_from_ns: i64,
    last_seen_ns: i64,
    last_received_ns: i64,
    interval_s: i64,
    wire_form: String,
    state_hash: String,
    deleted_at_ns: Option<i64>,
    delete_reason: Option<String>,
    observer: String,
    /// Set until the row is inserted.
    unsaved: Option<Unsaved>,
    /// A stored row whose mutable columns changed.
    dirty: bool,
    /// The time of the event of this batch that set what the row records
    /// (its state, `last_seen_ns` or the tombstone); `None` while only
    /// earlier batches did.
    batch_ts_ns: Option<i64>,
}

impl Latest {
    /// Whether an event at `ts_ns` arrived after the event of its own batch
    /// that this row records and is not older than it: the tie on the
    /// timestamp that the order of arrival settles.
    fn set_earlier_in_batch(&self, ts_ns: i64) -> bool {
        self.batch_ts_ns.is_some_and(|set_at| ts_ns >= set_at)
    }
}

/// An event with its derived key, state text and hash.
struct Prepared<'a> {
    event: &'a EntityEventRow,
    /// The key of the entity and the environment it is tracked in.
    slot: Slot,
    description: String,
    relationships: String,
    hash: String,
    /// The interval the event states, within bounds.
    interval_s: Option<i64>,
}

impl<'a> Prepared<'a> {
    fn new(event: &'a EntityEventRow) -> Self {
        let description = canonical(&event.description);
        let relationships = canonical(&event.relationships);
        Self {
            event,
            slot: (
                entity_key(&event.entity_type, &event.entity_id),
                event.environment.clone(),
            ),
            hash: state_hash(&description, &relationships),
            description,
            relationships,
            interval_s: event
                .interval_s
                .map(|stated| stated.clamp(0, ENTITY_INTERVAL_MAX_S)),
        }
    }

    /// The interval of the entity once this state is applied: the one the
    /// event states, else the one of `latest`, the row the entity has,
    /// else none. An event that says nothing about the interval must not
    /// change it: of two reporters of one entity the one without an
    /// interval would otherwise open a row with every report and stop the
    /// entity from ever expiring.
    fn interval_after(&self, latest: Option<&Latest>) -> i64 {
        self.interval_s
            .or_else(|| latest.map(|row| row.interval_s))
            .unwrap_or(0)
    }

    /// The row this state opens after `superseded`, the latest row so far.
    /// It starts at the event time, or one nanosecond after the superseded
    /// row started when a tie would otherwise give two rows one start.
    fn open(&self, superseded: Option<&Latest>) -> Latest {
        let event = self.event;
        let valid_from_ns = superseded.map_or(event.ts_ns, |row| {
            event.ts_ns.max(row.valid_from_ns.saturating_add(1))
        });
        Latest {
            valid_from_ns,
            last_seen_ns: valid_from_ns,
            last_received_ns: event.received_at_ns,
            interval_s: self.interval_after(superseded),
            wire_form: event.wire_form.as_str().to_owned(),
            state_hash: self.hash.clone(),
            deleted_at_ns: None,
            delete_reason: None,
            observer: event.observer.clone(),
            unsaved: Some(Unsaved {
                entity_type: event.entity_type.clone(),
                entity_id: id_object(&event.entity_id),
                description: self.description.clone(),
                relationships: self.relationships.clone(),
                schema_url: event.schema_url.clone(),
            }),
            dirty: false,
            batch_ts_ns: Some(event.ts_ns),
        }
    }
}

/// What a state event does to the latest row of its entity.
enum StateStep {
    Open,
    Refresh,
    Stale,
}

fn state_step(latest: Option<&Latest>, state: &Prepared<'_>) -> StateStep {
    let Some(latest) = latest else {
        return StateStep::Open;
    };
    let ts_ns = state.event.ts_ns;
    let wins_tie = latest.set_earlier_in_batch(ts_ns);
    if let Some(deleted_at_ns) = latest.deleted_at_ns {
        if ts_ns > deleted_at_ns || wins_tie {
            return StateStep::Open;
        }
        return StateStep::Stale;
    }
    if latest.state_hash == state.hash && latest.interval_s == state.interval_after(Some(latest)) {
        return StateStep::Refresh;
    }
    if ts_ns > latest.last_seen_ns || wins_tie {
        return StateStep::Open;
    }
    StateStep::Stale
}

/// Record that the state was reported again. Every copy moves the time of
/// receipt forward: the daemon heard from the sender. Only one that is not
/// older than the last report moves `last_seen_ns`, and that one also sets
/// the wire form. The interval is never touched: another interval is
/// another state.
fn refresh(latest: &mut Latest, state: &Prepared<'_>) {
    latest.last_received_ns = latest.last_received_ns.max(state.event.received_at_ns);
    latest.dirty = true;
    if state.event.ts_ns < latest.last_seen_ns {
        return;
    }
    latest.batch_ts_ns = Some(state.event.ts_ns);
    latest.last_seen_ns = state.event.ts_ns;
    state
        .event
        .wire_form
        .as_str()
        .clone_into(&mut latest.wire_form);
    latest.dirty = true;
}

fn apply_delete(latest: Option<&mut Latest>, event: &EntityEventRow) -> EntityOutcome {
    let Some(latest) = latest else {
        return EntityOutcome::DeleteUnknown;
    };
    if latest.deleted_at_ns.is_some() {
        return EntityOutcome::DeleteRepeated;
    }
    if event.ts_ns < latest.last_seen_ns && !latest.set_earlier_in_batch(event.ts_ns) {
        return EntityOutcome::DeleteOutOfOrder;
    }
    // A row that won a tie starts, and was last seen, a nanosecond after
    // the event time; its tombstone is not earlier than that.
    latest.deleted_at_ns = Some(event.ts_ns.max(latest.last_seen_ns));
    latest.last_received_ns = latest.last_received_ns.max(event.received_at_ns);
    latest.batch_ts_ns = Some(event.ts_ns);
    latest.delete_reason.clone_from(&event.delete_reason);
    latest.dirty = true;
    EntityOutcome::Tombstoned
}

/// `(entity_key, environment)`.
type Slot = (String, String);

/// A batch while it is applied.
struct Fold<'a> {
    limits: &'a EntityLimits,
    latest: HashMap<Slot, Latest>,
    /// The entities that gained a row.
    grown: Vec<Slot>,
    /// Entities evicted to make room for the ones the batch opened.
    evicted: EntityEvictions,
    /// Rows that a later event of the batch superseded.
    closed: Vec<(Slot, Latest)>,
}

impl Fold<'_> {
    /// Every row the batch touched: the superseded ones, then the latest.
    fn rows(&self) -> impl Iterator<Item = (&Slot, &Latest)> {
        let closed = self.closed.iter().map(|(slot, row)| (slot, row));
        closed.chain(self.latest.iter())
    }
}

impl Writer {
    /// Fold `events` into `entity_versions` in one transaction, in the order
    /// given, within `limits`, and report what each event did.
    ///
    /// # Errors
    /// Returns an error if a statement fails (nothing is stored).
    pub fn apply_entity_events(
        &self,
        events: &[EntityEventRow],
        limits: &EntityLimits,
    ) -> Result<EntityApplied, StoreError> {
        self.entity_tx(|| self.put_entity_events(events, limits))
    }

    /// Store the log rows and the entity events of one export in a single
    /// transaction: a sender that retries a failed export must not find part
    /// of it already stored.
    ///
    /// # Errors
    /// Returns an error if a statement fails (nothing is stored).
    pub fn insert_logs_and_entity_events(
        &self,
        logs: &[LogRow],
        events: &[EntityEventRow],
        limits: &EntityLimits,
    ) -> Result<EntityApplied, StoreError> {
        self.entity_tx(|| {
            self.put_logs(logs)?;
            self.put_entity_events(events, limits)
        })
    }

    /// The body of [`Writer::apply_entity_events`], inside the caller's
    /// transaction.
    fn put_entity_events(
        &self,
        events: &[EntityEventRow],
        limits: &EntityLimits,
    ) -> Result<EntityApplied, StoreError> {
        if events.is_empty() {
            return Ok(EntityApplied::default());
        }
        let prepared: Vec<Prepared<'_>> = events.iter().map(Prepared::new).collect();
        let mut fold = Fold {
            limits,
            latest: self.load_latest(&prepared)?,
            grown: Vec::new(),
            evicted: EntityEvictions::default(),
            closed: Vec::new(),
        };
        let mut outcomes = Vec::with_capacity(events.len());
        for item in &prepared {
            let slot = item.slot.clone();
            let outcome = match item.event.kind {
                EntityEventKind::Delete => apply_delete(fold.latest.get_mut(&slot), item.event),
                EntityEventKind::State => self.apply_state(&mut fold, slot, item)?,
            };
            outcomes.push(outcome);
        }
        self.insert_versions(&fold)?;
        self.update_versions(&fold)?;
        let versions_pruned = self.cap_entity_versions(&fold.grown, limits.versions_per_entity)?;
        // A delete made an entity evictable where there may have been
        // none.
        if outcomes.contains(&EntityOutcome::Tombstoned) {
            self.entities.look_again();
        }
        Ok(EntityApplied {
            outcomes,
            versions_pruned,
            evicted: fold.evicted,
        })
    }

    fn apply_state(
        &self,
        fold: &mut Fold<'_>,
        slot: Slot,
        state: &Prepared<'_>,
    ) -> Result<EntityOutcome, StoreError> {
        match state_step(fold.latest.get(&slot), state) {
            StateStep::Stale => Ok(EntityOutcome::StaleState),
            StateStep::Refresh => {
                if let Some(row) = fold.latest.get_mut(&slot) {
                    refresh(row, state);
                }
                Ok(EntityOutcome::Refreshed)
            }
            StateStep::Open => {
                if !fold.latest.contains_key(&slot)
                    && !self.admit(&slot.1, fold.limits, &mut fold.evicted)?
                {
                    return Ok(EntityOutcome::EntityLimit);
                }
                let opened = state.open(fold.latest.get(&slot));
                fold.grown.push(slot.clone());
                // The row this one supersedes leaves the map; it is
                // written with the others when the batch is folded.
                if let Some(closed) = fold.latest.insert(slot.clone(), opened) {
                    fold.closed.push((slot, closed));
                }
                Ok(EntityOutcome::Opened)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonical_text_sorts_keys_at_every_depth() {
        let value: Value =
            serde_json::from_str(r#"{"b": [{"z": 1, "a": "x\"y"}], "a": null}"#).unwrap();
        assert_eq!(canonical(&value), r#"{"a":null,"b":[{"a":"x\"y","z":1}]}"#);
    }

    #[test]
    fn the_hash_separates_description_from_relationships() {
        let empty = state_hash("{}", "[]");
        assert_eq!(empty.len(), 64);
        assert_ne!(empty, state_hash("{}", "[{}]"));
        assert_ne!(state_hash("{\"a\":1}", "[]"), empty);
        assert_eq!(
            state_hash(&canonical(&json!({})), &canonical(&json!([]))),
            empty
        );
    }

    #[test]
    fn only_applied_outcomes_count_as_changes() {
        for applied in [
            EntityOutcome::Opened,
            EntityOutcome::Refreshed,
            EntityOutcome::Tombstoned,
        ] {
            assert!(!applied.is_ignored());
        }
        for ignored in [
            EntityOutcome::StaleState,
            EntityOutcome::DeleteUnknown,
            EntityOutcome::DeleteRepeated,
            EntityOutcome::DeleteOutOfOrder,
        ] {
            assert!(ignored.is_ignored());
        }
    }
}
