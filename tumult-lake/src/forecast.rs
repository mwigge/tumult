//! Forecast cache (schema v19): `forecasts`, one row per quantile forecast
//! with the provenance that makes it reproducible. Rows are append-only;
//! the reader returns the newest row for a series key, model and shape,
//! and the caller decides whether its context window is still current.

use serde_json::Value;

use crate::error::StoreError;
use crate::{Reader, Writer};

/// One `forecasts` row. `quantiles[step][level]` lines up with
/// `quantile_levels`; gaps are `NaN` in memory and `NULL` in the table.
#[derive(Debug, Clone, PartialEq)]
pub struct ForecastRow {
    pub series_key: String,
    pub model: String,
    pub model_version: String,
    pub weights_hash: String,
    pub context_start_ns: i64,
    pub context_end_ns: i64,
    pub context_points: i64,
    pub insufficient_history: bool,
    pub horizon: i64,
    pub step_ns: i64,
    pub quantile_levels: Vec<f64>,
    pub quantiles: Vec<Vec<f64>>,
    pub created_at_ns: i64,
}

/// Which model's rows a cache lookup accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelFilter<'a> {
    /// Rows written by exactly this model.
    Is(&'a str),
    /// Rows written by any other model (a sidecar names its own model).
    IsNot(&'a str),
}

impl ModelFilter<'_> {
    fn predicate(self) -> String {
        let (op, name) = match self {
            Self::Is(name) => ("=", name),
            Self::IsNot(name) => ("<>", name),
        };
        format!("model {op} '{}'", name.replace('\'', "''"))
    }
}

/// A `DOUBLE[]` literal; non-finite values become `NULL`.
fn list_literal(values: &[f64]) -> String {
    let items: Vec<String> = values
        .iter()
        .map(|v| {
            if v.is_finite() {
                format!("{v:?}")
            } else {
                "NULL".to_string()
            }
        })
        .collect();
    format!("[{}]", items.join(", "))
}

fn nested_literal(rows: &[Vec<f64>]) -> String {
    let items: Vec<String> = rows.iter().map(|r| list_literal(r)).collect();
    format!("[{}]", items.join(", "))
}

impl Writer {
    /// Append a forecast row.
    ///
    /// # Errors
    /// Returns an error if the insert fails.
    pub fn insert_forecast(&self, row: &ForecastRow) -> Result<(), StoreError> {
        let sql = format!(
            "INSERT INTO forecasts (series_key, model, model_version, weights_hash, \
             context_start_ns, context_end_ns, context_points, insufficient_history, \
             horizon, step_ns, quantile_levels, quantiles, created_at_ns) \
             VALUES (?,?,?,?,?,?,?,?,?,?, CAST({} AS DOUBLE[]), CAST({} AS DOUBLE[][]), ?)",
            list_literal(&row.quantile_levels),
            nested_literal(&row.quantiles)
        );
        self.conn.execute(
            &sql,
            duckdb::params![
                row.series_key,
                row.model,
                row.model_version,
                row.weights_hash,
                row.context_start_ns,
                row.context_end_ns,
                row.context_points,
                row.insufficient_history,
                row.horizon,
                row.step_ns,
                row.created_at_ns
            ],
        )?;
        Ok(())
    }
}

impl Reader {
    /// The newest cached forecast for a series key, model filter, step
    /// and horizon, when one exists.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn latest_forecast(
        &self,
        series_key: &str,
        model: ModelFilter<'_>,
        step_ns: i64,
        horizon: i64,
    ) -> Result<Option<ForecastRow>, StoreError> {
        let rows = self.query_json_rows(&format!(
            "SELECT * FROM forecasts WHERE series_key = '{}' AND {} \
             AND step_ns = {step_ns} AND horizon = {horizon} \
             ORDER BY created_at_ns DESC, rowid DESC LIMIT 1",
            series_key.replace('\'', "''"),
            model.predicate()
        ))?;
        Ok(rows.first().map(row_to_forecast))
    }
}

fn str_field(row: &Value, key: &str) -> String {
    row.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn i64_field(row: &Value, key: &str) -> i64 {
    row.get(key).and_then(Value::as_i64).unwrap_or_default()
}

fn f64_list(value: Option<&Value>) -> Vec<f64> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|v| v.as_f64().unwrap_or(f64::NAN))
                .collect()
        })
        .unwrap_or_default()
}

fn row_to_forecast(row: &Value) -> ForecastRow {
    ForecastRow {
        series_key: str_field(row, "series_key"),
        model: str_field(row, "model"),
        model_version: str_field(row, "model_version"),
        weights_hash: str_field(row, "weights_hash"),
        context_start_ns: i64_field(row, "context_start_ns"),
        context_end_ns: i64_field(row, "context_end_ns"),
        context_points: i64_field(row, "context_points"),
        insufficient_history: row
            .get("insufficient_history")
            .and_then(Value::as_bool)
            .unwrap_or_default(),
        horizon: i64_field(row, "horizon"),
        step_ns: i64_field(row, "step_ns"),
        quantile_levels: f64_list(row.get("quantile_levels")),
        quantiles: row
            .get("quantiles")
            .and_then(Value::as_array)
            .map(|rows| rows.iter().map(|r| f64_list(Some(r))).collect())
            .unwrap_or_default(),
        created_at_ns: i64_field(row, "created_at_ns"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;

    fn row(created_at_ns: i64, model: &str) -> ForecastRow {
        ForecastRow {
            series_key: "metric:p95|target:db".into(),
            model: model.into(),
            model_version: "1.0".into(),
            weights_hash: "none".into(),
            context_start_ns: 1_000,
            context_end_ns: 2_000,
            context_points: 10,
            insufficient_history: false,
            horizon: 2,
            step_ns: 100,
            quantile_levels: vec![0.1, 0.5, 0.9],
            quantiles: vec![vec![1.0, 2.0, 3.0], vec![1.5, f64::NAN, 3.5]],
            created_at_ns,
        }
    }

    #[test]
    fn forecasts_round_trip_and_the_newest_row_wins() {
        let d = tempfile::TempDir::new().unwrap();
        let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
        let writer = store.writer().unwrap();
        writer.insert_forecast(&row(10, "holt-winters")).unwrap();
        writer.insert_forecast(&row(20, "holt-winters")).unwrap();
        writer.insert_forecast(&row(30, "timestral")).unwrap();

        let reader = store.read_only().unwrap();
        let hit = reader
            .latest_forecast(
                "metric:p95|target:db",
                ModelFilter::Is("holt-winters"),
                100,
                2,
            )
            .unwrap()
            .unwrap();
        assert_eq!(hit.created_at_ns, 20);
        assert_eq!(hit.quantile_levels, vec![0.1, 0.5, 0.9]);
        assert_eq!(hit.quantiles[0], vec![1.0, 2.0, 3.0]);
        assert!(hit.quantiles[1][1].is_nan(), "gaps survive as NaN");
        assert_eq!(hit.quantiles[1][2], 3.5);
        assert_eq!(hit.context_end_ns, 2_000);
        assert!(!hit.insufficient_history);
        assert_eq!(hit.model_version, "1.0");

        assert!(reader
            .latest_forecast(
                "metric:p95|target:db",
                ModelFilter::Is("holt-winters"),
                100,
                3
            )
            .unwrap()
            .is_none());
        assert!(reader
            .latest_forecast("other", ModelFilter::Is("holt-winters"), 100, 2)
            .unwrap()
            .is_none());
        assert_eq!(
            reader
                .latest_forecast("metric:p95|target:db", ModelFilter::Is("timestral"), 100, 2)
                .unwrap()
                .unwrap()
                .created_at_ns,
            30
        );
    }

    /// A v18 lake (every table up to `perf_verdicts`, no `forecasts`,
    /// version recorded as 18) gains the forecast cache on open, keeps
    /// its rows and advances to the current version.
    #[test]
    fn a_v18_lake_gains_the_forecasts_table_on_open() {
        let d = tempfile::TempDir::new().unwrap();
        let path = d.path().join("kronika.duckdb");
        drop(Store::open(&path).unwrap());
        {
            let conn = duckdb::Connection::open(&path).unwrap();
            conn.execute_batch(
                "DROP TABLE forecasts;
                 UPDATE schema_meta SET value = 18 WHERE key = 'version';
                 INSERT INTO users VALUES ('u-v18', 'kept', '!', 'viewer', false, true, 0);",
            )
            .unwrap();
            let tables: i64 = conn
                .query_row(
                    "SELECT count(*) FROM information_schema.tables WHERE table_name = 'forecasts'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(tables, 0, "the seeded lake is v18-shaped");
        }

        let store = Store::open(&path).unwrap();
        let writer = store.writer().unwrap();
        assert_eq!(
            writer.schema_version().unwrap(),
            crate::CURRENT_SCHEMA_VERSION
        );
        writer.insert_forecast(&row(10, "holt-winters")).unwrap();
        let reader = store.read_only().unwrap();
        let hit = reader
            .latest_forecast(
                "metric:p95|target:db",
                ModelFilter::Is("holt-winters"),
                100,
                2,
            )
            .unwrap();
        assert_eq!(hit.unwrap().created_at_ns, 10);
        let kept = reader
            .query_json_rows("SELECT count(*) AS c FROM users WHERE id = 'u-v18'")
            .unwrap();
        assert_eq!(kept[0]["c"].as_u64(), Some(1), "v18 rows survive");
        let perf = reader
            .query_json_rows("SELECT count(*) AS c FROM perf_verdicts")
            .unwrap();
        assert_eq!(perf[0]["c"].as_u64(), Some(0), "the v18 table is intact");
    }

    /// A sidecar names its own model, so its rows are found as "anything
    /// the built-in provider did not write".
    #[test]
    fn a_filter_can_exclude_a_model() {
        let d = tempfile::TempDir::new().unwrap();
        let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
        let writer = store.writer().unwrap();
        let other = ModelFilter::IsNot("holt-winters");
        writer.insert_forecast(&row(10, "holt-winters")).unwrap();
        assert!(store
            .read_only()
            .unwrap()
            .latest_forecast("metric:p95|target:db", other, 100, 2)
            .unwrap()
            .is_none());
        writer.insert_forecast(&row(20, "timestral")).unwrap();
        writer.insert_forecast(&row(30, "holt-winters")).unwrap();
        let hit = store
            .read_only()
            .unwrap()
            .latest_forecast("metric:p95|target:db", other, 100, 2)
            .unwrap()
            .unwrap();
        assert_eq!(hit.model, "timestral");
        assert_eq!(hit.created_at_ns, 20);
    }

    #[test]
    fn list_literals_render_gaps_as_null() {
        assert_eq!(list_literal(&[0.1, f64::NAN, 2.0]), "[0.1, NULL, 2.0]");
        assert_eq!(nested_literal(&[vec![1.0], vec![]]), "[[1.0], []]");
    }
}
