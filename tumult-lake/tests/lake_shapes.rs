//! The catalog of a fresh lake, kept as text: the tables, the columns in
//! their order with type, NULL and default, the keys, the indexes, the views,
//! the macros, the sequences and the rows of `schema_meta`. A change of the
//! schema shows as a change of `tests/golden/catalog.txt` in review.

use std::path::{Path, PathBuf};

use duckdb::Connection;
use tumult_lake::Store;

/// The kept catalog of a fresh lake.
fn kept_catalog() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/catalog.txt")
}

/// The first column of each row of `sql`, as text.
fn texts<P: duckdb::Params>(conn: &Connection, sql: &str, params: P) -> Vec<String> {
    let mut stmt = conn.prepare(sql).unwrap();
    let rows = stmt
        .query_map(params, |row| row.get::<_, Option<String>>(0))
        .unwrap();
    rows.map(|row| row.unwrap().unwrap_or_default()).collect()
}

/// The SQL of an index on one line, with one space between its words.
/// `DuckDB` writes it again from what it parsed, so the text does not
/// depend on the layout of the statement that made it.
fn one_line(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// One line for each view, by name, with its query normalised: parsed and
/// written again by `DuckDB` (`json_serialize_sql`). A lake file of an older
/// storage version keeps a view in an older form, which reads back with
/// other parentheses around its joins; the parsed query is the same.
fn view_lines(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare(
            "SELECT view_name, sql FROM duckdb_views() \
             WHERE NOT internal AND schema_name = 'main' ORDER BY view_name",
        )
        .unwrap();
    let views: Vec<(String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    views
        .into_iter()
        .map(|(view, sql)| {
            let head = format!("CREATE VIEW {view} AS ");
            let query = sql
                .strip_prefix(&head)
                .unwrap_or_else(|| panic!("the view {view} is not made by {head}: {sql}"))
                .trim_end_matches(';');
            let normalised: String = conn
                .query_row(
                    "SELECT json_deserialize_sql(json_serialize_sql(?))",
                    [query],
                    |row| row.get(0),
                )
                .unwrap();
            format!("view {view} | {}", one_line(&normalised))
        })
        .collect()
}

/// The tables of the lake, by name.
fn table_names(conn: &Connection) -> Vec<String> {
    texts(
        conn,
        "SELECT table_name FROM duckdb_tables() \
         WHERE NOT internal AND schema_name = 'main' ORDER BY table_name",
        [],
    )
}

/// One line for each column of `table`, in the order of the columns: its
/// place, name, type, whether it takes NULL and its default.
fn column_lines(conn: &Connection, table: &str) -> Vec<String> {
    texts(
        conn,
        "SELECT concat_ws(' ', 'column', table_name, column_index, column_name, data_type,
             CASE WHEN is_nullable THEN 'NULL' ELSE 'NOT NULL' END,
             'DEFAULT ' || column_default)
         FROM duckdb_columns()
         WHERE schema_name = 'main' AND table_name = ?
         ORDER BY column_index",
        [table],
    )
}

/// One line for each key or check of `table`. A NOT NULL shows on the
/// column.
fn constraint_lines(conn: &Connection, table: &str) -> Vec<String> {
    texts(
        conn,
        "SELECT concat_ws(' ', 'constraint', table_name, constraint_text)
         FROM duckdb_constraints()
         WHERE schema_name = 'main' AND table_name = ? AND constraint_type <> 'NOT NULL'
         ORDER BY constraint_text",
        [table],
    )
}

/// The catalog of the lake at `path` as text, in a stable order: the
/// tables with their columns and keys, the indexes, the views, the macros,
/// the sequences and the rows of `schema_meta`.
fn catalog(path: &Path) -> String {
    let conn = Connection::open(path).unwrap();
    let mut lines = vec![
        "# The catalog of a fresh lake, as tests/lake_shapes.rs reads it. Every".to_string(),
        "# shipped shape in tests/shapes upgrades to this text, but for the known".to_string(),
        "# lines that test names. Change the schema, then write this file again".to_string(),
        "# and read the difference:".to_string(),
        "#   TUMULT_BLESS=1 cargo test -p tumult-lake --test lake_shapes".to_string(),
    ];
    for table in table_names(&conn) {
        lines.push(format!("table {table}"));
        lines.extend(column_lines(&conn, &table));
        lines.extend(constraint_lines(&conn, &table));
    }
    let indexes = texts(
        &conn,
        "SELECT concat_ws(' ', 'index', index_name, 'on', table_name, '|', sql)
         FROM duckdb_indexes() WHERE schema_name = 'main' ORDER BY table_name, index_name",
        [],
    );
    lines.extend(indexes.iter().map(|index| one_line(index)));
    lines.extend(view_lines(&conn));
    lines.extend(texts(
        &conn,
        "SELECT concat_ws(' ', 'macro', function_name, macro_definition)
         FROM duckdb_functions()
         WHERE NOT internal AND function_type IN ('macro', 'table_macro')
         ORDER BY function_name",
        [],
    ));
    lines.extend(texts(
        &conn,
        "SELECT concat_ws(' ', 'sequence', sequence_name, 'start', start_value,
             'increment', increment_by)
         FROM duckdb_sequences() WHERE schema_name = 'main' ORDER BY sequence_name",
        [],
    ));
    lines.extend(texts(
        &conn,
        "SELECT concat_ws(' ', 'schema_meta', key, value) FROM schema_meta ORDER BY key",
        [],
    ));
    lines.push(String::new());
    lines.join("\n")
}

#[test]
fn the_catalog_of_a_fresh_lake_is_the_kept_catalog() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = dir.path().join("lake.duckdb");
    Store::open(&db).unwrap();

    tumult_test_utils::assert_kept(&kept_catalog(), &catalog(&db));
}
