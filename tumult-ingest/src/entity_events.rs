//! Entity events on the ingest paths.
//!
//! An entity event is a log record that carries the state or the deletion
//! of an entity (see `tumult_otlp::split_entity_events`). With
//! `KRONIKA_ENTITY_EVENTS=1` the three log carriers (OTLP/gRPC, OTLP/HTTP,
//! OTel-Arrow) store such records in `entity_versions` and keep them out of
//! `logs`. Without the flag nothing here runs: a logs export is converted
//! and stored exactly as before.

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use tumult_lake::{
    EntityApplied, EntityEvictions, EntityLimits, EntityOutcome, EvictionReason, Writer,
};
use tumult_otlp::{EntityDrop, EntityOptions};

use crate::log_guard::LogLimiter;
use crate::writer::Batch;

/// Set to `1`, `true`, `yes` or `on` (in any case) to store entity events
/// as entities.
pub const FLAG_ENV: &str = "KRONIKA_ENTITY_EVENTS";
/// How much of an unrecognised flag value is written to the log, in bytes.
const FLAG_LOGGED_LEN: usize = 64;
/// Comma-separated label names kept from `k8s.<kind>.label.<name>` keys.
pub const LABEL_ALLOW_ENV: &str = "KRONIKA_ENTITY_LABEL_ALLOW";
/// Comma-separated description keys kept whatever they are: a whole key,
/// or the start of one followed by `*`.
pub const ATTRIBUTE_ALLOW_ENV: &str = "KRONIKA_ENTITY_ATTRIBUTE_ALLOW";

/// The most entities one environment may hold, a positive integer; the
/// lake as a whole holds four times as many. Unset: 50 000.
pub const MAX_PER_ENVIRONMENT_ENV: &str = "KRONIKA_ENTITY_MAX_PER_ENVIRONMENT";

/// Days an entity that is gone or expired is kept after the daemon last
/// heard of it. After that all its rows are removed.
pub const RETENTION_DAYS: i64 = 30;
const RETENTION_NS: i64 = RETENTION_DAYS * 86_400 * 1_000_000_000;
/// Entities past the retention window are looked for this often, and only
/// when entity events arrive.
const PRUNE_EVERY_NS: i64 = 600 * 1_000_000_000;
/// Entities removed per look, so one look never holds the writer long.
const PRUNE_BATCH: usize = 5_000;

/// The observer recorded for events that arrived with the ingest token.
/// The daemon has exactly one ingest token (`KRONIKA_INGEST_TOKEN`), with
/// no id and no per-sender variant, so every sender is this one observer
/// and nothing can be decided by it. The name says nothing about the
/// token's value and stays the same when it is rotated.
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
    /// `KRONIKA_ENTITY_MAX_PER_ENVIRONMENT`.
    pub max_per_environment: Option<&'a str>,
}

/// Entity events switched on, with what the conversion and the store need
/// to know.
#[derive(Debug, Clone)]
pub struct EntityEvents {
    options: Arc<EntityOptions>,
    limits: EntityLimits,
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
        let max_per_environment = std::env::var(MAX_PER_ENVIRONMENT_ENV).ok();
        Self::from_settings(&EntitySettings {
            flag: flag.as_deref(),
            label_allow: labels.as_deref(),
            attribute_allow: attributes.as_deref(),
            ingest_token,
            max_per_environment: max_per_environment.as_deref(),
        })
    }

    /// [`Self::from_env`] with the values given: the flag switches entity
    /// events on when it is `1`, `true`, `yes` or `on` in any case; the two
    /// allow-lists are comma-separated. A flag value that is neither a yes
    /// nor a no leaves entity events off and is logged as a warning.
    #[must_use]
    pub fn from_settings(settings: &EntitySettings<'_>) -> Option<Self> {
        match Flag::parse(settings.flag) {
            Flag::On => {}
            Flag::Off => return None,
            Flag::Unrecognised(value) => {
                warn_unrecognised_flag(value);
                return None;
            }
        }
        let label_allow = comma_separated(settings.label_allow);
        let observer = match settings.ingest_token {
            Some(_) => TOKEN_OBSERVER,
            None => ANONYMOUS_OBSERVER,
        };
        let attribute_allow = comma_separated(settings.attribute_allow);
        attribute_allow
            .iter()
            .filter(|entry| is_broad_pattern(entry))
            .for_each(|entry| warn_broad_pattern(entry));
        Some(Self {
            options: Arc::new(EntityOptions {
                observer: observer.to_string(),
                label_allow,
                attribute_allow,
            }),
            limits: entity_limits(settings.max_per_environment),
        })
    }

    /// How far the entity table may grow.
    #[must_use]
    pub fn limits(&self) -> EntityLimits {
        self.limits
    }

    /// What the conversion is given.
    #[must_use]
    pub fn options(&self) -> &EntityOptions {
        &self.options
    }
}

/// What `KRONIKA_ENTITY_EVENTS` says.
#[derive(Debug, PartialEq, Eq)]
enum Flag<'a> {
    On,
    Off,
    /// Neither a yes nor a no; the value as given.
    Unrecognised(&'a str),
}

impl<'a> Flag<'a> {
    /// Unset and empty are off; spaces around the value and its case do
    /// not matter.
    fn parse(value: Option<&'a str>) -> Self {
        let Some(value) = value else {
            return Self::Off;
        };
        match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Self::On,
            "" | "0" | "false" | "no" | "off" => Self::Off,
            _ => Self::Unrecognised(value),
        }
    }
}

/// Entity events stay off for a value that is neither a yes nor a no, and
/// an operator who wrote one expects them on: say so once, at start.
fn warn_unrecognised_flag(value: &str) {
    tracing::warn!(
        name: "ingest.entity_events.flag_unrecognised",
        {
            config.key = FLAG_ENV,
            config.value = ?tumult_core::compliance::truncated(value.trim(), FLAG_LOGGED_LEN),
        },
        "entity events stay off: the flag is neither a yes nor a no"
    );
}

/// The limits with `KRONIKA_ENTITY_MAX_PER_ENVIRONMENT` as given.
/// Unset and empty give the default; so does anything that is not a
/// positive integer, with a warning.
fn entity_limits(max_per_environment: Option<&str>) -> EntityLimits {
    let Some(value) = max_per_environment
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return EntityLimits::default();
    };
    match value.parse::<usize>() {
        Ok(entities) if entities > 0 => EntityLimits::per_environment(entities),
        _ => {
            warn_unusable_limit(value);
            EntityLimits::default()
        }
    }
}

fn warn_unusable_limit(value: &str) {
    tracing::warn!(
        name: "ingest.entity_events.limit_unrecognised",
        {
            config.key = MAX_PER_ENVIRONMENT_ENV,
            config.value = ?tumult_core::compliance::truncated(value, FLAG_LOGGED_LEN),
        },
        "the entity limit is not a positive integer: the default applies"
    );
}

/// Whether an entry of `KRONIKA_ENTITY_ATTRIBUTE_ALLOW` keeps keys of more
/// than one namespace: `*` alone, or the start of a key that ends before
/// its first dot (`k8s*`, `h*`).
fn is_broad_pattern(entry: &str) -> bool {
    entry
        .strip_suffix('*')
        .is_some_and(|start| !start.contains('.'))
}

/// A pattern without a namespace keeps the labels, the selectors and the
/// annotations of every kind, manifests included. It is the operator's to
/// set and is honoured; it is said once, at start, so that it is not set
/// without being seen.
fn warn_broad_pattern(entry: &str) {
    tracing::warn!(
        name: "ingest.entity_events.attribute_allow_broad",
        {
            config.key = ATTRIBUTE_ALLOW_ENV,
            config.value = ?tumult_core::compliance::truncated(entry, FLAG_LOGGED_LEN),
        },
        "an attribute pattern names no namespace: every key that starts so is stored"
    );
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
        limits: entities.limits(),
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
    EntityOutcome::EntityLimit,
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

fn outcome_name(outcome: EntityOutcome) -> &'static str {
    match outcome {
        EntityOutcome::Opened => "opened",
        EntityOutcome::Refreshed => "refreshed",
        EntityOutcome::Tombstoned => "tombstoned",
        EntityOutcome::StaleState => "stale_state",
        EntityOutcome::DeleteUnknown => "delete_unknown",
        EntityOutcome::DeleteRepeated => "delete_repeated",
        EntityOutcome::DeleteOutOfOrder => "delete_out_of_order",
        EntityOutcome::EntityLimit => "entity_limit",
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
        EntityOutcome::EntityLimit => 7,
    };
    &OUTCOME_COUNTS[index]
}

/// Entity events stored since start with this outcome.
#[must_use]
pub fn outcome_count(outcome: EntityOutcome) -> u64 {
    outcome_counter(outcome).load(Ordering::Relaxed)
}

/// Count what the store did with a batch of entity events.
pub(crate) fn note_outcomes(outcomes: &[EntityOutcome]) {
    for outcome in outcomes {
        outcome_counter(*outcome).fetch_add(1, Ordering::Relaxed);
    }
}

/// Why rows left `entity_versions`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pruned {
    /// The entity had more rows than it may keep.
    VersionCap,
    /// The entity was gone or expired for longer than the retention window.
    Retention,
}

static PRUNED_VERSION_CAP: AtomicU64 = AtomicU64::new(0);
static PRUNED_RETENTION: AtomicU64 = AtomicU64::new(0);
/// Rows in `entity_versions` after the last batch of entity events.
static VERSION_ROWS: AtomicU64 = AtomicU64::new(0);
/// When entities past the retention window were last looked for.
static LAST_PRUNE_NS: AtomicI64 = AtomicI64::new(0);
static REFUSALS: LogLimiter = LogLimiter::warnings();
/// Entities evicted to make room, in the order of [`EvictionReason::ALL`].
static EVICTED: [AtomicU64; 3] = [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)];
static EVICTIONS: LogLimiter = LogLimiter::warnings();

fn evicted_counter(reason: EvictionReason) -> &'static AtomicU64 {
    let index = match reason {
        EvictionReason::Gone => 0,
        EvictionReason::Expired => 1,
        EvictionReason::Unreported => 2,
    };
    &EVICTED[index]
}

/// Entities removed since start to make room for new ones, for `reason`.
#[must_use]
pub fn entities_evicted(reason: EvictionReason) -> u64 {
    evicted_counter(reason).load(Ordering::Relaxed)
}

fn add_evicted(evicted: &EntityEvictions) {
    for reason in EvictionReason::ALL {
        let entities = u64::try_from(evicted.count(reason)).unwrap_or(u64::MAX);
        evicted_counter(reason).fetch_add(entities, Ordering::Relaxed);
    }
}

/// Say, at a bounded rate, that entities were removed to make room: it is
/// what the table is meant to do at a bound, and an operator who misses
/// an entity should be able to find out why.
fn log_evicted(limiter: &LogLimiter, evicted: &EntityEvictions) {
    if evicted.total() == 0 {
        return;
    }
    let Some(suppressed) = limiter.allow("entity_evicted") else {
        return;
    };
    tracing::info!(
        name: "ingest.entity_events.evicted",
        {
            config.key = MAX_PER_ENVIRONMENT_ENV,
            entity.evicted.gone = evicted.gone,
            entity.evicted.expired = evicted.expired,
            entity.evicted.unreported = evicted.unreported,
            log.suppressed = suppressed,
        },
        "entities nobody reports any more were removed to make room for new ones"
    );
}

fn pruned_counter(reason: Pruned) -> &'static AtomicU64 {
    match reason {
        Pruned::VersionCap => &PRUNED_VERSION_CAP,
        Pruned::Retention => &PRUNED_RETENTION,
    }
}

/// Rows removed from `entity_versions` since start for `reason`.
#[must_use]
pub fn versions_pruned(reason: Pruned) -> u64 {
    pruned_counter(reason).load(Ordering::Relaxed)
}

fn add_pruned(reason: Pruned, rows: usize) {
    let rows = u64::try_from(rows).unwrap_or(u64::MAX);
    pruned_counter(reason).fetch_add(rows, Ordering::Relaxed);
}

/// What follows a stored batch of entity events: the counters, the warning
/// for refused entities and the upkeep of the table. The batch is stored
/// by now, so nothing here fails it.
pub(crate) fn after_write(writer: &Writer, applied: &EntityApplied) {
    note_outcomes(&applied.outcomes);
    add_pruned(Pruned::VersionCap, applied.versions_pruned);
    add_evicted(&applied.evicted);
    log_evicted(&EVICTIONS, &applied.evicted);
    let refused = applied
        .outcomes
        .iter()
        .filter(|outcome| **outcome == EntityOutcome::EntityLimit)
        .count();
    warn_refused(&REFUSALS, refused);
    housekeeping(writer, &LAST_PRUNE_NS, crate::now_ns());
}

/// Say, at a bounded rate, that entities were refused for lack of room.
fn warn_refused(limiter: &LogLimiter, refused: usize) {
    if refused == 0 {
        return;
    }
    let Some(suppressed) = limiter.allow("entity_limit") else {
        return;
    };
    tracing::warn!(
        name: "ingest.entity_events.limit_reached",
        {
            config.key = MAX_PER_ENVIRONMENT_ENV,
            entity.event.refused = refused,
            log.suppressed = suppressed,
        },
        "new entities refused: the limit is reached and every entity under it is live"
    );
}

/// Whether entities past the retention window are due to be looked for,
/// `last` being when they last were. Marks the look as done.
fn prune_due(last: &AtomicI64, now_ns: i64) -> bool {
    if now_ns.saturating_sub(last.load(Ordering::Relaxed)) < PRUNE_EVERY_NS {
        return false;
    }
    last.store(now_ns, Ordering::Relaxed);
    true
}

/// Remove what left the retention window, when that is due, and count the
/// rows of the table. A failure is logged and waits for the next batch:
/// the batch that led here is stored.
fn housekeeping(writer: &Writer, last: &AtomicI64, now_ns: i64) {
    if prune_due(last, now_ns) {
        match writer.prune_entities(now_ns.saturating_sub(RETENTION_NS), PRUNE_BATCH) {
            Ok(rows) => add_pruned(Pruned::Retention, rows),
            Err(error) => warn_upkeep_failed("prune", &error),
        }
    }
    match writer.entity_version_count() {
        Ok(rows) => VERSION_ROWS.store(rows, Ordering::Relaxed),
        Err(error) => warn_upkeep_failed("count", &error),
    }
}

fn warn_upkeep_failed(step: &'static str, error: &tumult_lake::StoreError) {
    tracing::warn!(
        name: "ingest.entity_events.upkeep_failed",
        {
            step,
            error.message = %error,
        },
        "upkeep of the entity table failed"
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
    push_labelled(
        out,
        "tumultd_entity_versions_pruned_total",
        "Rows removed from entity_versions, by reason.",
        "reason",
        &[
            ("version_cap", versions_pruned(Pruned::VersionCap)),
            ("retention", versions_pruned(Pruned::Retention)),
        ],
    );
    let evicted: Vec<(&str, u64)> = EvictionReason::ALL
        .iter()
        .map(|reason| (reason.as_str(), entities_evicted(*reason)))
        .collect();
    push_labelled(
        out,
        "tumultd_entity_evictions_total",
        "Entities removed with all their rows to make room at a limit, by reason.",
        "reason",
        &evicted,
    );
    let name = "tumultd_entity_versions";
    out.push_str(&format!(
        "# HELP {name} Rows in entity_versions after the last batch of entity events.\n\
         # TYPE {name} gauge\n{name} {}\n",
        VERSION_ROWS.load(Ordering::Relaxed)
    ));
    let name = "tumultd_entity_event_keys_filtered_total";
    out.push_str(&format!(
        "# HELP {name} Keys removed from descriptions and relationships of stored entity events.\n\
         # TYPE {name} counter\n{name} {}\n",
        tumult_otlp::entity_event_keys_filtered_total()
    ));
}

#[cfg(test)]
pub(crate) mod tests;
