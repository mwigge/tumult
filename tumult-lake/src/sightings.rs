//! Which `service.version`s a service has reported, per environment, when
//! each was last deployed, and which of them served a given window.
//!
//! `service_version_sightings` is a rollup of `spans`: one row per
//! (service, environment, version) with the first and the last time the
//! service's own telemetry carried that version there, and when its
//! latest deployment began. The writer keeps it current inside the
//! transaction that inserts the spans, so reading a service's deployment
//! history costs a scan of a table whose size follows the number of
//! versions, never the number of spans.
//!
//! The key is kept in code: the one writer reads a service's rows, folds
//! the new sightings in and writes back the rows that changed, an update
//! where the key is held and an insert where it is not. The table carries
//! no unique index because its rows are updated in place (the lake's
//! crash-robustness rule for updated tables), and its environment may be
//! NULL, which a primary key does not admit.
//!
//! A version is deployed again when it speaks after a silence longer than
//! [`REDEPLOY_GAP`] during which another version of the service spoke in
//! the same environment: a rollback, or a redeploy after a newer build.
//!
//! A span that arrives late moves a version's first sight back by at most
//! [`LATE_SPAN_LIMIT`]; one from earlier than that is left out.
//!
//! A lake written before the rollup existed, or before it knew
//! deployments, is read back from its spans once, a day of spans at a
//! step: a bounded stretch when the lake is opened, then one step with
//! every batch of spans stored, until `sightings_backfill` says done.
//! While that runs the rollup is incomplete, and [`Reader`] says so.
//!
//! An environment is matched exactly, and spans that name none are an
//! environment of their own (`None`): they never stand in for a named one.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use duckdb::{params, Connection};
use tumult_core::runner::{RunWindow, TargetVersionResolver};
use tumult_core::types::{bounded_version_text, VersionShare, MAX_VERSION_LEN};

use crate::error::StoreError;
use crate::rows::SpanRow;
use crate::store::with_tx;
use crate::{Reader, Store, Writer};

/// The silence after which a version that speaks again, another having
/// spoken meanwhile, counts as deployed anew.
pub const REDEPLOY_GAP: Duration = Duration::from_secs(600);

/// How far before a version's first sight a span that arrives later may
/// move that sight back. A span from earlier still (a clock gone wrong,
/// a replay) is kept as a span and left out of the rollup: one such span
/// would otherwise make a new version read as the oldest.
pub const LATE_SPAN_LIMIT: Duration = Duration::from_secs(600);

/// How long opening a lake may spend reading an old lake's spans back
/// into the rollup before it leaves the rest to later steps.
pub(crate) const BACKFILL_OPEN_BUDGET: Duration = Duration::from_secs(2);

const SECOND_NS: i64 = 1_000_000_000;
/// The span of spans one backfill step reads.
const BACKFILL_STEP_NS: i64 = 86_400 * SECOND_NS;
/// A step reads its day in slices of this length and looks at its budget
/// between them, so a day dense with spans cannot overrun a turn by more
/// than one slice.
const BACKFILL_SLICE_NS: i64 = 3600 * SECOND_NS;
/// Sightings are folded a minute of spans at a time, so one batch that
/// spans a rollback still shows the silence between its deployments.
const BUCKET_NS: i64 = 60 * SECOND_NS;

/// A `service.version` in one environment: when the service's telemetry
/// there first and last carried it, and when its latest deployment began.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionSighting {
    /// `None` for spans that name no environment.
    pub environment: Option<String>,
    pub version: String,
    pub first_seen_ns: i64,
    pub last_seen_ns: i64,
    /// The first sight, or the sight that ended a silence in which
    /// another version spoke.
    pub last_deployed_ns: i64,
    /// How many times it was deployed: 1 unless it came back.
    pub deployments: u32,
    /// The telemetry's version text broke the bounds every version is
    /// held to and was cut to them.
    pub sanitized: bool,
}

impl VersionSighting {
    /// Deployed again after another version.
    #[must_use]
    pub fn redeployed(&self) -> bool {
        self.deployments > 1
    }
}

/// The versioned spans of one service, environment and version inside
/// one minute.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Sight {
    service: String,
    environment: Option<String>,
    version: String,
    sanitized: bool,
    first_ns: i64,
    last_ns: i64,
}

/// (environment, version): the rollup's key under one service.
type Key = (Option<String>, String);

/// One rollup row, without its key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Held {
    first_ns: i64,
    last_ns: i64,
    deployed_ns: i64,
    deployments: u32,
    sanitized: bool,
}

/// The rollup rows of one service, merged per key.
const HELD: &str = "
SELECT
    environment,
    service_version,
    min(first_seen_ns) AS first_seen_ns,
    max(last_seen_ns) AS last_seen_ns,
    max(coalesce(last_deployed_ns, first_seen_ns)) AS last_deployed_ns,
    max(coalesce(deployments, 1)) AS deployments,
    coalesce(bool_or(sanitized), false) AS sanitized
FROM service_version_sightings
WHERE service_name = ?
GROUP BY environment, service_version
ORDER BY first_seen_ns, service_version, environment
";

/// A span's `service.version` as the lake stores it: within the bounds
/// every version is held to, and whether it had to be cut to them.
/// `None` for a span that names no version.
pub(crate) fn stored_version(row: &SpanRow) -> Option<(Cow<'_, str>, bool)> {
    let version = row.service_version.as_deref()?;
    Some(bounded_version_text(version))
}

/// Whether a sight at `at_ns` begins a new deployment of the version
/// `row` holds: it comes after a silence longer than [`REDEPLOY_GAP`],
/// and another version of the same environment was first seen before it
/// and still spoke more than the gap into that silence.
fn returned_after_another(held: &BTreeMap<Key, Held>, key: &Key, row: &Held, at_ns: i64) -> bool {
    let gap_ns = i64::try_from(REDEPLOY_GAP.as_nanos()).unwrap_or(i64::MAX);
    if at_ns <= row.last_ns.saturating_add(gap_ns) {
        return false;
    }
    held.iter().any(|((environment, version), other)| {
        environment == &key.0
            && version != &key.1
            && other.last_ns > row.last_ns.saturating_add(gap_ns)
            && other.first_ns < at_ns
    })
}

/// Whether `sight` lies further before the first sight `row` holds than
/// a late span may: more than [`LATE_SPAN_LIMIT`].
fn before_the_start(row: &Held, sight: &Sight) -> bool {
    let limit_ns = i64::try_from(LATE_SPAN_LIMIT.as_nanos()).unwrap_or(i64::MAX);
    sight.first_ns < row.first_ns.saturating_sub(limit_ns)
}

/// Fold one sight into the rows of its service.
fn fold(held: &mut BTreeMap<Key, Held>, sight: &Sight) {
    let key = (sight.environment.clone(), sight.version.clone());
    let Some(mut row) = held.get(&key).copied() else {
        held.insert(
            key,
            Held {
                first_ns: sight.first_ns,
                last_ns: sight.last_ns,
                deployed_ns: sight.first_ns,
                deployments: 1,
                sanitized: sight.sanitized,
            },
        );
        return;
    };
    if before_the_start(&row, sight) {
        tracing::warn!(
            service = %sight.service,
            first_seen_ns = row.first_ns,
            span_ts_ns = sight.first_ns,
            "lake.sightings.early_span_ignored"
        );
        return;
    }
    if returned_after_another(held, &key, &row, sight.first_ns) {
        row.deployed_ns = sight.first_ns;
        row.deployments = row.deployments.saturating_add(1);
    } else if sight.first_ns < row.first_ns && row.deployments == 1 {
        // Spans that arrived late move the one deployment's start back,
        // by no more than `LATE_SPAN_LIMIT`.
        row.deployed_ns = sight.first_ns;
    }
    row.first_ns = row.first_ns.min(sight.first_ns);
    row.last_ns = row.last_ns.max(sight.last_ns);
    row.sanitized |= sight.sanitized;
    held.insert(key, row);
}

/// A service, environment and version within one minute of a batch.
type Bucket<'a> = (&'a str, Option<&'a str>, Cow<'a, str>, i64);

/// The versioned spans of one batch as sights, oldest first. With `upto`
/// only the spans at or before it: the rest are the backfill's to read.
fn sights_in(rows: &[SpanRow], upto: Option<i64>) -> Vec<Sight> {
    let mut seen: BTreeMap<Bucket<'_>, (i64, i64, bool)> = BTreeMap::new();
    for row in rows {
        if upto.is_some_and(|upto| row.ts_ns > upto) {
            continue;
        }
        let Some((version, cut)) = stored_version(row).filter(|(v, _)| !v.is_empty()) else {
            continue;
        };
        let key = (
            row.service_name.as_str(),
            row.target_environment.as_deref(),
            version,
            row.ts_ns.div_euclid(BUCKET_NS),
        );
        let span = seen.entry(key).or_insert((row.ts_ns, row.ts_ns, cut));
        span.0 = span.0.min(row.ts_ns);
        span.1 = span.1.max(row.ts_ns);
        span.2 |= cut;
    }
    let mut sights: Vec<Sight> = seen
        .into_iter()
        .map(
            |((service, environment, version, _), (first_ns, last_ns, sanitized))| Sight {
                service: service.to_string(),
                environment: environment.map(str::to_string),
                version: version.into_owned(),
                sanitized,
                first_ns,
                last_ns,
            },
        )
        .collect();
    sights.sort_by_key(|sight| sight.first_ns);
    sights
}

fn held_of(conn: &Connection, service: &str) -> Result<BTreeMap<Key, Held>, duckdb::Error> {
    let mut stmt = conn.prepare(HELD)?;
    let rows = stmt.query_map(params![service], |row| {
        Ok((
            (row.get::<_, Option<String>>(0)?, row.get::<_, String>(1)?),
            Held {
                first_ns: row.get(2)?,
                last_ns: row.get(3)?,
                deployed_ns: row.get(4)?,
                deployments: row.get(5)?,
                sanitized: row.get(6)?,
            },
        ))
    })?;
    rows.collect()
}

/// Write the rows of `service` that differ from what the rollup held:
/// an update where the key is there, an insert where it is not.
fn store_changed(
    conn: &Connection,
    service: &str,
    before: &BTreeMap<Key, Held>,
    after: &BTreeMap<Key, Held>,
) -> Result<(), duckdb::Error> {
    let mut update = conn.prepare(
        "UPDATE service_version_sightings
         SET first_seen_ns = ?, last_seen_ns = ?, last_deployed_ns = ?,
             deployments = ?, sanitized = ?
         WHERE service_name = ? AND service_version = ?
           AND environment IS NOT DISTINCT FROM ?",
    )?;
    let mut insert = conn.prepare(
        "INSERT INTO service_version_sightings
            (service_name, environment, service_version, first_seen_ns, last_seen_ns,
             last_deployed_ns, deployments, sanitized)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )?;
    for (key, row) in after {
        let (environment, version) = key;
        match before.get(key) {
            Some(held) if held == row => {}
            Some(_) => {
                update.execute(params![
                    row.first_ns,
                    row.last_ns,
                    row.deployed_ns,
                    row.deployments,
                    row.sanitized,
                    service,
                    version,
                    environment
                ])?;
            }
            None => {
                insert.execute(params![
                    service,
                    environment,
                    version,
                    row.first_ns,
                    row.last_ns,
                    row.deployed_ns,
                    row.deployments,
                    row.sanitized
                ])?;
            }
        }
    }
    Ok(())
}

/// Fold `sights` (oldest first) into the rollup. Runs inside the caller's
/// transaction.
fn record(conn: &Connection, sights: &[Sight]) -> Result<(), duckdb::Error> {
    let mut by_service: BTreeMap<&str, Vec<&Sight>> = BTreeMap::new();
    for sight in sights {
        by_service.entry(&sight.service).or_default().push(sight);
    }
    for (service, sights) in by_service {
        let before = held_of(conn, service)?;
        let mut after = before.clone();
        for sight in sights {
            fold(&mut after, sight);
        }
        store_changed(conn, service, &before, &after)?;
    }
    Ok(())
}

/// Where the one-time reading of an old lake's spans stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Backfill {
    NotStarted,
    /// Spans up to and including this instant have been read.
    Running(i64),
    Done,
}

fn backfill_state(conn: &Connection) -> Result<Backfill, duckdb::Error> {
    let mut stmt = conn.prepare("SELECT done, cursor_ns FROM sightings_backfill LIMIT 1")?;
    let mut rows = stmt.query(params![])?;
    let Some(row) = rows.next()? else {
        return Ok(Backfill::NotStarted);
    };
    if row.get::<_, bool>(0)? {
        return Ok(Backfill::Done);
    }
    Ok(Backfill::Running(
        row.get::<_, Option<i64>>(1)?.unwrap_or(i64::MIN),
    ))
}

fn in_tx<T>(
    conn: &Connection,
    f: impl FnOnce() -> Result<T, duckdb::Error>,
) -> Result<T, duckdb::Error> {
    conn.execute_batch("BEGIN TRANSACTION")?;
    match f() {
        Ok(value) => {
            conn.execute_batch("COMMIT")?;
            Ok(value)
        }
        Err(e) => {
            if let Err(rollback) = conn.execute_batch("ROLLBACK") {
                tracing::warn!(
                    error.message = %rollback,
                    "lake.sightings.rollback_failed"
                );
            }
            Err(e)
        }
    }
}

/// Empty the rollup and set the backfill to read every span again.
/// Runs inside the caller's transaction.
fn backfill_restart(conn: &Connection) -> Result<(), duckdb::Error> {
    conn.execute_batch(
        "DELETE FROM service_version_sightings;
         DELETE FROM sightings_backfill;
         INSERT INTO sightings_backfill (done, cursor_ns) VALUES (false, NULL);",
    )
}

/// Read the next day of versioned spans into the rollup, a slice of
/// [`BACKFILL_SLICE_NS`] at a time, and move the cursor past what was
/// read; `true` when no span is left to read, which marks the backfill
/// done. Past `until` the step ends after the slice it is in, the rest of
/// the day left to the next step. Runs inside the caller's transaction.
fn backfill_step(conn: &Connection, until: Option<Instant>) -> Result<bool, duckdb::Error> {
    let Backfill::Running(cursor) = backfill_state(conn)? else {
        return Ok(true);
    };
    let next: Option<i64> = conn.query_row(
        "SELECT min(ts_ns) FROM spans
         WHERE ts_ns > ? AND service_version IS NOT NULL AND service_version <> ''",
        params![cursor],
        |row| row.get(0),
    )?;
    let Some(next) = next else {
        conn.execute_batch("UPDATE sightings_backfill SET done = true")?;
        return Ok(true);
    };
    let day_end = next.saturating_add(BACKFILL_STEP_NS);
    let mut from = cursor;
    let mut upto = next;
    while upto < day_end {
        upto = upto.saturating_add(BACKFILL_SLICE_NS).min(day_end);
        backfill_slice(conn, from, upto)?;
        from = upto;
        if until.is_some_and(|until| Instant::now() >= until) {
            break;
        }
    }
    Ok(false)
}

/// Read the versioned spans of `(from, upto]` into the rollup and set
/// the cursor to `upto`.
fn backfill_slice(conn: &Connection, from: i64, upto: i64) -> Result<(), duckdb::Error> {
    let mut stmt = conn.prepare(
        "SELECT service_name, target_environment, service_version,
                min(ts_ns) AS first_ns, max(ts_ns) AS last_ns
         FROM spans
         WHERE ts_ns > ? AND ts_ns <= ?
           AND service_version IS NOT NULL AND service_version <> ''
         GROUP BY service_name, target_environment, service_version, ts_ns // ?
         ORDER BY first_ns",
    )?;
    let rows = stmt.query_map(params![from, upto, BUCKET_NS], |row| {
        let version: String = row.get(2)?;
        let (version, sanitized) = bounded_version_text(&version);
        Ok(Sight {
            service: row.get(0)?,
            environment: row.get(1)?,
            version: version.into_owned(),
            sanitized,
            first_ns: row.get(3)?,
            last_ns: row.get(4)?,
        })
    })?;
    let sights = rows.collect::<Result<Vec<_>, _>>()?;
    record(conn, &sights)?;
    conn.execute("UPDATE sightings_backfill SET cursor_ns = ?", params![upto])?;
    Ok(())
}

/// Step the backfill until it is done or `budget` is spent, each step in
/// a transaction of its own; `true` when it is done.
fn backfill_for(conn: &Connection, budget: Duration) -> Result<bool, duckdb::Error> {
    let started = Instant::now();
    let until = started.checked_add(budget);
    loop {
        let done = in_tx(conn, || backfill_step(conn, until))?;
        if done || started.elapsed() >= budget {
            return Ok(done);
        }
    }
}

/// On opening a lake: begin the backfill where none was recorded (a lake
/// written before the rollup knew deployments; on a new lake it ends at
/// once, there being no spans) and give a running one
/// [`BACKFILL_OPEN_BUDGET`]. A finished backfill is never run again.
pub(crate) fn backfill(conn: &Connection) -> Result<(), duckdb::Error> {
    match backfill_state(conn)? {
        Backfill::Done => return Ok(()),
        Backfill::NotStarted => in_tx(conn, || backfill_restart(conn))?,
        Backfill::Running(_) => {}
    }
    let done = backfill_for(conn, BACKFILL_OPEN_BUDGET)?;
    if !done {
        tracing::info!(
            budget_ms = u64::try_from(BACKFILL_OPEN_BUDGET.as_millis()).unwrap_or(u64::MAX),
            "lake.sightings.backfill.running"
        );
    }
    Ok(())
}

/// Fold the versioned spans of one batch into the rollup. Runs inside the
/// caller's transaction. While the backfill runs, spans it has yet to
/// reach are left to it, and each batch moves it one step on.
pub(crate) fn note_sightings(conn: &Connection, rows: &[SpanRow]) -> Result<(), StoreError> {
    let cut = rows
        .iter()
        .filter(|row| stored_version(row).is_some_and(|(_, cut)| cut))
        .count();
    if cut > 0 {
        tracing::warn!(
            spans = cut,
            max_len = MAX_VERSION_LEN,
            "lake.span.version_sanitized"
        );
    }
    let state = backfill_state(conn)?;
    let upto = match state {
        Backfill::Running(cursor) => Some(cursor),
        Backfill::NotStarted | Backfill::Done => None,
    };
    record(conn, &sights_in(rows, upto))?;
    if upto.is_some() {
        backfill_step(conn, None)?;
    }
    Ok(())
}

impl Reader {
    /// Every version `service` has reported, per environment, oldest
    /// first sight first.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn version_sightings(&self, service: &str) -> Result<Vec<VersionSighting>, StoreError> {
        let held = held_of(&self.conn, service)?;
        let mut sightings: Vec<VersionSighting> = held
            .into_iter()
            .map(|((environment, version), row)| VersionSighting {
                environment,
                version,
                first_seen_ns: row.first_ns,
                last_seen_ns: row.last_ns,
                last_deployed_ns: row.deployed_ns,
                deployments: row.deployments,
                sanitized: row.sanitized,
            })
            .collect();
        sightings.sort_by(|a, b| {
            a.first_seen_ns
                .cmp(&b.first_seen_ns)
                .then_with(|| a.version.cmp(&b.version))
                .then_with(|| a.environment.cmp(&b.environment))
        });
        Ok(sightings)
    }

    /// Whether the rollup is still being read back from an old lake's
    /// spans, and so may miss versions and deployments.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn version_backfill_running(&self) -> Result<bool, StoreError> {
        Ok(matches!(backfill_state(&self.conn)?, Backfill::Running(_)))
    }

    /// The versions of `service` that served `[from_ns, to_ns]` in
    /// `environment` (`None`: the spans that name no environment), with
    /// the requests each served, most first. A request is a span the
    /// service received (kind server or consumer) or began (no parent);
    /// the spans a request fans out into inside the service are not
    /// counted, so a chatty build weighs no more than a quiet one. With
    /// `namespace`, only spans whose resource carries that
    /// `service.namespace`. Empty when the service reported nothing
    /// versioned there in that time.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn observed_versions(
        &self,
        service: &str,
        environment: Option<&str>,
        namespace: Option<&str>,
        from_ns: i64,
        to_ns: i64,
    ) -> Result<Vec<VersionShare>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT service_version, count(*) FROM spans
             WHERE service_name = ? AND ts_ns BETWEEN ? AND ?
               AND target_environment IS NOT DISTINCT FROM ?
               AND service_version IS NOT NULL AND service_version <> ''
               AND (upper(span_kind) IN ('SERVER', 'CONSUMER')
                    OR parent_span_id IS NULL OR parent_span_id = '')
               AND (? IS NULL OR resource_attrs['service.namespace'] = ?)
             GROUP BY service_version",
        )?;
        let rows = stmt.query_map(
            params![service, from_ns, to_ns, environment, namespace, namespace],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?)),
        )?;
        let mut requests: BTreeMap<String, u64> = BTreeMap::new();
        for row in rows {
            let (version, served) = row?;
            let (version, _) = bounded_version_text(&version);
            *requests.entry(version.into_owned()).or_default() += served;
        }
        let mut shares: Vec<VersionShare> = requests
            .into_iter()
            .map(|(version, requests)| VersionShare { version, requests })
            .collect();
        shares.sort_by(|a, b| {
            b.requests
                .cmp(&a.requests)
                .then_with(|| a.version.cmp(&b.version))
        });
        Ok(shares)
    }
}

impl Writer {
    /// Rebuild the rollup from the spans stored, to the end, for use
    /// after spans arrived by another road than [`Writer::insert_spans`]
    /// (a bulk import).
    ///
    /// # Errors
    /// Returns an error if the rebuild fails (the rollup is left as it was).
    pub fn rebuild_version_sightings(&self) -> Result<(), StoreError> {
        with_tx(&self.conn, || {
            backfill_restart(&self.conn)?;
            while !backfill_step(&self.conn, None)? {}
            Ok(())
        })
    }

    /// Give a backfill that is still running up to `budget` more; `true`
    /// when the rollup is complete.
    ///
    /// # Errors
    /// Returns an error if a step fails (the steps before it are kept).
    pub fn continue_version_backfill(&self, budget: Duration) -> Result<bool, StoreError> {
        Ok(backfill_for(&self.conn, budget)?)
    }
}

/// Answers a runner's question about the versions that served its run
/// from the target's spans in the store at a path, on a read-only
/// connection opened for the question.
#[derive(Debug, Clone)]
pub struct StoredVersions {
    path: PathBuf,
}

impl StoredVersions {
    /// The versions seen by the store at `path`.
    #[must_use]
    pub fn at(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
        }
    }
}

impl TargetVersionResolver for StoredVersions {
    fn resolve(&self, window: &RunWindow) -> Result<Vec<VersionShare>, String> {
        let reader = Store::at(&self.path)
            .read_only()
            .map_err(|e| format!("cannot read the store at {}: {e}", self.path.display()))?;
        reader
            .observed_versions(
                &window.system,
                window.environment.as_deref(),
                window.namespace.as_deref(),
                window.started_at_ns,
                window.ended_at_ns,
            )
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests;
