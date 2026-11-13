//! Which `service.version`s a service has reported, per environment, and
//! which of them served a given window.
//!
//! `service_version_sightings` is a rollup of `spans`: one row per
//! (service, environment, version) with the first and the last time the
//! service's own telemetry carried that version there. The writer keeps
//! it current inside the transaction that inserts the spans, so reading a
//! service's deployment history costs a scan of a table whose size
//! follows the number of versions, never the number of spans. A lake
//! written before the rollup existed is backfilled once, on open.
//!
//! An environment is matched exactly, and spans that name none are an
//! environment of their own (`None`): they never stand in for a named one.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use duckdb::{params, Connection};
use tumult_core::runner::{RunWindow, TargetVersionResolver};

use crate::error::StoreError;
use crate::rows::SpanRow;
use crate::store::with_tx;
use crate::{Reader, Store, Writer};

/// A `service.version` in one environment and when the service's
/// telemetry there first and last carried it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionSighting {
    /// `None` for spans that name no environment.
    pub environment: Option<String>,
    pub version: String,
    pub first_seen_ns: i64,
    pub last_seen_ns: i64,
}

/// The rollup as the spans stored give it.
const FROM_SPANS: &str = "
INSERT INTO service_version_sightings
SELECT
    service_name,
    target_environment,
    service_version,
    min(ts_ns) AS first_seen_ns,
    max(ts_ns) AS last_seen_ns
FROM spans
WHERE service_version IS NOT NULL AND service_version <> ''
GROUP BY service_name, target_environment, service_version;
";

/// Fill an empty rollup from the spans already stored: the one-time
/// upgrade of a lake written before the rollup existed. A rollup that
/// holds a row is left alone, so this is safe on every open.
pub(crate) fn backfill(conn: &Connection) -> Result<(), duckdb::Error> {
    let held: i64 = conn.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM service_version_sightings LIMIT 1) AS held",
        params![],
        |row| row.get(0),
    )?;
    if held > 0 {
        return Ok(());
    }
    conn.execute_batch(FROM_SPANS)
}

/// A run of digits as a number of any size: fewer digits (leading zeros
/// aside) is less, the same count compares digit by digit. Nothing is
/// parsed into a machine word, so no number is too long to order.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Number {
    digits: usize,
    text: String,
}

impl Number {
    /// `None` unless `text` is one or more ASCII digits.
    fn read(text: &str) -> Option<Self> {
        if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let significant = text.trim_start_matches('0');
        Some(Self {
            digits: significant.len(),
            text: significant.to_string(),
        })
    }

    fn is_zero(&self) -> bool {
        self.digits == 0
    }
}

/// One identifier of a pre-release tag: numbers sort below text.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Part {
    Number(Number),
    Text(String),
}

impl Part {
    /// `None` unless `text` is one or more of `[0-9A-Za-z-]`.
    fn read(text: &str) -> Option<Self> {
        let fits = |b: u8| b.is_ascii_alphanumeric() || b == b'-';
        if text.is_empty() || !text.bytes().all(fits) {
            return None;
        }
        Some(Number::read(text).map_or_else(|| Self::Text(text.to_string()), Self::Number))
    }
}

/// What follows the numbers: a pre-release sorts below its release, and a
/// tag that is the start of another below the longer one.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Stage {
    Pre(Vec<Part>),
    Release,
}

/// What a version string is ordered by. The variants are the two classes,
/// in their order: everything that reads as a version, then all other
/// text. Two strings are compared inside one class only, which is what
/// makes the order total.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Rank {
    /// Dotted numbers without their trailing zeros (`1.2` and `1.2.0` are
    /// the same number), then the stage.
    Version(Vec<Number>, Stage),
    Text,
}

/// The fewest dotted numbers a version has: one number alone is a date,
/// a build counter or a sha that happens to be all digits.
const MIN_NUMBERS: usize = 2;

impl Rank {
    fn of(text: &str) -> Self {
        Self::version(text).unwrap_or(Self::Text)
    }

    /// `None` unless `text` is an optional `v`, two or more dotted
    /// numbers, an optional `-` pre-release of dotted identifiers and an
    /// optional `+` build tag (which does not rank).
    fn version(text: &str) -> Option<Self> {
        let text = text.strip_prefix('v').unwrap_or(text);
        let text = text
            .split_once('+')
            .map_or(text, |(version, _build)| version);
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };
        let mut numbers = core
            .split('.')
            .map(Number::read)
            .collect::<Option<Vec<_>>>()?;
        if numbers.len() < MIN_NUMBERS {
            return None;
        }
        while numbers.last().is_some_and(Number::is_zero) {
            numbers.pop();
        }
        let stage = match pre {
            None => Stage::Release,
            Some(tag) => Stage::Pre(tag.split('.').map(Part::read).collect::<Option<_>>()?),
        };
        Some(Self::Version(numbers, stage))
    }
}

/// Order two version strings. A string that reads as a version — an
/// optional leading `v`, two or more dotted numbers, an optional
/// pre-release and build tag — sorts before one that does not (a sha, a
/// date, a branch name). Versions compare by their numbers (`1.10.0`
/// above `1.9.0`, however long the numbers), then a pre-release below its
/// release with its identifiers compared as semver compares them; build
/// metadata does not rank. Everything the rank leaves equal, and all
/// other text, is ordered by the string itself, so the order is total:
/// only the same string compares equal.
#[must_use]
pub fn version_cmp(a: &str, b: &str) -> Ordering {
    Rank::of(a).cmp(&Rank::of(b)).then_with(|| a.cmp(b))
}

/// The version that served a window, from `(version, spans)` counts: the
/// one with most spans, then the one deployed latest (`deployed_at_ns`),
/// then the greater version. `None` when nothing was counted.
#[must_use]
pub fn most_served(counts: &[(String, u64)], deployed_at_ns: impl Fn(&str) -> i64) -> Option<&str> {
    counts
        .iter()
        .max_by(|(a, served_a), (b, served_b)| {
            served_a
                .cmp(served_b)
                .then_with(|| deployed_at_ns(a).cmp(&deployed_at_ns(b)))
                .then_with(|| version_cmp(a, b))
        })
        .map(|(version, _)| version.as_str())
}

/// (service, environment, version) → (first, last) over one batch.
type Seen<'a> = BTreeMap<(&'a str, Option<&'a str>, &'a str), (i64, i64)>;

fn seen_in(rows: &[SpanRow]) -> Seen<'_> {
    let mut seen = Seen::new();
    for row in rows {
        let Some(version) = row.service_version.as_deref().filter(|v| !v.is_empty()) else {
            continue;
        };
        let key = (
            row.service_name.as_str(),
            row.target_environment.as_deref(),
            version,
        );
        let span = seen.entry(key).or_insert((row.ts_ns, row.ts_ns));
        span.0 = span.0.min(row.ts_ns);
        span.1 = span.1.max(row.ts_ns);
    }
    seen
}

/// Widen the rollup with the versioned spans of one batch. Runs inside
/// the caller's transaction.
pub(crate) fn note_sightings(conn: &Connection, rows: &[SpanRow]) -> Result<(), StoreError> {
    let seen = seen_in(rows);
    if seen.is_empty() {
        return Ok(());
    }
    let mut widen = conn.prepare(
        "UPDATE service_version_sightings
         SET first_seen_ns = least(first_seen_ns, ?), last_seen_ns = greatest(last_seen_ns, ?)
         WHERE service_name = ? AND service_version = ?
           AND environment IS NOT DISTINCT FROM ?",
    )?;
    let mut add = conn.prepare("INSERT INTO service_version_sightings VALUES (?, ?, ?, ?, ?)")?;
    for ((service, environment, version), (first, last)) in seen {
        let widened = widen.execute(params![first, last, service, version, environment])?;
        if widened == 0 {
            add.execute(params![service, environment, version, first, last])?;
        }
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
        let mut stmt = self.conn.prepare(
            "SELECT environment, service_version,
                    min(first_seen_ns) AS first_seen_ns, max(last_seen_ns) AS last_seen_ns
             FROM service_version_sightings WHERE service_name = ?
             GROUP BY environment, service_version
             ORDER BY first_seen_ns, service_version, environment",
        )?;
        let rows = stmt.query_map(params![service], |row| {
            Ok(VersionSighting {
                environment: row.get(0)?,
                version: row.get(1)?,
                first_seen_ns: row.get(2)?,
                last_seen_ns: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    /// The version of `service` that served `[from_ns, to_ns]` in
    /// `environment` (`None`: the spans that name no environment): the
    /// one carried by most of the service's spans inside the window, by
    /// the rule of [`most_served`]. `None` when the service reported
    /// nothing versioned there in that time.
    ///
    /// # Errors
    /// Returns an error if a query fails.
    pub fn observed_version(
        &self,
        service: &str,
        environment: Option<&str>,
        from_ns: i64,
        to_ns: i64,
    ) -> Result<Option<String>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT service_version, count(*) FROM spans
             WHERE service_name = ? AND ts_ns BETWEEN ? AND ?
               AND target_environment IS NOT DISTINCT FROM ?
               AND service_version IS NOT NULL AND service_version <> ''
             GROUP BY service_version",
        )?;
        let rows = stmt.query_map(params![service, from_ns, to_ns, environment], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?))
        })?;
        let counts = rows.collect::<Result<Vec<_>, _>>()?;
        if counts.is_empty() {
            return Ok(None);
        }
        let deployed: BTreeMap<String, i64> = self
            .version_sightings(service)?
            .into_iter()
            .filter(|sighting| sighting.environment.as_deref() == environment)
            .map(|sighting| (sighting.version, sighting.first_seen_ns))
            .collect();
        let deployed_at = |version: &str| deployed.get(version).copied().unwrap_or(i64::MIN);
        Ok(most_served(&counts, deployed_at).map(str::to_string))
    }
}

impl Writer {
    /// Rebuild the rollup from the spans stored, for use after spans
    /// arrived by another road than [`Writer::insert_spans`] (a bulk
    /// import).
    ///
    /// # Errors
    /// Returns an error if the rebuild fails (the rollup is left as it was).
    pub fn rebuild_version_sightings(&self) -> Result<(), StoreError> {
        with_tx(&self.conn, || {
            self.conn
                .execute_batch("DELETE FROM service_version_sightings")?;
            self.conn.execute_batch(FROM_SPANS)?;
            Ok(())
        })
    }
}

/// Answers a runner's question about the version that served its run from
/// the target's spans in the store at a path, on a read-only connection
/// opened for the question.
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
    fn resolve(&self, window: &RunWindow) -> Result<Option<String>, String> {
        let reader = Store::at(&self.path)
            .read_only()
            .map_err(|e| format!("cannot read the store at {}: {e}", self.path.display()))?;
        reader
            .observed_version(
                &window.system,
                window.environment.as_deref(),
                window.started_at_ns,
                window.ended_at_ns,
            )
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: i64 = 1_000_000_000;

    fn span(service: &str, version: Option<&str>, env: Option<&str>, ts_ns: i64) -> SpanRow {
        SpanRow {
            ts_ns,
            trace_id: format!("t-{service}-{ts_ns}"),
            span_id: format!("s-{ts_ns}"),
            span_name: "GET /".into(),
            span_kind: "SERVER".into(),
            status_code: "OK".into(),
            service_name: service.into(),
            service_version: version.map(str::to_string),
            target_environment: env.map(str::to_string),
            events: "[]".into(),
            ..SpanRow::default()
        }
    }

    fn lake() -> (tempfile::TempDir, Store) {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Store::open(&dir.path().join("lake.duckdb")).unwrap();
        (dir, store)
    }

    type Seen = (Option<String>, String, i64, i64);

    fn seen(store: &Store, service: &str) -> Vec<Seen> {
        store
            .read_only()
            .unwrap()
            .version_sightings(service)
            .unwrap()
            .into_iter()
            .map(|s| (s.environment, s.version, s.first_seen_ns, s.last_seen_ns))
            .collect()
    }

    fn demo(version: &str, first: i64, last: i64) -> Seen {
        (Some("demo".into()), version.into(), first * S, last * S)
    }

    #[test]
    fn versions_compare_by_their_numbers_before_their_text() {
        use Ordering::{Equal, Greater, Less};
        assert_eq!(version_cmp("1.10.0", "1.9.0"), Greater);
        assert_eq!(version_cmp("1.9.0", "1.10.0"), Less);
        assert_eq!(version_cmp("v2.0.0", "1.99.99"), Greater);
        assert_eq!(version_cmp("2.0", "1.9.9"), Greater);
        assert_eq!(version_cmp("1.2.3", "1.2.3"), Equal);
        // A pre-release sorts below its release, and its parts by number.
        assert_eq!(version_cmp("1.0.0-rc.1", "1.0.0"), Less);
        assert_eq!(version_cmp("1.0.0-rc.10", "1.0.0-rc.2"), Greater);
        assert_eq!(version_cmp("1.0.0-alpha", "1.0.0-alpha.1"), Less);
        assert_eq!(version_cmp("1.0.0-1", "1.0.0-alpha"), Less);
        // Build metadata does not order; the text breaks the tie so the
        // order stays total.
        assert_eq!(version_cmp("1.0.0+b2", "1.0.1+b1"), Less);
        assert_eq!(version_cmp("1.0.0+b2", "1.0.0+b1"), Greater);
        assert_eq!(version_cmp("1.2", "1.2.0"), Less);
        // Anything that does not read as a version is text, and all text
        // sorts above every version: a sha, a date, a bare number, nothing.
        assert_eq!(version_cmp("main-abd", "main-abc"), Greater);
        assert_eq!(version_cmp("2024-06-01", "9.0.0"), Greater);
        assert_eq!(version_cmp("0a1b2c3", "9.0.0"), Greater);
        assert_eq!(version_cmp("1234567", "9.0.0"), Greater);
        assert_eq!(version_cmp("", "1.0.0"), Greater);
        assert_eq!(version_cmp("1.0.0", "main"), Less);
    }

    #[test]
    fn numbers_too_long_for_a_machine_word_still_compare_as_numbers() {
        use Ordering::{Greater, Less};
        let huge = "1.99999999999999999999999999999999999999.0";
        let huger = "1.100000000000000000000000000000000000000.0";
        assert_eq!(version_cmp(huge, huger), Less);
        assert_eq!(version_cmp(huge, "1.2.0"), Greater);
        assert_eq!(
            version_cmp("1.0.0-99999999999999999999999", "1.0.0-2"),
            Greater
        );
        // Leading zeros do not make a number greater.
        assert_eq!(version_cmp("1.007.0", "1.8.0"), Less);
    }

    /// Versions of every kind the order has to place: releases, a `v`
    /// prefix, pre-releases, build metadata, equal numbers spelled
    /// differently, numbers past `u64`, shas, dates, branch names, nothing.
    const CORPUS: [&str; 30] = [
        "1.0.0",
        "v1.0.0",
        "1.0",
        "1.0.0.0",
        "1.0.0-alpha",
        "1.0.0-alpha.1",
        "1.0.0-alpha.beta",
        "1.0.0-beta.2",
        "1.0.0-beta.11",
        "1.0.0-rc.1",
        "1.0.0-1",
        "1.0.0-",
        "1.0.0+build.5",
        "1.0.0+build.10",
        "1.0.0-rc.1+sha.5114f85",
        "1.10.0",
        "1.9.0",
        "01.9.0",
        "2.0.0",
        "18446744073709551616.0.0",
        "18446744073709551615.0.0",
        "99999999999999999999999999.1",
        "2024-06-01",
        "2024.06.01",
        "20240601",
        "0a1b2c3",
        "5114f85",
        "main",
        "",
        "v",
    ];

    #[test]
    fn the_version_order_is_total_over_a_mixed_corpus() {
        use Ordering::Equal;
        for a in CORPUS {
            assert_eq!(version_cmp(a, a), Equal, "{a:?} equals itself");
            for b in CORPUS {
                let ab = version_cmp(a, b);
                assert_eq!(ab, version_cmp(b, a).reverse(), "antisymmetry: {a:?} {b:?}");
                assert_eq!(ab == Equal, a == b, "only equal text is equal: {a:?} {b:?}");
                for c in CORPUS {
                    if ab.is_le() && version_cmp(b, c).is_le() {
                        assert!(version_cmp(a, c).is_le(), "transitivity: {a:?} {b:?} {c:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn sorting_any_permutation_of_the_corpus_gives_one_order() {
        let mut expected = CORPUS.to_vec();
        expected.sort_by(|a, b| version_cmp(a, b));
        // Every version sorts before all text.
        let reads = |v: &str| version_cmp(v, "0.0").is_ge() && version_cmp(v, "").is_lt();
        let first_text = expected.iter().position(|v| !reads(v)).unwrap();
        assert!(
            expected[first_text..].iter().all(|v| !reads(v)),
            "{expected:?}"
        );
        assert_eq!(expected[first_text - 1], "99999999999999999999999999.1");
        assert_eq!(first_text, 22, "{expected:?}");
        // Rotations, the reverse, and a few hundred seeded shuffles.
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move |bound: usize| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            usize::try_from(state >> 33).unwrap_or(0) % bound
        };
        let mut permutations: Vec<Vec<&str>> = (0..CORPUS.len())
            .map(|by| {
                let mut rotated = CORPUS.to_vec();
                rotated.rotate_left(by);
                rotated
            })
            .collect();
        permutations.push(CORPUS.iter().rev().copied().collect());
        for _ in 0..300 {
            let mut shuffled = CORPUS.to_vec();
            for i in (1..shuffled.len()).rev() {
                shuffled.swap(i, next(i + 1));
            }
            permutations.push(shuffled);
        }
        for mut permutation in permutations {
            permutation.sort_by(|a, b| version_cmp(a, b));
            assert_eq!(permutation, expected);
        }
    }

    #[test]
    fn inserting_spans_keeps_first_and_last_sight_per_environment() {
        let (_dir, store) = lake();
        let writer = store.writer().unwrap();
        writer
            .insert_spans(&[
                span("checkout", Some("1.0.0"), Some("demo"), 20 * S),
                span("checkout", Some("1.0.0"), Some("demo"), 10 * S),
                span("checkout", Some("1.0.0"), Some("prod"), 15 * S),
                span("checkout", Some("1.1.0"), None, 30 * S),
                span("checkout", None, Some("demo"), 99 * S),
                span("ledger", Some("3.0.0"), Some("demo"), 5 * S),
            ])
            .unwrap();
        // A later batch widens what is there, in both directions.
        writer
            .insert_spans(&[
                span("checkout", Some("1.0.0"), Some("demo"), 40 * S),
                span("checkout", Some("1.0.0"), Some("demo"), 4 * S),
                span("checkout", Some("1.1.0"), None, 31 * S),
            ])
            .unwrap();
        drop(writer);
        assert_eq!(
            seen(&store, "checkout"),
            vec![
                demo("1.0.0", 4, 40),
                (Some("prod".into()), "1.0.0".into(), 15 * S, 15 * S),
                (None, "1.1.0".into(), 30 * S, 31 * S),
            ]
        );
        assert_eq!(seen(&store, "ledger").len(), 1);
        assert!(seen(&store, "nobody").is_empty());
    }

    #[test]
    fn a_lake_written_before_the_rollup_is_backfilled_once_on_open() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = dir.path().join("lake.duckdb");
        {
            let store = Store::open(&db).unwrap();
            let writer = store.writer().unwrap();
            writer
                .insert_spans(&[
                    span("checkout", Some("1.0.0"), Some("demo"), 10 * S),
                    span("checkout", Some("1.0.0"), Some("demo"), 50 * S),
                    span("checkout", Some("1.1.0"), Some("demo"), 60 * S),
                ])
                .unwrap();
        }
        // As an older build left it: spans and no rollup.
        duckdb::Connection::open(&db)
            .unwrap()
            .execute_batch("DROP TABLE IF EXISTS service_version_sightings")
            .unwrap();
        let expected = vec![demo("1.0.0", 10, 50), demo("1.1.0", 60, 60)];
        let store = Store::open(&db).unwrap();
        assert_eq!(seen(&store, "checkout"), expected);
        // Opening again adds nothing.
        drop(store);
        let store = Store::open(&db).unwrap();
        assert_eq!(seen(&store, "checkout"), expected);
    }

    #[test]
    fn a_rebuild_reads_the_rollup_back_from_the_spans() {
        let (_dir, store) = lake();
        let writer = store.writer().unwrap();
        writer
            .insert_spans(&[
                span("checkout", Some("1.0.0"), Some("demo"), 10 * S),
                span("checkout", Some("1.1.0"), Some("demo"), 60 * S),
            ])
            .unwrap();
        // Spans that arrived by a bulk import, past `insert_spans`.
        writer
            .conn
            .execute_batch(
                "INSERT INTO spans (ts_ns, trace_id, span_id, span_name, span_kind, \
                 duration_ns, status_code, status_message, service_name, service_version, \
                 target_environment) VALUES \
                 (70000000000, 't', 's', 'GET /', 'SERVER', 0, 'OK', '', 'checkout', \
                 '1.2.0', 'demo')",
            )
            .unwrap();
        assert_eq!(seen(&store, "checkout").len(), 2);
        writer.rebuild_version_sightings().unwrap();
        assert_eq!(
            seen(&store, "checkout"),
            vec![
                demo("1.0.0", 10, 10),
                demo("1.1.0", 60, 60),
                demo("1.2.0", 70, 70)
            ]
        );
    }

    #[test]
    fn the_observed_version_is_the_one_with_most_spans_in_the_window_and_environment() {
        let (_dir, store) = lake();
        let writer = store.writer().unwrap();
        let mut rows = vec![
            // Before the window: 0.9.0 was live.
            span("checkout", Some("0.9.0"), Some("demo"), 50 * S),
            // A busy other environment inside the window.
            span("checkout", Some("2.0.0"), Some("prod"), 110 * S),
            span("checkout", Some("2.0.0"), Some("prod"), 111 * S),
            span("checkout", Some("2.0.0"), Some("prod"), 112 * S),
            span("checkout", Some("2.0.0"), Some("prod"), 113 * S),
            // Untagged spans are an environment of their own.
            span("checkout", Some("3.0.0"), None, 120 * S),
        ];
        rows.extend((0..3).map(|i| span("checkout", Some("1.0.0"), Some("demo"), (100 + i) * S)));
        rows.extend((0..2).map(|i| span("checkout", Some("1.1.0"), Some("demo"), (150 + i) * S)));
        writer.insert_spans(&rows).unwrap();
        drop(writer);
        let reader = store.read_only().unwrap();
        let observed = |env: Option<&str>, from: i64, to: i64| {
            reader
                .observed_version("checkout", env, from * S, to * S)
                .unwrap()
        };
        assert_eq!(observed(Some("demo"), 100, 200).as_deref(), Some("1.0.0"));
        assert_eq!(observed(Some("prod"), 100, 200).as_deref(), Some("2.0.0"));
        assert_eq!(observed(None, 100, 200).as_deref(), Some("3.0.0"));
        // The bounds are inclusive; nothing inside is nothing observed,
        // whatever spoke before.
        assert_eq!(observed(Some("demo"), 151, 151).as_deref(), Some("1.1.0"));
        assert_eq!(observed(Some("demo"), 60, 90), None);
        assert_eq!(observed(Some("staging"), 0, 500), None);
    }

    #[test]
    fn a_tie_inside_the_window_goes_to_the_latest_deploy_then_the_greater_version() {
        let (_dir, store) = lake();
        let writer = store.writer().unwrap();
        writer
            .insert_spans(&[
                // 1.9.0 was deployed after 1.10.0 here.
                span("checkout", Some("1.10.0"), Some("demo"), 10 * S),
                span("checkout", Some("1.9.0"), Some("demo"), 20 * S),
                span("checkout", Some("1.10.0"), Some("demo"), 100 * S),
                span("checkout", Some("1.9.0"), Some("demo"), 101 * S),
                // Both first seen at the same instant.
                span("ledger", Some("1.9.0"), Some("demo"), 100 * S),
                span("ledger", Some("1.10.0"), Some("demo"), 100 * S),
            ])
            .unwrap();
        drop(writer);
        let reader = store.read_only().unwrap();
        let observed = |service: &str| {
            reader
                .observed_version(service, Some("demo"), 100 * S, 200 * S)
                .unwrap()
        };
        assert_eq!(observed("checkout").as_deref(), Some("1.9.0"));
        assert_eq!(observed("ledger").as_deref(), Some("1.10.0"));
    }

    #[test]
    fn most_served_ranks_by_spans_then_deploy_then_version() {
        let counts = vec![("1.0.0".to_string(), 90), ("1.1.0".to_string(), 10)];
        assert_eq!(most_served(&counts, |_| 0), Some("1.0.0"));
        let even = vec![("1.9.0".to_string(), 5), ("1.10.0".to_string(), 5)];
        let deployed = |v: &str| if v == "1.9.0" { 9 } else { 3 };
        assert_eq!(most_served(&even, deployed), Some("1.9.0"));
        assert_eq!(most_served(&even, |_| 0), Some("1.10.0"));
        assert_eq!(most_served(&[], |_| 0), None);
    }

    #[test]
    fn a_runner_is_told_the_version_that_served_its_window_in_its_environment() {
        let (dir, store) = lake();
        let writer = store.writer().unwrap();
        let mut rows = vec![
            span("checkout", Some("1.0.0"), Some("demo"), 50 * S),
            span("checkout", Some("3.0.0"), None, 120 * S),
        ];
        rows.extend((0..3).map(|i| span("checkout", Some("1.1.0"), Some("demo"), (100 + i) * S)));
        writer.insert_spans(&rows).unwrap();
        drop(writer);
        let versions = StoredVersions::at(&dir.path().join("lake.duckdb"));
        let window = |environment: Option<&str>, from: i64, to: i64| RunWindow {
            system: "checkout".into(),
            environment: environment.map(str::to_string),
            started_at_ns: from * S,
            ended_at_ns: to * S,
        };
        let demo = versions.resolve(&window(Some("demo"), 90, 130));
        assert_eq!(demo, Ok(Some("1.1.0".to_string())));
        let untagged = versions.resolve(&window(None, 90, 130));
        assert_eq!(untagged, Ok(Some("3.0.0".to_string())));
        assert_eq!(versions.resolve(&window(Some("prod"), 90, 130)), Ok(None));
        assert_eq!(versions.resolve(&window(Some("demo"), 60, 70)), Ok(None));
    }

    #[test]
    fn a_store_that_cannot_be_read_is_an_error_not_an_absent_version() {
        let dir = tempfile::TempDir::new().unwrap();
        let versions = StoredVersions::at(&dir.path().join("missing.duckdb"));
        let window = RunWindow {
            system: "checkout".into(),
            environment: None,
            started_at_ns: 0,
            ended_at_ns: S,
        };
        let error = versions.resolve(&window).unwrap_err();
        assert!(error.contains("missing.duckdb"), "{error}");
    }
}
