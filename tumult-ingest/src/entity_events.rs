//! Entity events on the ingest paths.
//!
//! An entity event is a log record that carries the state or the deletion
//! of an entity (see `tumult_otlp::split_entity_events`). With
//! `KRONIKA_ENTITY_EVENTS=1` the three log carriers (OTLP/gRPC, OTLP/HTTP,
//! OTel-Arrow) store such records in `entity_versions` and keep them out of
//! `logs`. Without the flag nothing here runs: a logs export is converted
//! and stored exactly as before.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use tumult_lake::EntityOutcome;
use tumult_otlp::{EntityDrop, EntityOptions};

use crate::log_guard::LogLimiter;
use crate::writer::Batch;

/// Set to `1` (or `true`) to store entity events as entities.
pub const FLAG_ENV: &str = "KRONIKA_ENTITY_EVENTS";
/// Comma-separated label names kept from `k8s.<kind>.label.<name>` keys.
pub const LABEL_ALLOW_ENV: &str = "KRONIKA_ENTITY_LABEL_ALLOW";
/// Comma-separated description keys kept whatever they are: a whole key,
/// or the start of one followed by `*`.
pub const ATTRIBUTE_ALLOW_ENV: &str = "KRONIKA_ENTITY_ATTRIBUTE_ALLOW";

/// The observer recorded for events that arrived with the ingest token.
/// The daemon has one shared token, so it is one observer; the name says
/// nothing about the token's value and stays the same when it is rotated.
const TOKEN_OBSERVER: &str = "token:default";
/// The observer recorded when ingest runs without a token (loopback).
const ANONYMOUS_OBSERVER: &str = "anonymous";

/// The settings of entity events as the environment gives them.
#[derive(Debug, Clone, Copy, Default)]
pub struct EntitySettings<'a> {
    /// `KRONIKA_ENTITY_EVENTS`.
    pub flag: Option<&'a str>,
    /// `KRONIKA_ENTITY_LABEL_ALLOW`.
    pub label_allow: Option<&'a str>,
    /// `KRONIKA_ENTITY_ATTRIBUTE_ALLOW`.
    pub attribute_allow: Option<&'a str>,
    /// `KRONIKA_INGEST_TOKEN`.
    pub ingest_token: Option<&'a str>,
}

/// Entity events switched on, with what the conversion needs to know.
#[derive(Debug, Clone)]
pub struct EntityEvents {
    options: Arc<EntityOptions>,
}

impl EntityEvents {
    /// Read the flag and the label allow-list from the environment;
    /// `None` when the flag is not set. `ingest_token` is the configured
    /// `KRONIKA_INGEST_TOKEN`.
    #[must_use]
    pub fn from_env(ingest_token: Option<&str>) -> Option<Self> {
        let flag = std::env::var(FLAG_ENV).ok();
        let labels = std::env::var(LABEL_ALLOW_ENV).ok();
        let attributes = std::env::var(ATTRIBUTE_ALLOW_ENV).ok();
        Self::from_settings(&EntitySettings {
            flag: flag.as_deref(),
            label_allow: labels.as_deref(),
            attribute_allow: attributes.as_deref(),
            ingest_token,
        })
    }

    /// [`Self::from_env`] with the values given: the flag switches entity
    /// events on when it is exactly `1` or `true`; the two allow-lists are
    /// comma-separated.
    #[must_use]
    pub fn from_settings(settings: &EntitySettings<'_>) -> Option<Self> {
        if !matches!(settings.flag, Some("1" | "true")) {
            return None;
        }
        let label_allow = comma_separated(settings.label_allow);
        let observer = match settings.ingest_token {
            Some(_) => TOKEN_OBSERVER,
            None => ANONYMOUS_OBSERVER,
        };
        Some(Self {
            options: Arc::new(EntityOptions {
                observer: observer.to_string(),
                label_allow,
                attribute_allow: comma_separated(settings.attribute_allow),
            }),
        })
    }

    /// What the conversion is given.
    #[must_use]
    pub fn options(&self) -> &EntityOptions {
        &self.options
    }
}

/// The entries of a comma-separated list, without the empty ones.
fn comma_separated(list: Option<&str>) -> Vec<String> {
    list.unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect()
}

/// The batch a logs export becomes. Without entity events, and for an
/// export that carries none, that is the logs batch it always was.
pub(crate) fn logs_batch(
    entities: Option<&EntityEvents>,
    request: &ExportLogsServiceRequest,
    received_at_ns: i64,
) -> Batch {
    let Some(entities) = entities else {
        return Batch::Logs(tumult_otlp::logs_request_to_rows(request, received_at_ns));
    };
    let split = tumult_otlp::split_entity_events(request, received_at_ns, entities.options());
    if split.entities.is_empty() {
        return Batch::Logs(split.logs);
    }
    Batch::LogsAndEntities {
        logs: split.logs,
        entities: split.entities,
    }
}

/// Every outcome, in the order of the counters.
const OUTCOMES: [EntityOutcome; 8] = [
    EntityOutcome::Opened,
    EntityOutcome::Refreshed,
    EntityOutcome::Tombstoned,
    EntityOutcome::StaleState,
    EntityOutcome::DeleteUnknown,
    EntityOutcome::DeleteRepeated,
    EntityOutcome::DeleteOutOfOrder,
    EntityOutcome::DeleteForeignObserver,
];

static OUTCOME_COUNTS: [AtomicU64; 8] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];

/// Limits the warning about deletes from another observer: whoever holds
/// a way in can cause it at will.
static FOREIGN_DELETES: LogLimiter = LogLimiter::warnings();

fn outcome_name(outcome: EntityOutcome) -> &'static str {
    match outcome {
        EntityOutcome::Opened => "opened",
        EntityOutcome::Refreshed => "refreshed",
        EntityOutcome::Tombstoned => "tombstoned",
        EntityOutcome::StaleState => "stale_state",
        EntityOutcome::DeleteUnknown => "delete_unknown",
        EntityOutcome::DeleteRepeated => "delete_repeated",
        EntityOutcome::DeleteOutOfOrder => "delete_out_of_order",
        EntityOutcome::DeleteForeignObserver => "delete_foreign_observer",
    }
}

fn outcome_counter(outcome: EntityOutcome) -> &'static AtomicU64 {
    let index = match outcome {
        EntityOutcome::Opened => 0,
        EntityOutcome::Refreshed => 1,
        EntityOutcome::Tombstoned => 2,
        EntityOutcome::StaleState => 3,
        EntityOutcome::DeleteUnknown => 4,
        EntityOutcome::DeleteRepeated => 5,
        EntityOutcome::DeleteOutOfOrder => 6,
        EntityOutcome::DeleteForeignObserver => 7,
    };
    &OUTCOME_COUNTS[index]
}

/// Entity events stored since start with this outcome.
#[must_use]
pub fn outcome_count(outcome: EntityOutcome) -> u64 {
    outcome_counter(outcome).load(Ordering::Relaxed)
}

/// Count what the store did with a batch of entity events. A delete from
/// an observer that did not report the entity is also logged: it is either
/// a misconfigured sender or an attempt to remove someone else's entity.
pub(crate) fn note_outcomes(outcomes: &[EntityOutcome]) {
    let mut foreign_deletes = 0_u64;
    for outcome in outcomes {
        outcome_counter(*outcome).fetch_add(1, Ordering::Relaxed);
        if *outcome == EntityOutcome::DeleteForeignObserver {
            foreign_deletes += 1;
        }
    }
    if foreign_deletes == 0 {
        return;
    }
    let Some(suppressed) = FOREIGN_DELETES.allow("foreign_delete") else {
        return;
    };
    tracing::warn!(
        name: "ingest.entity_event.delete_refused",
        {
            entity.event.refused_in_batch = foreign_deletes,
            entity.event.refused_total = outcome_count(EntityOutcome::DeleteForeignObserver),
            log.suppressed = suppressed,
        },
        "entity delete from another observer ignored"
    );
}

fn push_labelled(out: &mut String, name: &str, help: &str, label: &str, values: &[(&str, u64)]) {
    out.push_str(&format!("# HELP {name} {help}\n# TYPE {name} counter\n"));
    for (value, count) in values {
        out.push_str(&format!("{name}{{{label}=\"{value}\"}} {count}\n"));
    }
}

/// Append the entity-event counters in Prometheus text exposition format.
pub fn render_prometheus(out: &mut String) {
    let outcomes: Vec<(&str, u64)> = OUTCOMES
        .iter()
        .map(|outcome| (outcome_name(*outcome), outcome_count(*outcome)))
        .collect();
    push_labelled(
        out,
        "tumultd_entity_events_total",
        "Entity events applied to the store, by what they did.",
        "outcome",
        &outcomes,
    );
    let drops: Vec<(&str, u64)> = EntityDrop::ALL
        .iter()
        .map(|reason| (reason.as_str(), tumult_otlp::entity_events_dropped(*reason)))
        .collect();
    push_labelled(
        out,
        "tumultd_entity_events_dropped_total",
        "Entity events dropped at ingest, by reason.",
        "reason",
        &drops,
    );
    let name = "tumultd_entity_event_keys_filtered_total";
    out.push_str(&format!(
        "# HELP {name} Keys removed from descriptions and relationships of stored entity events.\n\
         # TYPE {name} counter\n{name} {}\n",
        tumult_otlp::entity_event_keys_filtered_total()
    ));
}

#[cfg(test)]
pub(crate) mod tests;
