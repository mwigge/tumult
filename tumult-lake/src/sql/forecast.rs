//! The statements of the forecast cache (`forecast.rs`).

pub(crate) const PRUNE: &str = "DELETE FROM forecasts WHERE rowid IN (SELECT rowid FROM forecasts \
    WHERE created_at_ns < ? ORDER BY created_at_ns, rowid LIMIT ?)";

pub(crate) const CAP: &str = "DELETE FROM forecasts WHERE rowid IN (SELECT rowid FROM forecasts \
    WHERE series_key = ? ORDER BY created_at_ns DESC, rowid DESC \
    OFFSET ?)";

/// The columns a lookup reads, in `ForecastRow` order.
const COLUMNS: &str = "series_key, model, model_version, weights_hash, context_start_ns, \
     context_end_ns, context_points, insufficient_history, degenerate_band, horizon, step_ns, \
     quantile_levels, quantiles, created_at_ns";

/// Append a forecast, with its quantile levels and quantiles as the list
/// literals `levels` and `quantiles`.
pub(crate) fn insert(levels: &str, quantiles: &str) -> String {
    format!(
        "INSERT INTO forecasts (series_key, model, model_version, weights_hash, \
         context_start_ns, context_end_ns, context_points, insufficient_history, \
         degenerate_band, horizon, step_ns, quantile_levels, quantiles, created_at_ns) \
         VALUES (?,?,?,?,?,?,?,?,?,?,?, CAST({levels} AS DOUBLE[]), CAST({quantiles} AS DOUBLE[][]), ?)"
    )
}

/// The newest forecast of a lookup, with `operator` comparing the model.
pub(crate) fn latest(operator: &str) -> String {
    format!(
        "SELECT row_to_json(t) AS j FROM (SELECT {COLUMNS} FROM forecasts \
         WHERE series_key = ? AND model {operator} ? AND step_ns = ? AND horizon = ? \
         AND context_start_ns = ? AND context_end_ns = ? \
         ORDER BY created_at_ns DESC, rowid DESC LIMIT 1) AS t"
    )
}

/// The statements of this module, for the SQL corpus.
#[cfg(test)]
pub(super) fn sql_corpus() -> Vec<tumult_test_utils::sql_corpus::Statement> {
    tumult_test_utils::sql_statements![
        PRUNE,
        CAP,
        insert("[0.1, 0.5, 0.9]", "[[1.0, 2.0, 3.0]]"),
        latest("="),
        latest("<>")
    ]
}
