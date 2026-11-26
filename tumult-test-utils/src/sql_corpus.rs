//! A SQL corpus: every statement a crate runs, in one kept file.
//!
//! A test of the crate lists its statements with [`sql_statements!`]: each
//! const as it is, and each function that builds SQL with sample
//! arguments. [`assert_corpus`] prepares every statement on a fresh lake,
//! so a wrong table or column fails with the name of its statement. Then
//! it compares the text with the kept file, which CI lints with sqlfluff.
//! When a statement changes on purpose, `TUMULT_BLESS=1` writes the file
//! again.

use std::path::Path;

use crate::assert_kept;

/// One statement of a corpus: where the code keeps it and its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    /// The source of the statement, `file:item`.
    pub name: String,
    /// The text, as the code runs it.
    pub sql: String,
    /// Why the statement is not prepared on a fresh lake, when it is not.
    pub unprepared: Option<&'static str>,
    /// The statement that runs before this one is prepared, in the same
    /// transaction, as `(name, sql)`: the one that makes the temporary
    /// tables it reads.
    pub after: Option<(String, String)>,
    /// Why the linter does not read the statement, when it does not. The
    /// kept file then holds its text as comment lines: sqlfluff stops
    /// reading a file at the first statement it cannot parse.
    pub unlinted: Option<&'static str>,
}

impl Statement {
    /// The statement `sql`, kept under `name`, written on one line.
    pub fn new(name: impl Into<String>, sql: impl AsRef<str>) -> Self {
        let name: String = name.into();
        Self {
            name: name.split_whitespace().collect::<Vec<_>>().join(" "),
            sql: sql.as_ref().to_string(),
            unprepared: None,
            after: None,
            unlinted: None,
        }
    }

    /// The same statement, left out of the prepare for `reason`. The
    /// reason is written above it in the kept file.
    #[must_use]
    pub fn unprepared(self, reason: &'static str) -> Self {
        Self {
            unprepared: Some(reason),
            ..self
        }
    }

    /// The same statement, kept out of the linter for `reason`. The
    /// reason is written above it in the kept file, and its text as
    /// comment lines.
    #[must_use]
    pub fn unlinted(self, reason: &'static str) -> Self {
        Self {
            unlinted: Some(reason),
            ..self
        }
    }

    /// The same statement, prepared after `setup` runs in the same
    /// transaction. The name of `setup` is written above it in the kept
    /// file.
    #[must_use]
    pub fn after(self, setup: &Statement) -> Self {
        Self {
            after: Some((setup.name.clone(), setup.sql.clone())),
            ..self
        }
    }

    /// The text as the kept file holds it: without the white space around
    /// it, ended by one semicolon, and as comment lines when it is not
    /// linted.
    fn text(&self) -> String {
        let text = format!("{};", self.sql.trim().trim_end_matches(';').trim_end());
        if self.unlinted.is_none() {
            return text;
        }
        let lines: Vec<String> = text.lines().map(|line| format!("-- {line}")).collect();
        lines.join("\n")
    }
}

/// A statement of a corpus, named `file:item` after the file this is
/// written in and the expression that gives its text: a const, or a call
/// of a function with its sample arguments.
#[macro_export]
macro_rules! sql_statement {
    ($item:expr) => {
        $crate::sql_corpus::Statement::new(concat!(file!(), ":", stringify!($item)), $item)
    };
}

/// The statements of a corpus, each named as [`sql_statement!`] names one.
#[macro_export]
macro_rules! sql_statements {
    ($($item:expr),* $(,)?) => {
        vec![$($crate::sql_statement!($item)),*]
    };
}

/// A statement of a corpus that holds the SQL fragment `$item` in the
/// statement `$host`, at its `{}`. It is named after the file and the
/// fragment, as [`sql_statements!`] names a statement.
#[macro_export]
macro_rules! sql_fragment {
    ($host:literal, $item:expr) => {
        $crate::sql_corpus::Statement::new(
            concat!(file!(), ":", stringify!($item)),
            format!($host, $item),
        )
    };
}

/// The text of the kept file of `crate_name`: a header, then each
/// statement under its name, ended by a semicolon.
#[must_use]
pub fn render(crate_name: &str, statements: &[Statement]) -> String {
    let mut lines = vec![format!(
        "-- The SQL corpus of {crate_name}: every statement the crate runs, as its
-- code renders it, each under the file and the item that hold it. A test
-- of the crate prepares each statement on a fresh lake and keeps this
-- file; CI lints it with sqlfluff. Change the SQL in the code, then write
-- this file again and read the difference:
--   TUMULT_BLESS=1 cargo test -p {crate_name} -- sql_corpus"
    )];
    for statement in statements {
        lines.push(String::new());
        lines.push(format!("-- {}", statement.name));
        if let Some(reason) = statement.unprepared {
            lines.push(format!("-- Not prepared: {reason}."));
        }
        if let Some((setup, _)) = &statement.after {
            lines.push(format!("-- Prepared after {setup}."));
        }
        if let Some(reason) = statement.unlinted {
            lines.push(format!("-- Not linted: {reason}."));
        }
        lines.push(statement.text());
    }
    lines.join("\n") + "\n"
}

/// Whether `sql` is a batch that opens its own transaction, as a rebuild
/// of the schema does: its first statement runs when it is prepared.
fn opens_its_own_transaction(sql: &str) -> bool {
    let sql = sql.trim();
    let begins = sql
        .get(..5)
        .is_some_and(|word| word.eq_ignore_ascii_case("BEGIN"));
    begins && sql.trim_end_matches(';').contains(';')
}

/// Prepare `statement` on `conn` in a transaction, then roll it back.
fn prepare(conn: &duckdb::Connection, statement: &Statement) -> Result<(), String> {
    let own = statement.after.is_none() && opens_its_own_transaction(&statement.sql);
    if !own {
        conn.execute_batch("BEGIN TRANSACTION")
            .map_err(|e| e.to_string())?;
    }
    let prepared = setup(conn, statement).and_then(|()| {
        conn.prepare(&statement.sql)
            .map(drop)
            .map_err(|e| e.to_string())
    });
    let rolled_back = conn
        .execute_batch("ROLLBACK")
        .map_err(|e| format!("no transaction to roll back after it: {e}"));
    prepared.and(rolled_back)
}

/// Run the setup of `statement`, if it has one.
fn setup(conn: &duckdb::Connection, statement: &Statement) -> Result<(), String> {
    let Some((name, sql)) = &statement.after else {
        return Ok(());
    };
    conn.execute_batch(sql)
        .map_err(|e| format!("after {name}: {e}"))
}

/// The statements that do not prepare on the lake at `lake`, each as
/// `name: error`.
///
/// Each one is prepared in a transaction that is then rolled back. A batch
/// runs all but its last statement when it is prepared, and the roll back
/// leaves the lake as it was. A batch that opens its own transaction, as a
/// rebuild of the schema does, is prepared without one around it.
///
/// # Panics
///
/// When the lake does not open.
#[must_use]
pub fn unprepared_on(lake: &Path, statements: &[Statement]) -> Vec<String> {
    let conn = duckdb::Connection::open(lake).unwrap_or_else(|e| panic!("{}: {e}", lake.display()));
    statements
        .iter()
        .filter(|statement| statement.unprepared.is_none())
        .filter_map(|statement| {
            prepare(&conn, statement)
                .err()
                .map(|e| format!("{}: {e}", statement.name))
        })
        .collect()
}

/// Assert that every statement prepares on the fresh lake at `lake`, that
/// no two have the same name, and that their text is the kept file
/// `kept_at`.
///
/// # Panics
///
/// When a statement does not prepare, when a name is used twice, and when
/// the text is not the kept one.
pub fn assert_corpus(kept_at: &Path, crate_name: &str, lake: &Path, statements: &[Statement]) {
    let failures = unprepared_on(lake, statements);
    assert!(
        failures.is_empty(),
        "statements that do not prepare on a fresh lake:\n  {}",
        failures.join("\n  ")
    );
    let mut names: Vec<&str> = statements.iter().map(|s| s.name.as_str()).collect();
    names.sort_unstable();
    let twice: Vec<&str> = names
        .windows(2)
        .filter(|pair| pair[0] == pair[1])
        .map(|pair| pair[0])
        .collect();
    assert!(twice.is_empty(), "statements named twice: {twice:?}");
    assert_kept(kept_at, &render(crate_name, statements));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EnvGuard, BLESS};

    /// The message of the panic `check` ends with.
    fn panic_of(check: impl FnOnce() + std::panic::UnwindSafe) -> String {
        let panic = std::panic::catch_unwind(check).expect_err("a panic");
        panic.downcast_ref::<String>().expect("a message").clone()
    }

    /// A lake with the one table `t (a INTEGER)`.
    fn lake() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("lake.duckdb");
        let conn = duckdb::Connection::open(&path).expect("lake");
        conn.execute_batch("CREATE TABLE t (a INTEGER)")
            .expect("table");
        (dir, path)
    }

    /// The tables of the lake at `path`.
    fn tables(path: &Path) -> Vec<String> {
        let conn = duckdb::Connection::open(path).expect("lake");
        let mut stmt = conn
            .prepare("SELECT table_name FROM duckdb_tables() ORDER BY 1")
            .expect("query");
        stmt.query_map([], |row| row.get(0))
            .expect("rows")
            .collect::<Result<_, _>>()
            .expect("names")
    }

    #[test]
    fn the_macro_names_a_statement_after_its_file_and_its_expression() {
        const ONE: &str = "SELECT 1";
        let twice = |n: i32| format!("SELECT {n} * 2");

        let statements = sql_statements![ONE, twice(3)];

        assert_eq!(
            statements,
            [
                Statement::new("tumult-test-utils/src/sql_corpus.rs:ONE", "SELECT 1"),
                Statement::new(
                    "tumult-test-utils/src/sql_corpus.rs:twice(3)",
                    "SELECT 3 * 2"
                ),
            ]
        );
    }

    #[test]
    fn the_fragment_macro_names_a_statement_after_its_fragment() {
        let rows = |table: &str| format!("count(*) FROM {table}");

        let statement = crate::sql_fragment!("SELECT {} AS n", rows("t"));

        assert_eq!(
            statement,
            Statement::new(
                "tumult-test-utils/src/sql_corpus.rs:rows(\"t\")",
                "SELECT count(*) FROM t AS n"
            )
        );
    }

    #[test]
    fn a_name_is_written_on_one_line() {
        let statement = Statement::new("a.rs:rows(&Query {\n    table: \"t\",\n})", "SELECT 1");

        assert_eq!(statement.name, "a.rs:rows(&Query { table: \"t\", })");
    }

    #[test]
    fn the_kept_text_names_each_statement_and_ends_it_once() {
        let statements = [
            Statement::new("a.rs:ONE", "\nSELECT 1\n"),
            Statement::new("a.rs:TWO", "SELECT 2;\n"),
            Statement::new("a.rs:OLD", "SELECT 3").unprepared("needs an old lake"),
            Statement::new("a.rs:READ", "SELECT a FROM u").after(&Statement::new(
                "a.rs:STAGE",
                "CREATE TEMP TABLE u (a INTEGER)",
            )),
            Statement::new("a.rs:ATTACH", "ATTACH 'a.duckdb' AS a;\nDETACH a;")
                .unlinted("the linter has no grammar for it"),
        ];

        let text = render("tumult-x", &statements);

        assert!(
            text.ends_with(
                "\n-- a.rs:ONE\nSELECT 1;\n\n-- a.rs:TWO\nSELECT 2;\n\n\
                 -- a.rs:OLD\n-- Not prepared: needs an old lake.\nSELECT 3;\n\n\
                 -- a.rs:READ\n-- Prepared after a.rs:STAGE.\nSELECT a FROM u;\n\n\
                 -- a.rs:ATTACH\n-- Not linted: the linter has no grammar for it.\n\
                 -- ATTACH 'a.duckdb' AS a;\n-- DETACH a;\n"
            ),
            "{text}"
        );
        assert!(text.starts_with("-- "), "{text}");
        assert!(
            text.contains("TUMULT_BLESS=1 cargo test -p tumult-x -- sql_corpus"),
            "{text}"
        );
    }

    #[test]
    fn a_statement_that_does_not_prepare_is_named_with_its_error() {
        let (_dir, path) = lake();
        let statements = [
            Statement::new("a.rs:GOOD", "SELECT a FROM t WHERE a = ?"),
            Statement::new("a.rs:BAD", "SELECT b FROM t"),
            Statement::new("a.rs:OLD", "SELECT c FROM t").unprepared("needs an old lake"),
        ];

        let failures = unprepared_on(&path, &statements);

        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(failures[0].starts_with("a.rs:BAD: "), "{failures:?}");
        assert!(failures[0].contains("\"b\""), "{failures:?}");
    }

    #[test]
    fn a_statement_is_prepared_after_its_setup_and_both_are_rolled_back() {
        let (_dir, path) = lake();
        let stage = Statement::new("a.rs:STAGE", "CREATE TEMP TABLE u (b INTEGER);");
        let statements = [
            Statement::new("a.rs:READ", "SELECT b FROM u").after(&stage),
            Statement::new("a.rs:LOST", "SELECT b FROM u"),
            Statement::new("a.rs:BROKEN", "SELECT 1")
                .after(&Statement::new("a.rs:NO_STAGE", "SELECT c FROM t")),
        ];

        let failures = unprepared_on(&path, &statements);

        assert_eq!(failures.len(), 2, "{failures:?}");
        assert!(failures[0].starts_with("a.rs:LOST: "), "{failures:?}");
        assert!(
            failures[1].starts_with("a.rs:BROKEN: after a.rs:NO_STAGE: "),
            "{failures:?}"
        );
    }

    #[test]
    fn a_batch_is_prepared_in_a_transaction_that_is_rolled_back() {
        let (_dir, path) = lake();
        let statements = [Statement::new(
            "a.rs:BATCH",
            "DROP TABLE t;\nCREATE TABLE u (b INTEGER);\nSELECT b FROM u;",
        )];

        let failures = unprepared_on(&path, &statements);

        assert_eq!(failures, Vec::<String>::new());
        assert_eq!(tables(&path), ["t"]);
    }

    #[test]
    fn a_batch_with_its_own_transaction_is_rolled_back_too() {
        let (_dir, path) = lake();
        let statements = [
            Statement::new(
                "a.rs:REBUILD",
                "BEGIN TRANSACTION;\nDROP TABLE t;\nCREATE TABLE u (b INTEGER);\nCOMMIT;",
            ),
            Statement::new("a.rs:BEGIN", "BEGIN TRANSACTION"),
            Statement::new("a.rs:COMMIT", "COMMIT"),
        ];

        let failures = unprepared_on(&path, &statements);

        assert_eq!(failures, Vec::<String>::new());
        assert_eq!(tables(&path), ["t"]);
    }

    #[test]
    fn a_batch_that_commits_on_its_way_fails() {
        let (_dir, path) = lake();
        let statements = [Statement::new(
            "a.rs:COMMITS",
            "BEGIN TRANSACTION;\nCREATE TABLE u (b INTEGER);\nCOMMIT;\nSELECT 1;",
        )];

        let failures = unprepared_on(&path, &statements);

        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(failures[0].starts_with("a.rs:COMMITS: "), "{failures:?}");
    }

    #[test]
    fn the_corpus_fails_on_a_statement_that_does_not_prepare() {
        let mut env = EnvGuard::lock();
        env.set(BLESS, "1");
        let (dir, path) = lake();
        let kept = dir.path().join("sql-corpus.sql");
        let statements = [Statement::new("a.rs:BAD", "SELECT b FROM t")];

        let message = panic_of(|| assert_corpus(&kept, "tumult-x", &path, &statements));

        assert!(message.contains("a.rs:BAD: "), "{message}");
        assert!(
            !kept.exists(),
            "nothing is kept before every statement prepares"
        );
    }

    #[test]
    fn the_corpus_fails_on_a_name_used_twice() {
        let mut env = EnvGuard::lock();
        env.set(BLESS, "1");
        let (dir, path) = lake();
        let kept = dir.path().join("sql-corpus.sql");
        let statements = [
            Statement::new("a.rs:ONE", "SELECT 1"),
            Statement::new("a.rs:ONE", "SELECT 2"),
        ];

        let message = panic_of(|| assert_corpus(&kept, "tumult-x", &path, &statements));

        assert!(message.contains("a.rs:ONE"), "{message}");
    }

    #[test]
    fn the_corpus_fails_on_a_text_that_is_not_the_kept_one() {
        let mut env = EnvGuard::lock();
        env.remove(BLESS);
        let (dir, path) = lake();
        let kept = dir.path().join("sql-corpus.sql");
        let statements = [Statement::new("a.rs:ONE", "SELECT a FROM t")];
        std::fs::write(&kept, render("tumult-x", &statements)).expect("kept file");
        let changed = [Statement::new("a.rs:ONE", "SELECT a + 1 FROM t")];

        assert_corpus(&kept, "tumult-x", &path, &statements);
        let message = panic_of(|| assert_corpus(&kept, "tumult-x", &path, &changed));

        assert!(message.contains("now:  SELECT a + 1 FROM t;"), "{message}");
    }
}
