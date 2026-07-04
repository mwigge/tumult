//! K6 load executor — re-exported from `tumult_core::runner::k6`, which owns
//! the shared implementation (used by `tumult run --load`, `tumult gameday
//! run`, and the MCP server's `tumult_gameday_run`).

pub(crate) use tumult_core::runner::k6::K6LoadExecutor;

#[cfg(test)]
pub(crate) use tumult_core::runner::k6::{
    k6_metric_or_warn, k6_summary_count, k6_summary_metric, parse_k6_counter, parse_k6_metric,
    parse_k6_rate, read_k6_summary,
};
