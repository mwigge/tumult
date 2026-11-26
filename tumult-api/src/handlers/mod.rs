//! HTTP handlers for the read-only query API, one module per domain.

pub mod conformance;
pub mod dora;
pub mod dr;
pub mod entities;
pub(crate) mod evidence;
pub mod experiments;
pub mod gate;
pub mod logs;
pub mod metrics;
pub mod overview;
pub mod perf;
pub mod recommend;
pub mod reports;
pub mod risks;
pub mod starmap;
pub(crate) mod starmap_cache;
pub mod topology;
pub mod traces;

/// The statements of the handlers, for the SQL corpus.
#[cfg(test)]
pub(crate) fn sql_corpus() -> Vec<tumult_test_utils::sql_corpus::Statement> {
    [
        conformance::sql_corpus(),
        dr::sql_corpus(),
        entities::sql_corpus(),
        evidence::sql_corpus(),
        experiments::sql_corpus(),
        logs::sql_corpus(),
        metrics::sql_corpus(),
        overview::sql_corpus(),
        perf::sql_corpus(),
        recommend::sql_corpus(),
        risks::sql_corpus(),
        starmap::sql_corpus(),
        topology::sql_corpus(),
        traces::sql_corpus(),
    ]
    .concat()
}
