//! The example that scripts/build-lake-shapes.py adds to the tumult-lake
//! crate of a release tag. It is not part of the workspace.
//!
//! It makes a lake with the open of that release and prints the shape of
//! the lake as SQL: the tables, the indexes, the sequences, the rows the
//! open wrote, and the views. Then it runs the seed file on the lake, to
//! prove that the seed rows fit the shape of that release.
//!
//! Usage: dump_shape <new lake file> <seed file>

use std::error::Error;
use std::path::Path;

use duckdb::Connection;

/// The first column of each row of `sql`, as text.
fn texts(conn: &Connection, sql: &str) -> Result<Vec<String>, Box<dyn Error>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |r| r.get::<_, Option<String>>(0))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?.unwrap_or_default());
    }
    Ok(out)
}

/// A statement as DuckDB keeps it, ended by a semicolon.
fn statement(sql: &str) -> String {
    let sql = sql.trim();
    if sql.ends_with(';') {
        sql.to_string()
    } else {
        format!("{sql};")
    }
}

/// `text` as a SQL string literal.
fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// An insert for each row of `table`, in a stable order. Each value is
/// written as its text, cast back to the type of its column.
fn inserts(conn: &Connection, table: &str) -> Result<Vec<String>, Box<dyn Error>> {
    let mut stmt = conn.prepare(
        "SELECT column_name, data_type FROM duckdb_columns() \
         WHERE schema_name = 'main' AND table_name = ? ORDER BY column_index",
    )?;
    let columns: Vec<(String, String)> = stmt
        .query_map([table], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let select: Vec<String> = columns
        .iter()
        .map(|(name, _)| format!("CAST(\"{name}\" AS VARCHAR)"))
        .collect();
    let names: Vec<&str> = columns.iter().map(|(name, _)| name.as_str()).collect();
    let sql = format!("SELECT {} FROM \"{table}\" ORDER BY ALL", select.join(", "));
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let mut values = Vec::new();
        for (i, (_, ty)) in columns.iter().enumerate() {
            let value: Option<String> = row.get(i)?;
            values.push(match value {
                None => "NULL".to_string(),
                Some(v) if ty == "VARCHAR" => quoted(&v),
                Some(v) => format!("CAST({} AS {ty})", quoted(&v)),
            });
        }
        out.push(format!(
            "INSERT INTO {table} ({}) VALUES ({});",
            names.join(", "),
            values.join(", ")
        ));
    }
    Ok(out)
}

/// The name of the view that `sql` makes: the word after `VIEW`.
fn view_name(sql: &str) -> String {
    let words: Vec<&str> = sql.split_whitespace().collect();
    let at = words.iter().position(|w| w.eq_ignore_ascii_case("VIEW"));
    at.and_then(|i| words.get(i + 1))
        .map(|name| name.trim_matches('"').to_string())
        .unwrap_or_default()
}

/// Whether `sql` reads the view `name`: the name as a whole word.
fn reads(sql: &str, name: &str) -> bool {
    sql.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .any(|word| word == name)
}

/// The views in an order DuckDB can make them in: each after the views
/// it reads, and by name where that leaves a choice.
fn in_order_of_use(mut views: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    while !views.is_empty() {
        let names: Vec<String> = views.iter().map(|sql| view_name(sql)).collect();
        // A view names itself only in its head, so the others it names
        // are the views it reads.
        let next = (0..views.len()).find(|&i| {
            !names
                .iter()
                .enumerate()
                .any(|(j, name)| j != i && reads(&views[i], name))
        });
        let next = next.expect("views that read each other in a loop");
        out.push(views.remove(next));
    }
    out
}

/// Print the shape of the lake behind `conn` as SQL.
fn print_shape(conn: &Connection) -> Result<(), Box<dyn Error>> {
    let version = texts(conn, "SELECT library_version FROM pragma_version()")?;
    println!("-- DuckDB {} wrote this catalog.", version.concat());
    // A file keeps the storage version it was made with, also when a later
    // DuckDB writes to it, so a test makes its file with the same version.
    let tags = texts(
        conn,
        "SELECT CAST(tags AS VARCHAR) FROM duckdb_databases() \
         WHERE database_name = current_database()",
    )?
    .concat();
    let storage = tags
        .strip_prefix("{storage_version=")
        .and_then(|rest| rest.strip_suffix("+}"))
        .ok_or_else(|| format!("no storage version in the tags {tags}"))?;
    println!("-- storage_compatibility_version = {storage}");
    let foreign = texts(
        conn,
        "SELECT table_name FROM duckdb_constraints() WHERE constraint_type = 'FOREIGN KEY'",
    )?;
    assert!(
        foreign.is_empty(),
        "foreign keys need another order: {foreign:?}"
    );
    let macros = texts(
        conn,
        "SELECT function_name FROM duckdb_functions() \
         WHERE NOT internal AND function_type IN ('macro', 'table_macro')",
    )?;
    assert!(macros.is_empty(), "macros are not printed: {macros:?}");
    let tables = texts(
        conn,
        "SELECT table_name FROM duckdb_tables() \
         WHERE NOT internal AND schema_name = 'main' ORDER BY table_name",
    )?;
    println!("\n-- Tables, by name.");
    for sql in texts(
        conn,
        "SELECT sql FROM duckdb_tables() \
         WHERE NOT internal AND schema_name = 'main' ORDER BY table_name",
    )? {
        println!("{}", statement(&sql));
    }
    println!("\n-- Indexes, by table and name.");
    for sql in texts(
        conn,
        "SELECT sql FROM duckdb_indexes() WHERE schema_name = 'main' \
         ORDER BY table_name, index_name",
    )? {
        println!("{}", statement(&sql));
    }
    println!("\n-- Sequences, by name.");
    for sql in texts(
        conn,
        "SELECT sql FROM duckdb_sequences() WHERE schema_name = 'main' ORDER BY sequence_name",
    )? {
        println!("{}", statement(&sql));
    }
    println!("\n-- The rows the open wrote, by table.");
    for table in &tables {
        for insert in inserts(conn, table)? {
            println!("{insert}");
        }
    }
    println!("\n-- Views, each after the views it reads.");
    for sql in in_order_of_use(texts(
        conn,
        "SELECT sql FROM duckdb_views() \
         WHERE NOT internal AND schema_name = 'main' ORDER BY view_name",
    )?) {
        println!("{}", statement(&sql));
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().collect();
    let [_, lake, seed] = args.as_slice() else {
        return Err("usage: dump_shape <new lake file> <seed file>".into());
    };
    let lake = Path::new(lake);
    if lake.exists() {
        return Err(format!("{} exists: the lake must be new", lake.display()).into());
    }
    tumult_lake::Store::open(lake)?;
    let conn = Connection::open(lake)?;
    print_shape(&conn)?;
    conn.execute_batch(&std::fs::read_to_string(seed)?)?;
    Ok(())
}
