//! v19 forecast DDL: `forecasts`, the cache of quantile forecasts with the
//! provenance that makes a band reproducible (model, version, weights
//! hash, context window, step). Additive; rows are never updated, a newer
//! row for the same series key and context window supersedes the older
//! one, and rows past the retention are deleted by the writer.
//!
//! Unlike the v5 run tables this one carries an index: every chart view
//! looks the cache up by series key, and losing the table to a crash costs
//! nothing but a refit. The prune by `created_at_ns` has none: DuckDB
//! plans that range under a top-N as a sequential scan either way, so an
//! index on the column was only something to maintain on every write.
//! Lakes created while there was one lose it on open. `degenerate_band`
//! arrived after the first v19 lakes were created; like the dropped index
//! it is idempotent (`ADD COLUMN IF NOT EXISTS`, `DROP INDEX IF EXISTS`),
//! so an early v19 lake is brought in line on open without a version of
//! its own.

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
    created_at_ns BIGINT NOT NULL,
    degenerate_band BOOLEAN DEFAULT FALSE
);
ALTER TABLE forecasts ADD COLUMN IF NOT EXISTS degenerate_band BOOLEAN DEFAULT FALSE;
CREATE INDEX IF NOT EXISTS idx_forecasts_series_key ON forecasts (series_key);
DROP INDEX IF EXISTS idx_forecasts_created_at;
";
