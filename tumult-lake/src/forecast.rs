//! Forecast cache (schema v19): `forecasts`, one row per quantile forecast
//! with the provenance that makes it reproducible. Rows are append-only;
//! the reader returns the newest row for one series key, model, shape and
//! context window, so a forecast fitted on one window is never served for
//! another. The table is bounded twice over: rows older than a retention
//! the caller chooses are pruned a batch at a time, and one series key
//! keeps only its newest rows.

use serde_json::Value;

use crate::error::StoreError;
use crate::{Reader, Writer};

/// How long a forecast row is kept by default: long enough to reproduce a
/// band shown on a recent report, short enough that a dashboard polling
/// every minute cannot grow the table without bound.
pub const FORECAST_RETENTION_NS: i64 = 30 * 86_400 * 1_000_000_000;

/// How many rows one series key keeps by default. A chart polling every
/// minute over a few ranges and horizon buckets stays far below it within
/// the hour a cached row can still be hit; a client inventing horizons or
/// ranges for one series cannot push it past this.
pub const FORECAST_ROWS_PER_SERIES: usize = 128;

/// How many expired rows one write deletes by default: more than one
/// write adds, so a backlog shrinks, and few enough to stay cheap.
pub const FORECAST_PRUNE_BATCH: usize = 256;

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
    /// The band was widened to the series' floor (see `tumult-forecast`).
    pub degenerate_band: bool,
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

impl<'a> ModelFilter<'a> {
    /// The comparison operator and the model name it is bound against.
    fn operator(self) -> (&'static str, &'a str) {
        match self {
            Self::Is(name) => ("=", name),
            Self::IsNot(name) => ("<>", name),
        }
    }
}

/// Everything that identifies one cached forecast: the series, who made
/// it, its shape and the exact context window it was fitted on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForecastLookup<'a> {
    pub series_key: &'a str,
    pub model: ModelFilter<'a>,
    pub step_ns: i64,
    pub horizon: i64,
    pub context_start_ns: i64,
    pub context_end_ns: i64,
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

/// The columns a lookup reads, in [`ForecastRow`] order.
const COLUMNS: &str = "series_key, model, model_version, weights_hash, context_start_ns, \
     context_end_ns, context_points, insufficient_history, degenerate_band, horizon, step_ns, \
     quantile_levels, quantiles, created_at_ns";

impl Writer {
    /// Append a forecast row.
    ///
    /// # Errors
    /// Returns an error if the insert fails.
    pub fn insert_forecast(&self, row: &ForecastRow) -> Result<(), StoreError> {
        let sql = format!(
            "INSERT INTO forecasts (series_key, model, model_version, weights_hash, \
             context_start_ns, context_end_ns, context_points, insufficient_history, \
             degenerate_band, horizon, step_ns, quantile_levels, quantiles, created_at_ns) \
             VALUES (?,?,?,?,?,?,?,?,?,?,?, CAST({} AS DOUBLE[]), CAST({} AS DOUBLE[][]), ?)",
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
                row.degenerate_band,
                row.horizon,
                row.step_ns,
                row.created_at_ns
            ],
        )?;
        Ok(())
    }

    /// Delete at most `limit` of the forecast rows created before
    /// `cutoff_ns`, oldest first; returns how many went. Bounded so one
    /// write never pays for a backlog: the next write takes the next
    /// batch.
    ///
    /// # Errors
    /// Returns an error if the delete fails.
    pub fn prune_forecasts(&self, cutoff_ns: i64, limit: usize) -> Result<usize, StoreError> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        Ok(self.conn.execute(
            "DELETE FROM forecasts WHERE rowid IN (SELECT rowid FROM forecasts \
             WHERE created_at_ns < ? ORDER BY created_at_ns, rowid LIMIT ?)",
            duckdb::params![cutoff_ns, limit],
        )?)
    }

    /// Keep only the `keep` newest forecast rows of `series_key`; returns
    /// how many older ones went.
    ///
    /// # Errors
    /// Returns an error if the delete fails.
    pub fn cap_forecasts(&self, series_key: &str, keep: usize) -> Result<usize, StoreError> {
        let keep = i64::try_from(keep).unwrap_or(i64::MAX);
        Ok(self.conn.execute(
            "DELETE FROM forecasts WHERE rowid IN (SELECT rowid FROM forecasts \
             WHERE series_key = ? ORDER BY created_at_ns DESC, rowid DESC \
             LIMIT 9223372036854775807 OFFSET ?)",
            duckdb::params![series_key, keep],
        )?)
    }
}

impl Reader {
    /// The newest cached forecast matching `lookup` exactly, when one
    /// exists.
    ///
    /// # Errors
    /// Returns an error if the query fails or the row cannot be decoded.
    pub fn latest_forecast(
        &self,
        lookup: &ForecastLookup<'_>,
    ) -> Result<Option<ForecastRow>, StoreError> {
        let (operator, model) = lookup.model.operator();
        let sql = format!(
            "SELECT row_to_json(t) AS j FROM (SELECT {COLUMNS} FROM forecasts \
             WHERE series_key = ? AND model {operator} ? AND step_ns = ? AND horizon = ? \
             AND context_start_ns = ? AND context_end_ns = ? \
             ORDER BY created_at_ns DESC, rowid DESC LIMIT 1) AS t"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query_map(
            duckdb::params![
                lookup.series_key,
                model,
                lookup.step_ns,
                lookup.horizon,
                lookup.context_start_ns,
                lookup.context_end_ns
            ],
            |r| r.get::<usize, String>(0),
        )?;
        let Some(json) = rows.next() else {
            return Ok(None);
        };
        let row: Value = serde_json::from_str(&json?)?;
        Ok(Some(row_to_forecast(&row)))
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

fn bool_field(row: &Value, key: &str) -> bool {
    row.get(key).and_then(Value::as_bool).unwrap_or_default()
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
        insufficient_history: bool_field(row, "insufficient_history"),
        degenerate_band: bool_field(row, "degenerate_band"),
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

    const KEY: &str = r#"["p95","db",[]]"#;

    fn row(created_at_ns: i64, model: &str) -> ForecastRow {
        ForecastRow {
            series_key: KEY.into(),
            model: model.into(),
            model_version: "1.0".into(),
            weights_hash: "none".into(),
            context_start_ns: 1_000,
            context_end_ns: 2_000,
            context_points: 10,
            insufficient_history: false,
            degenerate_band: false,
            horizon: 2,
            step_ns: 100,
            quantile_levels: vec![0.1, 0.5, 0.9],
            quantiles: vec![vec![1.0, 2.0, 3.0], vec![1.5, f64::NAN, 3.5]],
            created_at_ns,
        }
    }

    fn lookup(model: ModelFilter<'_>) -> ForecastLookup<'_> {
        ForecastLookup {
            series_key: KEY,
            model,
            step_ns: 100,
            horizon: 2,
            context_start_ns: 1_000,
            context_end_ns: 2_000,
        }
    }

    const BUILT_IN: ModelFilter<'static> = ModelFilter::Is("holt-winters");

    #[test]
    fn forecasts_round_trip_and_the_newest_row_wins() {
        let d = tempfile::TempDir::new().unwrap();
        let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
        let writer = store.writer().unwrap();
        writer.insert_forecast(&row(10, "holt-winters")).unwrap();
        writer
            .insert_forecast(&ForecastRow {
                degenerate_band: true,
                ..row(20, "holt-winters")
            })
            .unwrap();
        writer.insert_forecast(&row(30, "timestral")).unwrap();

        let reader = store.read_only().unwrap();
        let hit = reader.latest_forecast(&lookup(BUILT_IN)).unwrap().unwrap();
        assert_eq!(hit.created_at_ns, 20);
        assert_eq!(hit.quantile_levels, vec![0.1, 0.5, 0.9]);
        assert_eq!(hit.quantiles[0], vec![1.0, 2.0, 3.0]);
        assert!(hit.quantiles[1][1].is_nan(), "gaps survive as NaN");
        assert_eq!(hit.quantiles[1][2], 3.5);
        assert_eq!(hit.context_end_ns, 2_000);
        assert!(!hit.insufficient_history);
        assert!(hit.degenerate_band, "the flag round-trips");
        assert_eq!(hit.model_version, "1.0");

        let other_horizon = ForecastLookup {
            horizon: 3,
            ..lookup(BUILT_IN)
        };
        assert!(reader.latest_forecast(&other_horizon).unwrap().is_none());
        let other_series = ForecastLookup {
            series_key: "other",
            ..lookup(BUILT_IN)
        };
        assert!(reader.latest_forecast(&other_series).unwrap().is_none());
        let sidecar = reader
            .latest_forecast(&lookup(ModelFilter::Is("timestral")))
            .unwrap()
            .unwrap();
        assert_eq!(sidecar.created_at_ns, 30);
    }

    /// The same series, model, step and horizon fitted on another context
    /// window is another forecast: a longer look-back or a later window
    /// end must both miss.
    #[test]
    fn a_lookup_matches_the_context_window_exactly() {
        let d = tempfile::TempDir::new().unwrap();
        let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
        store
            .writer()
            .unwrap()
            .insert_forecast(&row(10, "holt-winters"))
            .unwrap();
        let reader = store.read_only().unwrap();
        assert!(reader.latest_forecast(&lookup(BUILT_IN)).unwrap().is_some());
        let longer_context = ForecastLookup {
            context_start_ns: 500,
            ..lookup(BUILT_IN)
        };
        assert!(reader.latest_forecast(&longer_context).unwrap().is_none());
        let later_window = ForecastLookup {
            context_end_ns: 2_100,
            ..lookup(BUILT_IN)
        };
        assert!(reader.latest_forecast(&later_window).unwrap().is_none());
    }

    /// Keys and model names are bound, never spliced into the statement.
    #[test]
    fn hostile_keys_and_models_are_only_values() {
        let d = tempfile::TempDir::new().unwrap();
        let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
        let hostile = "x' OR '1'='1";
        let writer = store.writer().unwrap();
        writer.insert_forecast(&row(10, "holt-winters")).unwrap();
        writer
            .insert_forecast(&ForecastRow {
                series_key: hostile.into(),
                model: hostile.into(),
                ..row(20, "x")
            })
            .unwrap();
        let reader = store.read_only().unwrap();
        let by_key = ForecastLookup {
            series_key: hostile,
            ..lookup(ModelFilter::Is(hostile))
        };
        let hit = reader.latest_forecast(&by_key).unwrap().unwrap();
        assert_eq!(hit.created_at_ns, 20, "only the row with that exact key");
        assert_eq!(hit.series_key, hostile);
        let no_such_model = lookup(ModelFilter::Is(hostile));
        assert!(reader.latest_forecast(&no_such_model).unwrap().is_none());
        let everything_else = lookup(ModelFilter::IsNot(hostile));
        assert_eq!(
            reader
                .latest_forecast(&everything_else)
                .unwrap()
                .unwrap()
                .created_at_ns,
            10
        );
    }

    #[test]
    fn pruning_drops_only_rows_older_than_the_cutoff() {
        let d = tempfile::TempDir::new().unwrap();
        let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
        let writer = store.writer().unwrap();
        for created in [10, 20, 30] {
            writer
                .insert_forecast(&row(created, "holt-winters"))
                .unwrap();
        }
        assert_eq!(writer.prune_forecasts(5, 100).unwrap(), 0);
        assert_eq!(writer.prune_forecasts(30, 100).unwrap(), 2);
        let reader = store.read_only().unwrap();
        let left = reader
            .query_json_rows("SELECT count(*) AS c, min(created_at_ns) AS oldest FROM forecasts")
            .unwrap();
        assert_eq!(left[0]["c"].as_u64(), Some(1));
        assert_eq!(left[0]["oldest"].as_i64(), Some(30));
        assert_eq!(
            reader
                .latest_forecast(&lookup(BUILT_IN))
                .unwrap()
                .unwrap()
                .created_at_ns,
            30
        );
    }

    #[test]
    fn the_lookup_is_indexed_by_series_key() {
        let d = tempfile::TempDir::new().unwrap();
        let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
        let indexes = store
            .read_only()
            .unwrap()
            .query_json_rows(
                "SELECT index_name FROM duckdb_indexes() WHERE table_name = 'forecasts'",
            )
            .unwrap();
        assert_eq!(indexes.len(), 2, "{indexes:?}");
        let names: Vec<&str> = indexes
            .iter()
            .filter_map(|i| i["index_name"].as_str())
            .collect();
        assert!(names.contains(&"idx_forecasts_series_key"), "{names:?}");
        assert!(names.contains(&"idx_forecasts_created_at"), "{names:?}");
    }

    fn count(store: &Store, predicate: &str) -> u64 {
        let sql = format!("SELECT count(*) AS c FROM forecasts WHERE {predicate}");
        let rows = store.read_only().unwrap().query_json_rows(&sql).unwrap();
        rows[0]["c"].as_u64().unwrap()
    }

    /// A backlog of expired rows is worked off a batch at a time, oldest
    /// first, so no single write pays for all of it.
    #[test]
    fn a_prune_deletes_at_most_its_limit_oldest_first() {
        let d = tempfile::TempDir::new().unwrap();
        let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
        let writer = store.writer().unwrap();
        for created in [50, 10, 40, 20, 30, 60] {
            writer
                .insert_forecast(&row(created, "holt-winters"))
                .unwrap();
        }
        assert_eq!(writer.prune_forecasts(55, 2).unwrap(), 2);
        assert_eq!(count(&store, "created_at_ns IN (10, 20)"), 0);
        assert_eq!(count(&store, "true"), 4);
        assert_eq!(writer.prune_forecasts(55, 2).unwrap(), 2);
        assert_eq!(writer.prune_forecasts(55, 2).unwrap(), 1);
        assert_eq!(writer.prune_forecasts(55, 2).unwrap(), 0);
        assert_eq!(count(&store, "created_at_ns = 60"), 1);
        assert_eq!(count(&store, "true"), 1);
        assert_eq!(writer.prune_forecasts(100, 0).unwrap(), 0, "no budget");
    }

    /// One series keeps its newest rows only; another series' rows are
    /// not counted against it and are left alone.
    #[test]
    fn a_cap_evicts_the_oldest_rows_of_one_series_only() {
        let d = tempfile::TempDir::new().unwrap();
        let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
        let writer = store.writer().unwrap();
        for created in [30, 10, 50, 20, 40] {
            writer
                .insert_forecast(&row(created, "holt-winters"))
                .unwrap();
            writer
                .insert_forecast(&ForecastRow {
                    series_key: "other".into(),
                    ..row(created, "holt-winters")
                })
                .unwrap();
        }
        assert_eq!(writer.cap_forecasts(KEY, 5).unwrap(), 0, "at the cap");
        assert_eq!(writer.cap_forecasts(KEY, 2).unwrap(), 3);
        assert_eq!(count(&store, "series_key <> 'other'"), 2);
        assert_eq!(
            count(&store, "series_key <> 'other' AND created_at_ns >= 40"),
            2,
            "the newest two stay"
        );
        assert_eq!(count(&store, "series_key = 'other'"), 5);
        assert_eq!(writer.cap_forecasts(KEY, 2).unwrap(), 0);
        assert_eq!(writer.cap_forecasts("x' OR '1'='1", 0).unwrap(), 0);
        assert_eq!(count(&store, "true"), 7, "a hostile key is only a value");
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
        let hit = reader.latest_forecast(&lookup(BUILT_IN)).unwrap();
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

    /// A lake whose `forecasts` table predates the `degenerate_band`
    /// column gains it on open; the rows already there read as not
    /// degenerate.
    #[test]
    fn an_early_v19_table_gains_the_degenerate_band_column() {
        let d = tempfile::TempDir::new().unwrap();
        let path = d.path().join("kronika.duckdb");
        drop(Store::open(&path).unwrap());
        {
            let conn = duckdb::Connection::open(&path).unwrap();
            conn.execute_batch(
                "DROP TABLE forecasts;
                 CREATE TABLE forecasts (
                     series_key VARCHAR NOT NULL, model VARCHAR NOT NULL,
                     model_version VARCHAR NOT NULL, weights_hash VARCHAR NOT NULL,
                     context_start_ns BIGINT NOT NULL, context_end_ns BIGINT NOT NULL,
                     context_points BIGINT NOT NULL, insufficient_history BOOLEAN NOT NULL,
                     horizon BIGINT NOT NULL, step_ns BIGINT NOT NULL,
                     quantile_levels DOUBLE[] NOT NULL, quantiles DOUBLE[][] NOT NULL,
                     created_at_ns BIGINT NOT NULL);
                 INSERT INTO forecasts VALUES ('[\"p95\",\"db\",[]]', 'holt-winters', '1.0',
                     'none', 1000, 2000, 10, false, 2, 100, [0.5], [[1.0], [2.0]], 5);",
            )
            .unwrap();
        }
        let store = Store::open(&path).unwrap();
        let reader = store.read_only().unwrap();
        let old = reader.latest_forecast(&lookup(BUILT_IN)).unwrap().unwrap();
        assert_eq!(old.created_at_ns, 5);
        assert!(!old.degenerate_band);
        assert_eq!(old.quantiles, vec![vec![1.0], vec![2.0]]);
    }

    /// A sidecar names its own model, so its rows are found as "anything
    /// the built-in provider did not write".
    #[test]
    fn a_filter_can_exclude_a_model() {
        let d = tempfile::TempDir::new().unwrap();
        let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
        let writer = store.writer().unwrap();
        let other = lookup(ModelFilter::IsNot("holt-winters"));
        writer.insert_forecast(&row(10, "holt-winters")).unwrap();
        assert!(store
            .read_only()
            .unwrap()
            .latest_forecast(&other)
            .unwrap()
            .is_none());
        writer.insert_forecast(&row(20, "timestral")).unwrap();
        writer.insert_forecast(&row(30, "holt-winters")).unwrap();
        let hit = store
            .read_only()
            .unwrap()
            .latest_forecast(&other)
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
