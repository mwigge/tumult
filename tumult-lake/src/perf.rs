//! Performance-regression verdict storage (schema v18): the `perf_verdicts`
//! table, one row per verdict computed between two `service.version`s of a
//! target. Rows are append-only — a recomputation adds a newer row — so
//! `GET /api/perf/{target}` and the CLI read the latest row per
//! (target, probe, baseline, candidate) and the history stays auditable.

use duckdb::params;
use serde_json::Value;

use crate::error::StoreError;
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
            "INSERT INTO perf_verdicts (
                target, probe, baseline_version, candidate_version, quantile,
                baseline_n, candidate_n,
                baseline_p50, baseline_p95, baseline_p99,
                candidate_p50, candidate_p95, candidate_p99,
                delta, delta_pct, ci_low, ci_high, confidence, margin,
                verdict, reasons, computed_at_ns, envs
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        let mut stmt = self.conn.prepare(
            "SELECT target, probe, baseline_version, candidate_version, quantile,
                    baseline_n, candidate_n,
                    baseline_p50, baseline_p95, baseline_p99,
                    candidate_p50, candidate_p95, candidate_p99,
                    delta, delta_pct, ci_low, ci_high, confidence, margin,
                    verdict, CAST(reasons AS VARCHAR), computed_at_ns,
                    CAST(envs AS VARCHAR)
             FROM perf_verdicts WHERE target = ?
               AND (? OR (envs IS NOT NULL AND list_has_all(
                    CAST(CAST(? AS JSON) AS VARCHAR[]), CAST(envs AS VARCHAR[]))))
             ORDER BY computed_at_ns DESC LIMIT ?",
        )?;
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
