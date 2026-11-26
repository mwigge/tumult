//! Performance-regression verdict storage (schema v18): the `perf_verdicts`
//! table, one row per verdict computed between two `service.version`s of a
//! target. Rows are append-only — a recomputation adds a newer row — so
//! `GET /api/perf/{target}` and the CLI read the latest row per
//! (target, probe, baseline, candidate) and the history stays auditable.

use duckdb::params;
use serde_json::Value;

use crate::error::StoreError;
use crate::sql;
use crate::{Reader, Writer};

/// One `perf_verdicts` row: what was compared and what came out. The
/// statistical fields mirror `tumult_perf::Comparison`; `None` for a
/// percentile or interval means that side had no samples.
#[derive(Debug, Clone, PartialEq)]
pub struct PerfVerdictRow {
    pub target: String,
    /// The `probe_samples.probe_name` series compared.
    pub probe: String,
    pub baseline_version: String,
    pub candidate_version: String,
    /// The quantile the verdict is keyed on (`p50`, `p95`, `p99`).
    pub quantile: String,
    pub baseline_n: i64,
    pub candidate_n: i64,
    pub baseline_p50: Option<f64>,
    pub baseline_p95: Option<f64>,
    pub baseline_p99: Option<f64>,
    pub candidate_p50: Option<f64>,
    pub candidate_p95: Option<f64>,
    pub candidate_p99: Option<f64>,
    pub delta: Option<f64>,
    pub delta_pct: Option<f64>,
    pub ci_low: Option<f64>,
    pub ci_high: Option<f64>,
    pub confidence: f64,
    pub margin: f64,
    pub verdict: String,
    /// JSON array of strings.
    pub reasons: Vec<String>,
    pub computed_at_ns: i64,
    /// The environments the evidence was restricted to; `None` when the
    /// comparison read every environment.
    pub envs: Option<Vec<String>>,
}

impl Writer {
    /// Append one verdict row.
    ///
    /// # Errors
    /// Returns an error if the insert fails or `reasons` cannot be encoded.
    pub fn insert_perf_verdict(&self, row: &PerfVerdictRow) -> Result<(), StoreError> {
        let reasons = serde_json::to_string(&row.reasons)
            .map_err(|e| StoreError::Internal(format!("perf verdict reasons: {e}")))?;
        let envs = row
            .envs
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| StoreError::Internal(format!("perf verdict envs: {e}")))?;
        self.conn.execute(
            sql::perf::INSERT_VERDICT,
            params![
                row.target,
                row.probe,
                row.baseline_version,
                row.candidate_version,
                row.quantile,
                row.baseline_n,
                row.candidate_n,
                row.baseline_p50,
                row.baseline_p95,
                row.baseline_p99,
                row.candidate_p50,
                row.candidate_p95,
                row.candidate_p99,
                row.delta,
                row.delta_pct,
                row.ci_low,
                row.ci_high,
                row.confidence,
                row.margin,
                row.verdict,
                reasons,
                row.computed_at_ns,
                envs,
            ],
        )?;
        Ok(())
    }
}

/// Whether the latest stored row for the comparison `row` names — same
/// target, probe, versions, quantile and environments — holds the same
/// evidence: sample counts, percentiles, difference, interval, level,
/// margin and verdict. NULLs compare equal to NULLs.
const SAME_AS_LATEST: &str = "
SELECT count(*) FROM (
    SELECT * FROM perf_verdicts
    WHERE target = ? AND probe = ? AND baseline_version = ?
      AND candidate_version = ? AND quantile = ?
      AND CAST(envs AS VARCHAR) IS NOT DISTINCT FROM ?
    ORDER BY computed_at_ns DESC, rowid DESC LIMIT 1
) AS latest
WHERE latest.baseline_n = ? AND latest.candidate_n = ?
  AND latest.baseline_p50 IS NOT DISTINCT FROM ?
  AND latest.baseline_p95 IS NOT DISTINCT FROM ?
  AND latest.baseline_p99 IS NOT DISTINCT FROM ?
  AND latest.candidate_p50 IS NOT DISTINCT FROM ?
  AND latest.candidate_p95 IS NOT DISTINCT FROM ?
  AND latest.candidate_p99 IS NOT DISTINCT FROM ?
  AND latest.delta IS NOT DISTINCT FROM ?
  AND latest.delta_pct IS NOT DISTINCT FROM ?
  AND latest.ci_low IS NOT DISTINCT FROM ?
  AND latest.ci_high IS NOT DISTINCT FROM ?
  AND latest.confidence = ? AND latest.margin = ? AND latest.verdict = ?";

impl Writer {
    /// Append `row` unless the latest stored row for the same comparison
    /// already holds the same evidence, so that a repeated request — a
    /// retry after a client timeout — leaves one row. `Ok(true)` when a
    /// row was appended. A verdict that changed and later changed back is
    /// appended again: only the latest row is compared.
    ///
    /// # Errors
    /// Returns an error if the lookup or the insert fails.
    pub fn append_perf_verdict(&self, row: &PerfVerdictRow) -> Result<bool, StoreError> {
        let row = PerfVerdictRow {
            envs: row.envs.as_deref().map(as_set),
            computed_at_ns: crate::lake::now_ns(),
            ..row.clone()
        };
        if self.latest_perf_verdict_matches(&row)? {
            return Ok(false);
        }
        self.insert_perf_verdict(&row)?;
        Ok(true)
    }

    fn latest_perf_verdict_matches(&self, row: &PerfVerdictRow) -> Result<bool, StoreError> {
        let envs = row
            .envs
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| StoreError::Internal(format!("perf verdict envs: {e}")))?;
        let same: i64 = self.conn.query_row(
            SAME_AS_LATEST,
            params![
                row.target,
                row.probe,
                row.baseline_version,
                row.candidate_version,
                row.quantile,
                envs,
                row.baseline_n,
                row.candidate_n,
                row.baseline_p50,
                row.baseline_p95,
                row.baseline_p99,
                row.candidate_p50,
                row.candidate_p95,
                row.candidate_p99,
                row.delta,
                row.delta_pct,
                row.ci_low,
                row.ci_high,
                row.confidence,
                row.margin,
                row.verdict,
            ],
            |r| r.get(0),
        )?;
        Ok(same > 0)
    }
}

/// An environment list as the set it stands for: sorted, each name once.
fn as_set(envs: &[String]) -> Vec<String> {
    let mut envs = envs.to_vec();
    envs.sort();
    envs.dedup();
    envs
}

impl Reader {
    /// The stored verdicts for `target`, newest first, at most `limit`
    /// rows, as JSON objects with the column names of `perf_verdicts`
    /// (`reasons` and `envs` decoded to arrays).
    ///
    /// `scopes` restricts the rows to verdicts whose evidence came only
    /// from those environments; a verdict computed across every
    /// environment (`envs` NULL) is hidden from a scoped reader. An empty
    /// `scopes` reads every row.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn perf_verdicts(
        &self,
        target: &str,
        scopes: &[String],
        limit: usize,
    ) -> Result<Vec<Value>, StoreError> {
        let scopes_json = serde_json::to_string(scopes)
            .map_err(|e| StoreError::Internal(format!("perf verdict scopes: {e}")))?;
        let unscoped = scopes.is_empty();
        let mut stmt = self.conn.prepare(sql::perf::VERDICTS)?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = stmt.query_map(params![target, unscoped, scopes_json, limit], |r| {
            let reasons: String = r.get(20)?;
            let envs: Option<String> = r.get(22)?;
            Ok(serde_json::json!({
                "target": r.get::<_, String>(0)?,
                "probe": r.get::<_, String>(1)?,
                "baseline_version": r.get::<_, String>(2)?,
                "candidate_version": r.get::<_, String>(3)?,
                "quantile": r.get::<_, String>(4)?,
                "baseline_n": r.get::<_, i64>(5)?,
                "candidate_n": r.get::<_, i64>(6)?,
                "baseline_p50": r.get::<_, Option<f64>>(7)?,
                "baseline_p95": r.get::<_, Option<f64>>(8)?,
                "baseline_p99": r.get::<_, Option<f64>>(9)?,
                "candidate_p50": r.get::<_, Option<f64>>(10)?,
                "candidate_p95": r.get::<_, Option<f64>>(11)?,
                "candidate_p99": r.get::<_, Option<f64>>(12)?,
                "delta": r.get::<_, Option<f64>>(13)?,
                "delta_pct": r.get::<_, Option<f64>>(14)?,
                "ci_low": r.get::<_, Option<f64>>(15)?,
                "ci_high": r.get::<_, Option<f64>>(16)?,
                "confidence": r.get::<_, f64>(17)?,
                "margin": r.get::<_, f64>(18)?,
                "verdict": r.get::<_, String>(19)?,
                "reasons": serde_json::from_str::<Value>(&reasons).unwrap_or(Value::Array(vec![])),
                "computed_at_ns": r.get::<_, i64>(21)?,
                "envs": envs
                    .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                    .unwrap_or(Value::Null),
            }))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
}

/// The statements of the performance verdicts, for the SQL corpus.
#[cfg(test)]
pub(crate) fn sql_corpus() -> Vec<tumult_test_utils::sql_corpus::Statement> {
    tumult_test_utils::sql_statements![SAME_AS_LATEST]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;

    fn row(verdict: &str, computed_at_ns: i64) -> PerfVerdictRow {
        PerfVerdictRow {
            target: "checkout".into(),
            probe: "load.latency_p95_ms".into(),
            baseline_version: "1.4.0".into(),
            candidate_version: "1.5.0".into(),
            quantile: "p95".into(),
            baseline_n: 120,
            candidate_n: 118,
            baseline_p50: Some(40.0),
            baseline_p95: Some(180.0),
            baseline_p99: Some(260.0),
            candidate_p50: Some(42.0),
            candidate_p95: Some(230.0),
            candidate_p99: Some(300.0),
            delta: Some(50.0),
            delta_pct: Some(27.8),
            ci_low: Some(31.0),
            ci_high: Some(70.0),
            confidence: 0.95,
            margin: 9.0,
            verdict: verdict.into(),
            reasons: vec!["p95 difference interval lies above the margin".into()],
            computed_at_ns,
            envs: None,
        }
    }

    /// `perf_verdicts` as the first release created it: the lookup index
    /// and no `envs` column.
    const FIRST_SHAPE: &str = "
CREATE TABLE perf_verdicts (
    target VARCHAR NOT NULL, probe VARCHAR NOT NULL,
    baseline_version VARCHAR NOT NULL, candidate_version VARCHAR NOT NULL,
    quantile VARCHAR NOT NULL,
    baseline_n BIGINT NOT NULL, candidate_n BIGINT NOT NULL,
    baseline_p50 DOUBLE, baseline_p95 DOUBLE, baseline_p99 DOUBLE,
    candidate_p50 DOUBLE, candidate_p95 DOUBLE, candidate_p99 DOUBLE,
    delta DOUBLE, delta_pct DOUBLE, ci_low DOUBLE, ci_high DOUBLE,
    confidence DOUBLE NOT NULL, margin DOUBLE NOT NULL,
    verdict VARCHAR NOT NULL, reasons JSON NOT NULL,
    computed_at_ns BIGINT NOT NULL
);
CREATE INDEX idx_perf_verdicts_target ON perf_verdicts (target);
INSERT INTO perf_verdicts VALUES (
    'checkout', 'load.latency_p95_ms', '1.3.0', '1.4.0', 'p95', 40, 40,
    40.0, 100.0, 150.0, 41.0, 101.0, 151.0, 1.0, 1.0, -2.0, 4.0,
    0.95, 5.0, 'unchanged', '[\"inside the margin\"]', 100
);
";

    #[test]
    fn a_lake_with_the_first_perf_verdicts_shape_upgrades_and_keeps_its_rows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("lake.duckdb");
        // A raw connection: `Store::open` would migrate at once.
        duckdb::Connection::open(&db)
            .expect("open")
            .execute_batch(FIRST_SHAPE)
            .expect("first shape");

        let store = Store::open(&db).expect("the old lake opens");
        let writer = store.writer().expect("writer");
        let scoped = PerfVerdictRow {
            envs: Some(vec!["demo".into()]),
            ..row("regressed", 200)
        };
        writer
            .insert_perf_verdict(&scoped)
            .expect("the upgraded table takes a row with envs");

        let reader = store.read_only().expect("reader");
        let all = reader.perf_verdicts("checkout", &[], 10).expect("read");
        assert_eq!(all.len(), 2, "{all:?}");
        assert_eq!(all[0]["verdict"], "regressed");
        assert_eq!(all[0]["envs"], serde_json::json!(["demo"]));
        // The row written before the upgrade survived, unscoped.
        assert_eq!(all[1]["verdict"], "unchanged");
        assert_eq!(all[1]["candidate_version"], "1.4.0");
        assert!(all[1]["envs"].is_null(), "{all:?}");
        let demo = reader
            .perf_verdicts("checkout", &["demo".into()], 10)
            .expect("scoped read");
        assert_eq!(demo.len(), 1, "{demo:?}");

        // Opening it again changes nothing.
        drop((reader, writer, store));
        let store = Store::open(&db).expect("reopen");
        let again = store
            .read_only()
            .expect("reader")
            .perf_verdicts("checkout", &[], 10)
            .expect("read");
        assert_eq!(again.len(), 2, "{again:?}");
    }

    #[test]
    fn verdicts_round_trip_newest_first() {
        let d = tempfile::TempDir::new().unwrap();
        let store = Store::open(&d.path().join("lake.duckdb")).unwrap();
        let writer = store.writer().unwrap();
        writer.insert_perf_verdict(&row("unchanged", 10)).unwrap();
        writer.insert_perf_verdict(&row("regressed", 20)).unwrap();

        let reader = store.read_only().unwrap();
        let rows = reader.perf_verdicts("checkout", &[], 10).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["verdict"], "regressed");
        assert_eq!(rows[0]["computed_at_ns"], 20);
        assert_eq!(rows[0]["candidate_p95"], 230.0);
        assert_eq!(rows[0]["baseline_n"], 120);
        assert_eq!(
            rows[0]["reasons"][0],
            "p95 difference interval lies above the margin"
        );
        assert_eq!(rows[1]["verdict"], "unchanged");

        assert_eq!(reader.perf_verdicts("checkout", &[], 1).unwrap().len(), 1);
        assert!(reader.perf_verdicts("other", &[], 10).unwrap().is_empty());
    }

    #[test]
    fn appending_the_same_verdict_twice_keeps_one_row() {
        let d = tempfile::TempDir::new().unwrap();
        let store = Store::open(&d.path().join("lake.duckdb")).unwrap();
        let writer = store.writer().unwrap();
        let count = || {
            let reader = store.read_only().unwrap();
            reader.perf_verdicts("checkout", &[], 50).unwrap().len()
        };
        let demo = |verdict: &str, at: i64| PerfVerdictRow {
            envs: Some(vec!["demo".into()]),
            ..row(verdict, at)
        };
        assert!(writer.append_perf_verdict(&demo("regressed", 10)).unwrap());
        // The retry: same comparison, same evidence, a later clock.
        assert!(!writer.append_perf_verdict(&demo("regressed", 20)).unwrap());
        assert_eq!(count(), 1);

        // Another environment list, quantile or probe is another comparison.
        assert!(writer.append_perf_verdict(&row("regressed", 30)).unwrap());
        assert!(!writer.append_perf_verdict(&row("regressed", 31)).unwrap());
        let p99 = PerfVerdictRow {
            quantile: "p99".into(),
            ..demo("regressed", 40)
        };
        assert!(writer.append_perf_verdict(&p99).unwrap());
        let other_probe = PerfVerdictRow {
            probe: "health".into(),
            ..demo("regressed", 50)
        };
        assert!(writer.append_perf_verdict(&other_probe).unwrap());
        assert_eq!(count(), 4);

        // New evidence appends; going back to the earlier evidence does
        // too, because only the latest row is compared.
        let more = PerfVerdictRow {
            candidate_n: 200,
            ..demo("regressed", 60)
        };
        assert!(writer.append_perf_verdict(&more).unwrap());
        assert!(!writer.append_perf_verdict(&more).unwrap());
        assert!(writer.append_perf_verdict(&demo("regressed", 70)).unwrap());
        let missing = PerfVerdictRow {
            ci_low: None,
            ci_high: None,
            ..demo("inconclusive", 80)
        };
        assert!(writer.append_perf_verdict(&missing).unwrap());
        assert!(!writer.append_perf_verdict(&missing).unwrap());
        assert_eq!(count(), 7);
    }

    #[test]
    fn the_writer_stamps_the_row_and_reads_the_environment_list_as_a_set() {
        let d = tempfile::TempDir::new().unwrap();
        let store = Store::open(&d.path().join("lake.duckdb")).unwrap();
        let writer = store.writer().unwrap();
        let rows = || {
            let reader = store.read_only().unwrap();
            reader.perf_verdicts("checkout", &[], 50).unwrap()
        };
        let in_envs = |envs: &[&str], at: i64| PerfVerdictRow {
            envs: Some(envs.iter().map(ToString::to_string).collect()),
            ..row("regressed", at)
        };
        let before = crate::lake::now_ns();
        // The caller's clock is not what orders the rows: the writer's is.
        assert!(writer
            .append_perf_verdict(&in_envs(&["prod", "demo", "prod"], 5))
            .unwrap());
        let stored = rows();
        assert_eq!(stored[0]["envs"], serde_json::json!(["demo", "prod"]));
        let at = stored[0]["computed_at_ns"].as_i64().unwrap();
        assert!(at >= before && at <= crate::lake::now_ns(), "{at}");
        // The same environments in another order are the same comparison.
        assert!(!writer
            .append_perf_verdict(&in_envs(&["demo", "prod"], 1))
            .unwrap());
        assert_eq!(rows().len(), 1);

        // A caller with an earlier clock still lands after the latest row,
        // and the row it wrote is the one the next request is compared to.
        let more = PerfVerdictRow {
            candidate_n: 300,
            ..in_envs(&["demo", "prod"], 1)
        };
        assert!(writer.append_perf_verdict(&more).unwrap());
        assert!(!writer.append_perf_verdict(&more).unwrap());
        let stored = rows();
        assert_eq!(stored.len(), 2);
        assert_eq!(stored[0]["candidate_n"], 300);
        assert!(stored[0]["computed_at_ns"].as_i64().unwrap() >= at);
    }

    #[test]
    fn a_different_relative_difference_is_different_evidence() {
        let d = tempfile::TempDir::new().unwrap();
        let store = Store::open(&d.path().join("lake.duckdb")).unwrap();
        let writer = store.writer().unwrap();
        assert!(writer.append_perf_verdict(&row("regressed", 10)).unwrap());
        let other = PerfVerdictRow {
            delta_pct: Some(31.0),
            ..row("regressed", 20)
        };
        assert!(writer.append_perf_verdict(&other).unwrap());
        assert!(!writer.append_perf_verdict(&other).unwrap());
        let unknown = PerfVerdictRow {
            delta_pct: None,
            ..row("regressed", 30)
        };
        assert!(writer.append_perf_verdict(&unknown).unwrap());
        assert!(!writer.append_perf_verdict(&unknown).unwrap());
    }

    #[test]
    fn scoped_readers_see_only_verdicts_from_their_environments() {
        let d = tempfile::TempDir::new().unwrap();
        let store = Store::open(&d.path().join("lake.duckdb")).unwrap();
        let writer = store.writer().unwrap();
        let with_envs = |verdict: &str, at: i64, envs: Option<&[&str]>| PerfVerdictRow {
            envs: envs.map(|e| e.iter().map(ToString::to_string).collect()),
            ..row(verdict, at)
        };
        writer
            .insert_perf_verdict(&with_envs("unchanged", 1, None))
            .unwrap();
        writer
            .insert_perf_verdict(&with_envs("regressed", 2, Some(&["demo"])))
            .unwrap();
        writer
            .insert_perf_verdict(&with_envs("improved", 3, Some(&["demo", "prod"])))
            .unwrap();

        let reader = store.read_only().unwrap();
        let verdicts = |scopes: &[&str]| -> Vec<String> {
            let scopes: Vec<String> = scopes.iter().map(ToString::to_string).collect();
            reader
                .perf_verdicts("checkout", &scopes, 10)
                .unwrap()
                .iter()
                .map(|r| r["verdict"].as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(verdicts(&[]), vec!["improved", "regressed", "unchanged"]);
        assert_eq!(verdicts(&["demo"]), vec!["regressed"]);
        assert_eq!(verdicts(&["prod", "demo"]), vec!["improved", "regressed"]);
        assert!(verdicts(&["staging"]).is_empty());

        let rows = reader.perf_verdicts("checkout", &[], 10).unwrap();
        assert_eq!(rows[0]["envs"], serde_json::json!(["demo", "prod"]));
        assert!(rows[2]["envs"].is_null());
    }

    #[test]
    fn missing_percentiles_are_null() {
        let d = tempfile::TempDir::new().unwrap();
        let store = Store::open(&d.path().join("lake.duckdb")).unwrap();
        let writer = store.writer().unwrap();
        let mut r = row("inconclusive", 1);
        r.candidate_n = 0;
        r.candidate_p95 = None;
        r.delta = None;
        r.ci_low = None;
        r.ci_high = None;
        writer.insert_perf_verdict(&r).unwrap();
        let rows = store
            .read_only()
            .unwrap()
            .perf_verdicts("checkout", &[], 10)
            .unwrap();
        assert!(rows[0]["candidate_p95"].is_null());
        assert!(rows[0]["ci_low"].is_null());
        assert_eq!(rows[0]["baseline_p95"], 180.0);
    }
}
