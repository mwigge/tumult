//! v19 forecast DDL: `forecasts`, the cache of quantile forecasts with the
//! provenance that makes a band reproducible (model, version, weights
//! hash, context window, step). Additive and index-free under the same
//! crash-robustness rule as the v5 run tables; rows are never updated, a
//! newer row for the same series key supersedes the older one.

/// One row per forecast. `quantiles[step][level]` matches
/// `quantile_levels`; `NULL` elements are gaps the model could not fill.
pub(super) const DDL: &str = "
CREATE TABLE IF NOT EXISTS forecasts (
    series_key VARCHAR NOT NULL, model VARCHAR NOT NULL,
    model_version VARCHAR NOT NULL, weights_hash VARCHAR NOT NULL,
    context_start_ns BIGINT NOT NULL, context_end_ns BIGINT NOT NULL,
    context_points BIGINT NOT NULL, insufficient_history BOOLEAN NOT NULL,
    horizon BIGINT NOT NULL, step_ns BIGINT NOT NULL,
    quantile_levels DOUBLE[] NOT NULL, quantiles DOUBLE[][] NOT NULL,
    created_at_ns BIGINT NOT NULL
);
";
