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
//!   most [`EntityLimits::entities_total`]; a state for an entity that has
//!   no row is refused beyond either. An entity that is gone or expired
//!   still holds its place, until [`Writer::prune_entities`] removes it;
//! * [`Writer::prune_entities`] removes, with all their rows, the entities
//!   that are not live and were last heard of before a cutoff.

use std::collections::HashMap;

use duckdb::params;

use crate::error::StoreError;
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

/// How far `entity_versions` may grow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityLimits {
    /// Rows kept per entity and environment; at least the latest is kept.
    pub versions_per_entity: usize,
    /// Entities held per environment.
    pub entities_per_environment: usize,
    /// Entities held over all environments.
    pub entities_total: usize,
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
        }
    }
}

/// How many entities the lake holds, per environment and in all.
pub(super) struct Census {
    per_environment: HashMap<String, usize>,
    total: usize,
}

impl Census {
    /// Count one more entity in `environment` when the limits leave room
    /// for it; `false` when they do not.
    pub(super) fn admit(&mut self, environment: &str, limits: &EntityLimits) -> bool {
        let here = self.per_environment.get(environment).copied().unwrap_or(0);
        if here >= limits.entities_per_environment || self.total >= limits.entities_total {
            return false;
        }
        self.per_environment
            .insert(environment.to_owned(), here + 1);
        self.total += 1;
        true
    }
}

const CENSUS: &str = "
SELECT environment, count(DISTINCT entity_key)
FROM entity_versions
GROUP BY environment";

const CAP_VERSIONS: &str = "
DELETE FROM entity_versions WHERE rowid IN (
    SELECT rowid FROM entity_versions
    WHERE entity_key IN (SELECT entity_key FROM entity_batch_keys)
    QUALIFY row_number() OVER (
        PARTITION BY entity_key, environment ORDER BY valid_from_ns DESC
    ) > ?
)";

const PRUNE: &str = "
DELETE FROM entity_versions WHERE (entity_key, environment) IN (
    SELECT entity_key, environment FROM entities_current
    WHERE liveness <> 'live' AND last_received_ns < ?
    ORDER BY last_received_ns, entity_key, environment
    LIMIT ?
)";

fn as_param(count: usize) -> i64 {
    i64::try_from(count).unwrap_or(i64::MAX)
}

impl Writer {
    /// The entities stored now. Read once per batch, and only by a batch
    /// that names an entity without a row.
    pub(super) fn entity_census(&self) -> Result<Census, StoreError> {
        let mut stmt = self.conn.prepare(CENSUS)?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        let mut census = Census {
            per_environment: HashMap::new(),
            total: 0,
        };
        for row in rows {
            let (environment, entities) = row?;
            let entities = usize::try_from(entities).unwrap_or(usize::MAX);
            census.total = census.total.saturating_add(entities);
            census.per_environment.insert(environment, entities);
        }
        Ok(census)
    }

    /// Keep the `keep` newest rows (at least the latest) of each entity in
    /// `keys`, per environment; returns how many older rows went.
    pub(super) fn cap_entity_versions(
        &self,
        keys: &[String],
        keep: usize,
    ) -> Result<usize, StoreError> {
        if keys.is_empty() {
            return Ok(0);
        }
        self.stage_entity_keys(keys.iter())?;
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
    /// Returns an error if the delete fails.
    pub fn prune_entities(&self, cutoff_ns: i64, limit: usize) -> Result<usize, StoreError> {
        Ok(self
            .conn
            .execute(PRUNE, params![cutoff_ns, as_param(limit)])?)
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
