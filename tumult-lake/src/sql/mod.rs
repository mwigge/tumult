//! The SQL of the lake outside the schema, as named statements.
//!
//! A statement lives here as a const, or as a function where a part of it
//! is chosen at run time. The code that runs a statement takes it from
//! here, so that a test can collect every statement the lake runs into
//! its SQL corpus, `tests/golden/sql-corpus.sql`. The test prepares each
//! statement on a fresh lake, and CI lints the file with sqlfluff. The
//! tables, the views and the migrations are in `schema`.

pub(crate) mod analytics;
pub(crate) mod approvals;
pub(crate) mod auth;
pub(crate) mod entities;
pub(crate) mod forecast;
pub(crate) mod ict;
pub(crate) mod manual;
pub(crate) mod perf;
pub(crate) mod reader;
pub(crate) mod risk;
pub(crate) mod runs;
pub(crate) mod schedules;
pub(crate) mod sightings;
pub(crate) mod store;
pub(crate) mod tx;
pub(crate) mod webhooks;
pub(crate) mod writer;

#[cfg(test)]
mod tests;

/// The statements of this module, for the SQL corpus.
#[cfg(test)]
fn sql_corpus() -> Vec<tumult_test_utils::sql_corpus::Statement> {
    [
        analytics::sql_corpus(),
        approvals::sql_corpus(),
        auth::sql_corpus(),
        entities::sql_corpus(),
        forecast::sql_corpus(),
        ict::sql_corpus(),
        manual::sql_corpus(),
        perf::sql_corpus(),
        reader::sql_corpus(),
        risk::sql_corpus(),
        runs::sql_corpus(),
        schedules::sql_corpus(),
        sightings::sql_corpus(),
        store::sql_corpus(),
        tx::sql_corpus(),
        webhooks::sql_corpus(),
        writer::sql_corpus(),
    ]
    .concat()
}
