//! The SQL corpus of the API: every statement it runs, prepared on a fresh
//! lake and kept in `tests/golden/sql-corpus.sql` for the linter.

use std::path::Path;

use tumult_test_utils::sql_corpus::{assert_corpus, Statement};

/// Every statement of the API, the module of the consts first.
fn corpus() -> Vec<Statement> {
    [
        crate::sql::sql_corpus(),
        crate::approvals::sql_corpus(),
        crate::ask::sql_corpus(),
        crate::auth::scopes::sql_corpus(),
        crate::events::sql_corpus(),
        crate::forecast::sql_corpus(),
        crate::gamedays::sql_corpus(),
        crate::handlers::sql_corpus(),
        crate::runs::sql_corpus(),
        crate::sql_util::sql_corpus(),
    ]
    .concat()
}

#[test]
fn every_statement_of_the_sql_corpus_prepares_on_a_fresh_lake_and_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let lake = dir.path().join("kronika.duckdb");
    tumult_lake::Store::open(&lake).unwrap();
    let kept = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/sql-corpus.sql");

    assert_corpus(&kept, env!("CARGO_PKG_NAME"), &lake, &corpus());
}

/// The consts of the SQL module of the API.
fn consts_of(file: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(file).unwrap();
    text.lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("pub(crate) const ")?;
            let end = rest.find(':')?;
            Some(rest[..end].to_string())
        })
        .collect()
}

#[test]
fn the_sql_corpus_holds_every_statement_of_the_sql_module() {
    let names: Vec<String> = corpus().into_iter().map(|s| s.name).collect();
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/sql.rs");
    let missing: Vec<String> = consts_of(&file)
        .into_iter()
        .map(|item| format!("tumult-api/src/sql.rs:{item}"))
        .filter(|name| !names.contains(name))
        .collect();

    assert!(
        !consts_of(&file).is_empty(),
        "the SQL module holds statements"
    );
    assert!(missing.is_empty(), "not in the SQL corpus: {missing:?}");
}
