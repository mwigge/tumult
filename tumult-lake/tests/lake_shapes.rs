//! The shape of a lake: the catalog of a fresh lake, kept as text, and the
//! shapes that releases shipped, each of which must upgrade to it.
//!
//! A shape is the lake a release made on its first open, kept in
//! `tests/shapes` as the SQL that makes it again. `tests/shapes/README.md`
//! says how each file was made from the release tag. A test loads a shape
//! and the rows of `tests/shapes/seed.sql` into a new file of the storage
//! version of that release, opens it with today's [`Store::open`], and asks
//! for two things: the catalog is the catalog of a fresh lake, and every row
//! is still there with its values.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use duckdb::{Config, Connection};
use tumult_lake::Store;

/// The folder of the shapes and their seed rows.
fn shapes() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/shapes")
}

/// The kept catalog of a fresh lake.
fn kept_catalog() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/catalog.txt")
}

/// The text of the file at `path`.
fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
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

/// The lines of `text` that `other` does not hold, in their order.
fn lines_not_in<'a>(text: &'a str, other: &str) -> Vec<&'a str> {
    let others: std::collections::HashSet<&str> = other.lines().collect();
    text.lines().filter(|line| !others.contains(line)).collect()
}

/// The rows of `table` that the comparison reads, as a condition. The open
/// writes two kinds of rows again by design: the version in `schema_meta`,
/// which it moves, and the article nodes of the citation register, which
/// it upserts.
fn compared(table: &str) -> &'static str {
    match table {
        "schema_meta" => "FALSE",
        "graph_nodes" => "kind <> 'compliance_article'",
        _ => "TRUE",
    }
}

/// The columns of each table of the lake, in their order.
fn columns(conn: &Connection) -> BTreeMap<String, Vec<String>> {
    table_names(conn)
        .into_iter()
        .map(|table| {
            let names = texts(
                conn,
                "SELECT column_name FROM duckdb_columns() \
                 WHERE schema_name = 'main' AND table_name = ? ORDER BY column_index",
                [&table],
            );
            (table, names)
        })
        .collect()
}

/// The rows of each table of `columns`, in those columns, as sorted JSON
/// texts. Only the rows that [`compared`] names.
fn rows(
    conn: &Connection,
    columns: &BTreeMap<String, Vec<String>>,
) -> BTreeMap<String, Vec<String>> {
    columns
        .iter()
        .map(|(table, names)| {
            let quoted: Vec<String> = names.iter().map(|name| format!("\"{name}\"")).collect();
            let mut found = texts(
                conn,
                &format!(
                    "SELECT CAST(row_to_json(t) AS VARCHAR) FROM (SELECT {} FROM \"{table}\" \
                     WHERE {}) AS t",
                    quoted.join(", "),
                    compared(table)
                ),
                [],
            );
            found.sort();
            (table.clone(), found)
        })
        .collect()
}

/// The tables that `seed.sql` writes rows into.
fn seeded_tables() -> Vec<String> {
    read(&shapes().join("seed.sql"))
        .lines()
        .filter_map(|line| line.strip_prefix("INSERT INTO "))
        .filter_map(|rest| rest.split_whitespace().next())
        .map(str::to_string)
        .collect()
}

/// The storage version that the lake of the shape `text` had, from its
/// line `-- storage_compatibility_version = v1.0.0`. A lake file keeps the
/// storage version it was made with, also when a later `DuckDB` writes to
/// it.
fn storage_version(text: &str) -> &str {
    text.lines()
        .find_map(|line| line.strip_prefix("-- storage_compatibility_version = "))
        .expect("a shape names its storage version")
}

/// The storage version of the lake behind `conn`, as `DuckDB` tags it.
fn storage_of(conn: &Connection) -> String {
    texts(
        conn,
        "SELECT CAST(tags AS VARCHAR) FROM duckdb_databases() \
         WHERE database_name = current_database()",
        [],
    )
    .concat()
}

/// A line of the catalog in which an upgraded lake differs from a fresh
/// one, for a reason the open of today does not change.
struct Known {
    /// The line in the upgraded lake.
    upgraded: &'static str,
    /// The line in a fresh lake.
    fresh: &'static str,
}

/// `DuckDB` 1.5, the engine of 2.19.0 to 2.22.0, keeps the default `FALSE`
/// of `approval_requests.break_glass` as a cast of the text 'f', and
/// `DuckDB` 2.0 reads it back so from an old lake file. `DuckDB` 2.0 keeps
/// it as `false` in a lake it makes. The value is the same and the text
/// differs. The open does not set the default again, so a lake first made
/// by a 2.x release keeps the old text.
const BREAK_GLASS_DEFAULT_OF_DUCKDB_1_5: Known = Known {
    upgraded: "column approval_requests 11 break_glass BOOLEAN NOT NULL \
               DEFAULT CAST('f' AS BOOLEAN)",
    fresh: "column approval_requests 11 break_glass BOOLEAN NOT NULL DEFAULT false",
};

/// Load the shape `name` of `tests/shapes` and the seed rows into a new
/// lake, open it with today's open, and assert two things. Its catalog is
/// the kept catalog of a fresh lake, but for the `known` lines, which must
/// still differ. Every row is still there, with the values it had.
fn assert_upgrades_to_a_fresh_lake(name: &str, known: &[Known]) {
    let dir = tempfile::TempDir::new().unwrap();
    let db = dir.path().join("lake.duckdb");
    let (shape_columns, before) = {
        let shape = read(&shapes().join(format!("{name}.sql")));
        let storage = storage_version(&shape);
        let config = Config::default()
            .with("storage_compatibility_version", storage)
            .unwrap();
        let conn = Connection::open_with_flags(&db, config).unwrap();
        assert_eq!(storage_of(&conn), format!("{{storage_version={storage}+}}"));
        conn.execute_batch(&shape).unwrap();
        conn.execute_batch(&read(&shapes().join("seed.sql")))
            .unwrap();
        let shape_columns = columns(&conn);
        let before = rows(&conn, &shape_columns);
        (shape_columns, before)
    };
    for table in seeded_tables() {
        assert!(
            before.get(&table).is_some_and(|found| !found.is_empty()),
            "the seed rows of {table} are not read"
        );
    }

    Store::open(&db).unwrap();

    let after = rows(&Connection::open(&db).unwrap(), &shape_columns);
    let changed: Vec<String> = before
        .iter()
        .filter(|(table, found)| after.get(*table) != Some(found))
        .map(|(table, found)| {
            format!(
                "{table}\n  before: {found:?}\n  after:  {:?}",
                after.get(table)
            )
        })
        .collect();
    assert!(
        changed.is_empty(),
        "the rows of the shape {name} changed on the upgrade\n{}",
        changed.join("\n")
    );
    let upgraded = catalog(&db);
    for difference in known {
        assert!(
            upgraded.lines().any(|line| line == difference.upgraded),
            "the shape {name} no longer has the known difference {}",
            difference.upgraded
        );
    }
    let mut lines: Vec<&str> = upgraded
        .lines()
        .map(|line| {
            known
                .iter()
                .find(|difference| difference.upgraded == line)
                .map_or(line, |difference| difference.fresh)
        })
        .collect();
    lines.push("");
    let upgraded = lines.join("\n");
    let fresh = read(&kept_catalog());
    assert!(
        upgraded == fresh,
        "the shape {name} upgrades to another catalog than a fresh lake has\n\
         only in the upgraded lake:\n{}\nonly in a fresh lake:\n{}",
        lines_not_in(&upgraded, &fresh).join("\n"),
        lines_not_in(&fresh, &upgraded).join("\n"),
    );
}

#[test]
fn a_lake_made_by_2_19_0_at_v8_upgrades_to_a_fresh_lake_and_keeps_its_rows() {
    assert_upgrades_to_a_fresh_lake("v8-2.19.0", &[BREAK_GLASS_DEFAULT_OF_DUCKDB_1_5]);
}

#[test]
fn a_lake_made_by_2_20_0_at_v9_upgrades_to_a_fresh_lake_and_keeps_its_rows() {
    assert_upgrades_to_a_fresh_lake("v9-2.20.0", &[BREAK_GLASS_DEFAULT_OF_DUCKDB_1_5]);
}

#[test]
fn a_lake_made_by_2_21_0_at_v13_upgrades_to_a_fresh_lake_and_keeps_its_rows() {
    assert_upgrades_to_a_fresh_lake("v13-2.21.0", &[BREAK_GLASS_DEFAULT_OF_DUCKDB_1_5]);
}

#[test]
fn a_lake_made_by_2_22_0_at_v15_upgrades_to_a_fresh_lake_and_keeps_its_rows() {
    assert_upgrades_to_a_fresh_lake("v15-2.22.0", &[BREAK_GLASS_DEFAULT_OF_DUCKDB_1_5]);
}

#[test]
fn a_lake_made_by_3_0_0_at_v21_upgrades_to_a_fresh_lake_and_keeps_its_rows() {
    assert_upgrades_to_a_fresh_lake("v21-3.0.0", &[]);
}

#[test]
fn a_lake_made_by_3_0_4_at_v26_upgrades_to_a_fresh_lake_and_keeps_its_rows() {
    assert_upgrades_to_a_fresh_lake("v26-3.0.4", &[]);
}
