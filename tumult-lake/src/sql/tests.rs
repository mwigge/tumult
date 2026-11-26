//! The SQL corpus of the lake: every statement it runs, prepared on a
//! fresh lake and kept in `tests/golden/sql-corpus.sql` for the linter.

use std::path::Path;

use tumult_test_utils::sql_corpus::{assert_corpus, Statement};

/// Every statement of the lake, the schema first.
fn corpus() -> Vec<Statement> {
    [
        crate::schema::sql_corpus(),
        super::sql_corpus(),
        crate::backup::sql_corpus(),
        crate::dr::sql_corpus(),
        crate::duckdb_store::sql_corpus(),
        crate::entities::sql_corpus(),
        crate::ict::sql_corpus(),
        crate::lake::sql_corpus(),
        crate::manual::sql_corpus(),
        crate::perf::sql_corpus(),
        crate::scope::sql_corpus(),
        crate::sightings::sql_corpus(),
        crate::store::sql_corpus(),
    ]
    .concat()
}

#[test]
fn every_statement_of_the_sql_corpus_prepares_on_a_fresh_lake_and_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let lake = dir.path().join("kronika.duckdb");
    crate::Store::open(&lake).unwrap();
    let kept = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/sql-corpus.sql");

    assert_corpus(&kept, env!("CARGO_PKG_NAME"), &lake, &corpus());
}

/// The items of the file `file` of the SQL module: its consts and its
/// functions that are visible to the crate.
fn items_of(file: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(file).unwrap();
    text.lines()
        .filter_map(|line| {
            let rest = line
                .strip_prefix("pub(crate) const ")
                .or_else(|| line.strip_prefix("pub(crate) fn "))?;
            let end = rest.find([':', '('])?;
            Some(rest[..end].to_string())
        })
        .collect()
}

#[test]
fn the_sql_corpus_holds_every_statement_of_the_sql_module() {
    let names: Vec<String> = corpus().into_iter().map(|s| s.name).collect();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/sql");
    let mut missing = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let file = path.file_name().unwrap().to_string_lossy().into_owned();
        for item in items_of(&path) {
            let name = format!("tumult-lake/src/sql/{file}:{item}");
            let call = format!("{name}(");
            if !names.iter().any(|n| *n == name || n.starts_with(&call)) {
                missing.push(name);
            }
        }
    }

    assert!(missing.is_empty(), "not in the SQL corpus: {missing:?}");
}
