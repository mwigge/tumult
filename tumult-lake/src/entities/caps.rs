//! Bounds on how far `entity_versions` can grow.
//!
//! Whoever can send an entity event chooses its type, its id and its
//! environment, and can change the description as often as it likes. Left
//! alone the table would grow with whatever a sender invents, so:
//!
//! * an entity keeps at most [`EntityLimits::versions_per_entity`] rows per
//!   environment; when a new row takes it past that, its oldest rows go;
//! * an environment holds at most
//!   [`EntityLimits::entities_per_environment`] entities and the lake at
//!   most [`EntityLimits::entities_total`]. A state for an entity that has
//!   no row first makes room at either bound by evicting entities nobody
//!   reports any more (see [`Writer::admit`]) and is refused only when
//!   there are none;
//! * [`Writer::prune_entities`] removes, with all their rows, the entities
//!   that are not live and were last heard of before a cutoff;
//! * [`Writer::purge_entities`] removes what an operator names.
//!
//! The writer keeps the number of entities per environment in memory (the
//! [`Census`]): counted from the table when first needed and adjusted by
//! every statement here that adds or removes an entity. Counting them anew
//! for every export that names an unknown entity made such an export cost
//! a scan of the whole table. The daemon has one writer, so nothing else
//! changes the table behind it; a transaction that fails forgets the count
//! and the next one counts again.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};

use duckdb::params;

use super::Slot;
use crate::error::StoreError;
use crate::store::with_tx;
use crate::Writer;

/// Rows kept per entity and environment unless [`EntityLimits`] says
/// otherwise.
pub const ENTITY_VERSIONS_PER_ENTITY: usize = 100;

/// Entities held per environment unless [`EntityLimits`] says otherwise.
pub const ENTITIES_PER_ENVIRONMENT: usize = 50_000;

/// The lake as a whole holds this many times the bound of one environment.
/// The environment is named by the sender, so a bound per environment alone
/// bounds nothing.
const ENVIRONMENTS_WORTH: usize = 4;

/// Seconds after which an entity that sends no periodic reports
/// (`interval_s = 0`) and was not heard of may be evicted to make room,
/// unless [`EntityLimits`] says otherwise: one day. Such an entity never
/// expires, so without this a sender could fill an environment for good
/// with entities it reports once.
pub const ENTITY_UNREPORTED_AFTER_S: i64 = 86_400;

/// Entities evicted at once, at most, whatever the bound is.
const EVICTION_CHUNK_MAX: usize = 5_000;

/// How long the writer remembers that a bound held nothing to evict, in
/// nanoseconds. Looking costs a pass over the table, and a sender that
/// keeps naming new entities would otherwise make every export pay it.
const NOTHING_TO_EVICT_FOR_NS: i64 = 60 * 1_000_000_000;

/// How far `entity_versions` may grow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityLimits {
    /// Rows kept per entity and environment; at least the latest is kept.
    pub versions_per_entity: usize,
    /// Entities held per environment.
    pub entities_per_environment: usize,
    /// Entities held over all environments.
    pub entities_total: usize,
    /// Seconds without a report after which an entity that sends no
    /// periodic reports may be evicted at a bound.
    pub unreported_after_s: i64,
}

impl Default for EntityLimits {
    fn default() -> Self {
        Self::per_environment(ENTITIES_PER_ENVIRONMENT)
    }
}

impl EntityLimits {
    /// The default limits with `entities` allowed per environment and four
    /// times as many over all environments.
    #[must_use]
    pub fn per_environment(entities: usize) -> Self {
        Self {
            versions_per_entity: ENTITY_VERSIONS_PER_ENTITY,
            entities_per_environment: entities,
            entities_total: entities.saturating_mul(ENVIRONMENTS_WORTH),
            unreported_after_s: ENTITY_UNREPORTED_AFTER_S,
        }
    }

    /// How many entities one eviction removes at most: a hundredth of the
    /// bound of an environment, at least one. Finding them costs a pass
    /// over the table, so room is made for more than the one entity that
    /// asked.
    #[must_use]
    pub fn eviction_chunk(&self) -> usize {
        (self.entities_per_environment / 100).clamp(1, EVICTION_CHUNK_MAX)
    }
}

/// Why an entity was evicted to make room for a new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvictionReason {
    /// It was deleted.
    Gone,
    /// It missed three report intervals.
    Expired,
    /// It sends no periodic reports and was not heard of for
    /// [`EntityLimits::unreported_after_s`].
    Unreported,
}

impl EvictionReason {
    /// Every reason, in the order of the metric.
    pub const ALL: [Self; 3] = [Self::Gone, Self::Expired, Self::Unreported];

    /// The reason as it appears in metrics.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gone => "gone",
            Self::Expired => "expired",
            Self::Unreported => "unreported",
        }
    }

    fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|reason| reason.as_str() == name)
    }
}

/// Entities evicted to make room, by reason.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EntityEvictions {
    pub gone: usize,
    pub expired: usize,
    pub unreported: usize,
}

impl EntityEvictions {
    /// How many were evicted for `reason`.
    #[must_use]
    pub fn count(&self, reason: EvictionReason) -> usize {
        match reason {
            EvictionReason::Gone => self.gone,
            EvictionReason::Expired => self.expired,
            EvictionReason::Unreported => self.unreported,
        }
    }

    /// How many were evicted in all.
    #[must_use]
    pub fn total(&self) -> usize {
        self.gone + self.expired + self.unreported
    }

    fn add(&mut self, reason: EvictionReason, entities: usize) {
        match reason {
            EvictionReason::Gone => self.gone += entities,
            EvictionReason::Expired => self.expired += entities,
            EvictionReason::Unreported => self.unreported += entities,
        }
    }
}

/// Which bound has no room.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Full {
    /// The bound of this environment: room is made in it.
    Environment(String),
    /// The bound of the lake only: room is made wherever it can be.
    Lake,
}

impl Full {
    fn environment(&self) -> Option<&str> {
        match self {
            Self::Environment(environment) => Some(environment),
            Self::Lake => None,
        }
    }
}

/// How many entities the lake holds, per environment and in all. An
/// environment without entities has no entry.
#[derive(Debug, Default, PartialEq, Eq)]
struct Census {
    per_environment: HashMap<String, usize>,
    total: usize,
}

impl Census {
    fn held(&self, environment: &str) -> usize {
        self.per_environment.get(environment).copied().unwrap_or(0)
    }

    /// The bound that has no room for one more entity in `environment`.
    fn full(&self, environment: &str, limits: &EntityLimits) -> Option<Full> {
        if self.held(environment) >= limits.entities_per_environment {
            return Some(Full::Environment(environment.to_owned()));
        }
        (self.total >= limits.entities_total).then_some(Full::Lake)
    }

    fn add(&mut self, environment: &str) {
        *self
            .per_environment
            .entry(environment.to_owned())
            .or_insert(0) += 1;
        self.total += 1;
    }

    fn remove(&mut self, environment: &str, entities: usize) {
        let left = self.held(environment).saturating_sub(entities);
        self.total = self.total.saturating_sub(entities);
        if left == 0 {
            self.per_environment.remove(environment);
            return;
        }
        self.per_environment.insert(environment.to_owned(), left);
    }
}

/// What the writer remembers about the entity table between batches.
#[derive(Default)]
pub(crate) struct EntityBook {
    /// `None` until first needed and after a failed transaction.
    census: RefCell<Option<Census>>,
    /// The bounds that held nothing to evict, with the time until which
    /// that is taken as still true.
    nothing_to_evict: RefCell<HashMap<Full, i64>>,
    /// Statements run on the entity table and its staging tables.
    statements: Cell<u64>,
}

impl EntityBook {
    /// Count one statement.
    pub(super) fn ran(&self) {
        self.statements.set(self.statements.get() + 1);
    }

    /// Whatever the table holds now is not what was counted.
    fn forget(&self) {
        self.census.borrow_mut().take();
        self.look_again();
    }

    /// Something became evictable, or may have.
    pub(super) fn look_again(&self) {
        self.nothing_to_evict.borrow_mut().clear();
    }

    fn found_nothing(&self, full: &Full, now_ns: i64) -> bool {
        self.nothing_to_evict
            .borrow()
            .get(full)
            .is_some_and(|until_ns| now_ns < *until_ns)
    }

    /// Remember that `full` held nothing to evict. The environments are
    /// named by the sender, so what is remembered is dropped once it is
    /// out of date.
    fn note_nothing(&self, full: Full, now_ns: i64) {
        let mut noted = self.nothing_to_evict.borrow_mut();
        noted.retain(|_, until_ns| now_ns < *until_ns);
        noted.insert(full, now_ns.saturating_add(NOTHING_TO_EVICT_FOR_NS));
    }
}

const CENSUS: &str = "
SELECT environment, count(DISTINCT entity_key)
FROM entity_versions
GROUP BY environment";

const CAP_VERSIONS: &str = "
DELETE FROM entity_versions WHERE rowid IN (
    SELECT rowid FROM entity_versions
    WHERE (entity_key, environment) IN (
        SELECT entity_key, environment FROM entity_batch_keys
    )
    QUALIFY row_number() OVER (
        PARTITION BY entity_key, environment ORDER BY valid_from_ns DESC
    ) > ?
)";

/// The entities to remove are staged first, so that they can be counted
/// per environment before they go.
const VICTIMS: &str = "
CREATE TEMP TABLE IF NOT EXISTS entity_victims (
    entity_key VARCHAR,
    environment VARCHAR,
    reason VARCHAR
);
DELETE FROM entity_victims;";

const STAGE_PRUNED: &str = "
INSERT INTO entity_victims
SELECT entity_key, environment, 'retention' FROM entities_current
WHERE liveness <> 'live' AND last_received_ns < ?
ORDER BY last_received_ns, entity_key, environment
LIMIT ?";

/// The entities that may be evicted: those that are gone or expired,
/// longest silent first, then those that send no periodic reports and were
/// last heard of before the third parameter. The first two parameters
/// confine the search to one environment; what the batch itself names (the
/// staged keys) is left alone, since the batch may be about to report it.
const STAGE_EVICTED: &str = "
INSERT INTO entity_victims
SELECT entity_key, environment,
       CASE liveness
           WHEN 'gone' THEN 'gone'
           WHEN 'expired' THEN 'expired'
           ELSE 'unreported'
       END
FROM entities_current
WHERE (CAST(? AS VARCHAR) IS NULL OR environment = ?)
    AND (liveness <> 'live' OR (interval_s = 0 AND last_received_ns < ?))
    AND (entity_key, environment) NOT IN (
        SELECT entity_key, environment FROM entity_batch_keys
    )
ORDER BY liveness = 'live', last_received_ns, entity_key, environment
LIMIT ?";

const STAGE_PURGED: &str = "
INSERT INTO entity_victims
SELECT DISTINCT entity_key, environment, 'purge' FROM entity_versions
WHERE environment = ?
    AND (CAST(? AS VARCHAR) IS NULL OR entity_type = ?)";

const COUNT_VICTIMS: &str = "
SELECT environment, reason, count(*)
FROM entity_victims
GROUP BY environment, reason";

const DELETE_VICTIMS: &str = "
DELETE FROM entity_versions WHERE (entity_key, environment) IN (
    SELECT entity_key, environment FROM entity_victims
)";

fn as_param(count: usize) -> i64 {
    i64::try_from(count).unwrap_or(i64::MAX)
}

/// What a removal took: the rows, and the entities by environment and by
/// the reason they were staged with.
struct Removed {
    rows: usize,
    entities: Vec<(String, String, usize)>,
}

impl Removed {
    fn entity_count(&self) -> usize {
        self.entities.iter().map(|(_, _, entities)| entities).sum()
    }
}

/// What [`Writer::purge_entities`] removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EntityPurge {
    pub entities: usize,
    pub versions: usize,
}

impl Writer {
    fn read_census(&self) -> Result<Census, StoreError> {
        self.entities.ran();
        let mut stmt = self.conn.prepare(CENSUS)?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        let mut census = Census::default();
        for row in rows {
            let (environment, entities) = row?;
            let entities = usize::try_from(entities).unwrap_or(usize::MAX);
            census.total = census.total.saturating_add(entities);
            census.per_environment.insert(environment, entities);
        }
        Ok(census)
    }

    /// Run `read` on the census, counting the table first when the writer
    /// holds no count.
    fn with_census<T>(&self, read: impl FnOnce(&mut Census) -> T) -> Result<T, StoreError> {
        let mut held = self.entities.census.borrow_mut();
        let census = match held.take() {
            Some(census) => census,
            None => self.read_census()?,
        };
        Ok(read(held.insert(census)))
    }

    /// Run `write` in a transaction. When it fails, the count the writer
    /// holds is forgotten: it may include what was rolled back.
    pub(super) fn entity_tx<T>(
        &self,
        write: impl FnOnce() -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let result = with_tx(&self.conn, write);
        if result.is_err() {
            self.entities.forget();
        }
        result
    }

    /// The entities per environment as the writer counts them in memory
    /// (counted from the table when it holds no count).
    ///
    /// # Errors
    /// Returns an error if the table has to be counted and cannot be.
    pub fn entities_held(&self) -> Result<BTreeMap<String, usize>, StoreError> {
        self.with_census(|census| {
            census
                .per_environment
                .iter()
                .map(|(environment, entities)| (environment.clone(), *entities))
                .collect()
        })
    }

    /// Statements this writer ran on the entity table and its staging
    /// tables. What a batch costs is a number of statements that does not
    /// grow with its events; a test holds it to that.
    #[doc(hidden)]
    #[must_use]
    pub fn entity_statements(&self) -> u64 {
        self.entities.statements.get()
    }

    /// Whether there is room for one more entity in `environment`,
    /// counting it when there is. At a bound, room is made first: entities
    /// that are gone or expired go, longest silent first, then entities
    /// that send no periodic reports and were not heard of for
    /// [`EntityLimits::unreported_after_s`]. An entity that is live and
    /// reports is never evicted, so the answer is `false` when the bound
    /// is full of those.
    pub(super) fn admit(
        &self,
        environment: &str,
        limits: &EntityLimits,
        evicted: &mut EntityEvictions,
    ) -> Result<bool, StoreError> {
        let full = self.with_census(|census| census.full(environment, limits))?;
        if let Some(full) = full {
            // Room in the environment is room in the lake too, and the
            // lake is only looked at when the environment has room.
            if !self.evict(&full, limits, evicted)? {
                return Ok(false);
            }
        }
        self.with_census(|census| census.add(environment))?;
        Ok(true)
    }

    /// Evict up to [`EntityLimits::eviction_chunk`] entities from what is
    /// `full`; whether any went.
    fn evict(
        &self,
        full: &Full,
        limits: &EntityLimits,
        evicted: &mut EntityEvictions,
    ) -> Result<bool, StoreError> {
        let now_ns = (self.clock)();
        if self.entities.found_nothing(full, now_ns) {
            return Ok(false);
        }
        let silence_ns = limits.unreported_after_s.saturating_mul(1_000_000_000);
        let environment = full.environment();
        self.prepare_victims()?;
        self.entities.ran();
        self.conn.execute(
            STAGE_EVICTED,
            params![
                environment,
                environment,
                now_ns.saturating_sub(silence_ns),
                as_param(limits.eviction_chunk())
            ],
        )?;
        let removed = self.remove_victims()?;
        if removed.rows == 0 {
            self.entities.note_nothing(full.clone(), now_ns);
            return Ok(false);
        }
        for (_, reason, entities) in &removed.entities {
            if let Some(reason) = EvictionReason::named(reason) {
                evicted.add(reason, *entities);
            }
        }
        Ok(true)
    }

    fn prepare_victims(&self) -> Result<(), StoreError> {
        self.entities.ran();
        Ok(self.conn.execute_batch(VICTIMS)?)
    }

    /// Remove every row of the staged entities and take them out of the
    /// count.
    fn remove_victims(&self) -> Result<Removed, StoreError> {
        self.entities.ran();
        let mut stmt = self.conn.prepare(COUNT_VICTIMS)?;
        let counted = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        let mut entities = Vec::new();
        for row in counted {
            let (environment, reason, count) = row?;
            entities.push((environment, reason, usize::try_from(count).unwrap_or(0)));
        }
        if entities.is_empty() {
            return Ok(Removed { rows: 0, entities });
        }
        // A writer that holds no count takes it before the delete: taken
        // after, it would lack the victims already and lose them twice.
        self.with_census(|_| ())?;
        self.entities.ran();
        let rows = self.conn.execute(DELETE_VICTIMS, [])?;
        self.with_census(|census| {
            for (environment, _, count) in &entities {
                census.remove(environment, *count);
            }
        })?;
        Ok(Removed { rows, entities })
    }

    /// Keep the `keep` newest rows (at least the latest) of each entity in
    /// `slots`; returns how many older rows went.
    pub(super) fn cap_entity_versions(
        &self,
        slots: &[Slot],
        keep: usize,
    ) -> Result<usize, StoreError> {
        if slots.is_empty() {
            return Ok(0);
        }
        self.stage_entity_keys(slots.iter())?;
        self.entities.ran();
        Ok(self
            .conn
            .execute(CAP_VERSIONS, params![as_param(keep.max(1))])?)
    }

    /// Remove every row of at most `limit` entities that are gone or
    /// expired and were last heard of before `cutoff_ns` (by the daemon's
    /// clock), longest silent first; returns how many rows went. A live
    /// entity is never removed, however old.
    ///
    /// # Errors
    /// Returns an error if the delete fails (nothing is removed).
    pub fn prune_entities(&self, cutoff_ns: i64, limit: usize) -> Result<usize, StoreError> {
        self.entity_tx(|| {
            self.prepare_victims()?;
            self.entities.ran();
            self.conn
                .execute(STAGE_PRUNED, params![cutoff_ns, as_param(limit)])?;
            Ok(self.remove_victims()?.rows)
        })
    }

    /// Remove every row of every entity of `environment`, or of those of
    /// `entity_type` in it. For an operator: nothing else removes an
    /// entity that is live and reports.
    ///
    /// # Errors
    /// Returns an error if the delete fails (nothing is removed).
    pub fn purge_entities(
        &self,
        environment: &str,
        entity_type: Option<&str>,
    ) -> Result<EntityPurge, StoreError> {
        let removed = self.entity_tx(|| {
            self.prepare_victims()?;
            self.entities.ran();
            self.conn
                .execute(STAGE_PURGED, params![environment, entity_type, entity_type])?;
            self.remove_victims()
        })?;
        Ok(EntityPurge {
            entities: removed.entity_count(),
            versions: removed.rows,
        })
    }

    /// The number of rows in `entity_versions`.
    ///
    /// # Errors
    /// Returns an error if the count fails.
    pub fn entity_version_count(&self) -> Result<u64, StoreError> {
        let rows: i64 = self
            .conn
            .query_row("SELECT count(*) FROM entity_versions", [], |row| row.get(0))?;
        Ok(u64::try_from(rows).unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EntityEventKind, EntityEventRow, Store, WireForm};

    /// 2100-01-01: ahead of any clock, so the entity is live.
    const FUTURE: i64 = 4_102_444_800_000_000_000;

    fn host(id: &str) -> EntityEventRow {
        EntityEventRow {
            kind: EntityEventKind::State,
            ts_ns: FUTURE,
            received_at_ns: FUTURE,
            entity_type: "host".into(),
            entity_id: vec![("host.id".into(), id.into())],
            description: serde_json::json!({}),
            relationships: serde_json::json!([]),
            interval_s: Some(10),
            delete_reason: None,
            wire_form: WireForm::Spec,
            observer: "tok:a".into(),
            schema_url: None,
            environment: "prod".into(),
        }
    }

    fn writer() -> (tempfile::TempDir, Writer) {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Store::open(&dir.path().join("kronika.duckdb")).unwrap();
        let writer = store.writer().unwrap();
        (dir, writer)
    }

    #[test]
    fn the_census_drops_an_environment_that_holds_nothing() {
        let mut census = Census::default();
        census.add("prod");
        census.add("prod");
        census.add("dev");
        census.remove("prod", 1);
        census.remove("dev", 5);
        assert_eq!(census.total, 0, "never below nought");
        assert_eq!(census.held("prod"), 1);
        assert!(!census.per_environment.contains_key("dev"));
    }

    #[test]
    fn the_bound_of_the_environment_is_named_before_the_bound_of_the_lake() {
        let limits = EntityLimits {
            entities_per_environment: 1,
            entities_total: 1,
            ..EntityLimits::default()
        };
        let mut census = Census::default();
        assert_eq!(census.full("prod", &limits), None);
        census.add("prod");
        assert_eq!(
            census.full("prod", &limits),
            Some(Full::Environment("prod".into()))
        );
        assert_eq!(census.full("dev", &limits), Some(Full::Lake));
    }

    /// The count is taken from the table once and then kept: a batch that
    /// names an unknown entity does not count the table again.
    #[test]
    fn the_census_is_counted_once_and_kept_between_batches() {
        let (_dir, writer) = writer();
        let limits = EntityLimits::default();
        assert!(writer.entities.census.borrow().is_none());
        writer.apply_entity_events(&[host("a")], &limits).unwrap();
        assert_eq!(writer.entities.census.borrow().as_ref().unwrap().total, 1);
        // What another connection would do is not seen: the writer is the
        // only one that writes.
        writer
            .conn
            .execute("DELETE FROM entity_versions", [])
            .unwrap();
        writer.apply_entity_events(&[host("b")], &limits).unwrap();
        assert_eq!(writer.entities.census.borrow().as_ref().unwrap().total, 2);
    }

    #[test]
    fn a_failed_transaction_forgets_the_count_and_what_was_not_evictable() {
        let (_dir, writer) = writer();
        writer
            .apply_entity_events(&[host("a")], &EntityLimits::default())
            .unwrap();
        writer.entities.note_nothing(Full::Lake, 0);
        let failed: Result<(), StoreError> =
            writer.entity_tx(|| Err(StoreError::Internal("refused".into())));
        assert!(failed.is_err());
        assert!(writer.entities.census.borrow().is_none());
        assert!(!writer.entities.found_nothing(&Full::Lake, 0));
        assert_eq!(writer.entities_held().unwrap()["prod"], 1);
    }

    /// Nothing evictable is remembered for a minute by the writer's clock
    /// and per bound; the environments are the sender's to name, so what
    /// is out of date is dropped when the next is noted.
    #[test]
    fn nothing_to_evict_is_remembered_for_a_minute_per_bound() {
        let book = EntityBook::default();
        let prod = Full::Environment("prod".into());
        book.note_nothing(prod.clone(), 0);
        assert!(book.found_nothing(&prod, NOTHING_TO_EVICT_FOR_NS - 1));
        assert!(!book.found_nothing(&prod, NOTHING_TO_EVICT_FOR_NS));
        assert!(!book.found_nothing(&Full::Lake, 1));
        book.note_nothing(Full::Lake, NOTHING_TO_EVICT_FOR_NS);
        assert_eq!(book.nothing_to_evict.borrow().len(), 1, "prod was dropped");
        book.look_again();
        assert!(!book.found_nothing(&Full::Lake, NOTHING_TO_EVICT_FOR_NS));
    }

    /// The silence of an entity without reports is measured by the
    /// writer's clock, like the receipt it is measured from.
    #[test]
    fn the_silence_of_an_unreported_entity_is_measured_by_the_writers_clock() {
        let (_dir, mut writer) = writer();
        let limits = EntityLimits::per_environment(1);
        let once = EntityEventRow {
            interval_s: Some(0),
            ..host("once")
        };
        writer.apply_entity_events(&[once], &limits).unwrap();
        let day_ns = ENTITY_UNREPORTED_AFTER_S * 1_000_000_000;
        writer.clock = || FUTURE + ENTITY_UNREPORTED_AFTER_S * 1_000_000_000;
        let applied = writer.apply_entity_events(&[host("a")], &limits).unwrap();
        assert_eq!(applied.evicted, EntityEvictions::default(), "a day exactly");
        assert_eq!(day_ns, 86_400_000_000_000);
        writer.entities.look_again();
        writer.clock = || FUTURE + ENTITY_UNREPORTED_AFTER_S * 1_000_000_000 + 1;
        let applied = writer.apply_entity_events(&[host("a")], &limits).unwrap();
        assert_eq!(applied.evicted.unreported, 1);
    }
}
