//! Load executors live in `tumult_core::runner` (`k6`, `oha`, selected by
//! `load_executor_for`); this module only re-exports the k6 parsing helpers
//! the CLI tests cover.

#[cfg(test)]
pub(crate) use tumult_core::runner::k6::{
    k6_metric_or_warn, k6_summary_count, k6_summary_metric, parse_k6_counter, parse_k6_metric,
    parse_k6_rate, read_k6_summary,
};
