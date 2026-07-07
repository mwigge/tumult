//! Disposable query engine: snapshot -> embedded grafeo graph -> Cypher -> JSON table.
//!
//! Every [`run_cypher`] call builds a fresh in-memory `GrafeoDB`, so there is
//! no cache and no consistency problem — the engine's lifetime is exactly one
//! query. See the crate docs for why this trade was chosen over a write-path
//! mirror.

use std::collections::HashMap;

use grafeo::{GrafeoDB, NodeId, Session, Value as GrafeoValue};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::snapshot::GraphSnapshot;

/// Default maximum number of result rows returned to the caller.
///
/// Agents consume these results inside a token budget; an unbounded `MATCH`
/// over a large snapshot would blow that budget without adding signal, so we
/// truncate and say so ([`CypherTable::truncated`]) instead of failing.
pub const DEFAULT_ROW_CAP: usize = 500;

/// Clause keywords that mutate the graph in openCypher.
///
/// `DETACH DELETE` is covered by `DELETE`; `CREATE INDEX` / `DROP INDEX` by
/// `CREATE` / `DROP`.
const MUTATING_TOKENS: [&str; 6] = ["CREATE", "MERGE", "DELETE", "SET", "REMOVE", "DROP"];

/// Errors surfaced by [`run_cypher`].
#[derive(Debug, thiserror::Error)]
pub enum CypherError {
    /// The query contained a mutating clause keyword. Rejected before
    /// execution: writes against a disposable snapshot would silently vanish,
    /// which is worse for an agent than an explicit error.
    #[error(
        "query rejected: `{0}` is a mutating clause and tumult-cypher is read-only \
             (DuckDB is the source of truth; this graph is a disposable copy)"
    )]
    MutationRejected(String),
    /// An edge referenced a node id absent from the snapshot's node list.
    #[error("edge references node id `{0}` which is not in the snapshot")]
    UnknownNode(String),
    /// Two snapshot nodes shared the same id.
    #[error("duplicate node id `{0}` in snapshot")]
    DuplicateNode(String),
    /// Loading the snapshot into the embedded graph failed.
    #[error("failed to build in-memory graph: {0}")]
    Build(String),
    /// Grafeo failed to parse or execute the Cypher query.
    #[error("cypher execution failed: {0}")]
    Engine(String),
}

/// A materialized query result: column names plus JSON-typed rows.
///
/// JSON values (not grafeo's own value enum) so callers — MCP tool handlers,
/// agent transcripts — can serialize it without depending on the engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CypherTable {
    /// Column names from the query's `RETURN` clause.
    pub columns: Vec<String>,
    /// Result rows, in engine order, at most the requested row cap.
    pub rows: Vec<Vec<JsonValue>>,
    /// True when the engine produced more rows than the cap; the caller
    /// should narrow the query (e.g. add `LIMIT` or a `WHERE` filter).
    pub truncated: bool,
}

/// Runs an openCypher query against a snapshot with [`DEFAULT_ROW_CAP`].
///
/// # Errors
///
/// See [`CypherError`]: mutation rejection, snapshot integrity failures, or
/// engine parse/execution errors.
pub fn run_cypher(snapshot: &GraphSnapshot, query: &str) -> Result<CypherTable, CypherError> {
    run_cypher_capped(snapshot, query, DEFAULT_ROW_CAP)
}

/// Runs an openCypher query with an explicit row cap.
///
/// # Errors
///
/// See [`CypherError`]: mutation rejection, snapshot integrity failures, or
/// engine parse/execution errors.
pub fn run_cypher_capped(
    snapshot: &GraphSnapshot,
    query: &str,
    row_cap: usize,
) -> Result<CypherTable, CypherError> {
    // Guard first: a rejected query must never reach the engine, so even a
    // parser bug in grafeo cannot turn a write into a silent no-op.
    if let Some(token) = find_mutation_token(query) {
        return Err(CypherError::MutationRejected(token));
    }
    let db = GrafeoDB::new_in_memory();
    let session = db.session();
    build_graph(&session, snapshot)?;
    let result = session
        .execute_language(query, "cypher", None)
        .map_err(|e| CypherError::Engine(e.to_string()))?;
    let truncated = result.row_count() > row_cap;
    let rows = result
        .rows()
        .iter()
        .take(row_cap)
        .map(|row| row.iter().map(value_to_json).collect())
        .collect();
    Ok(CypherTable {
        columns: result.columns.clone(),
        rows,
        truncated,
    })
}

/// Loads the snapshot into a grafeo session via the direct (non-query) API.
///
/// Programmatic inserts rather than generated `INSERT` statements: no string
/// escaping surface, and property values keep their native types.
fn build_graph(session: &Session, snapshot: &GraphSnapshot) -> Result<(), CypherError> {
    let mut ids: HashMap<&str, NodeId> = HashMap::with_capacity(snapshot.nodes.len());
    for node in &snapshot.nodes {
        // Reserved keys win over same-named attrs so `n.id` always means the
        // ChaosGraph id, never a caller-supplied attribute.
        let mut props = flattened_attrs(&node.attrs, &["id", "label"]);
        props.push(("id".to_owned(), GrafeoValue::from(node.id.as_str())));
        props.push(("label".to_owned(), GrafeoValue::from(node.label.as_str())));
        let node_id = session
            .create_node_with_props(
                &[node.kind.as_str()],
                props.iter().map(|(k, v)| (k.as_str(), v.clone())),
            )
            .map_err(|e| CypherError::Build(e.to_string()))?;
        if ids.insert(node.id.as_str(), node_id).is_some() {
            return Err(CypherError::DuplicateNode(node.id.clone()));
        }
    }
    for edge in &snapshot.edges {
        // Missing endpoints are an error, not a skip: silently dropping an
        // edge would make traversal answers quietly wrong.
        let src = *ids
            .get(edge.src.as_str())
            .ok_or_else(|| CypherError::UnknownNode(edge.src.clone()))?;
        let dst = *ids
            .get(edge.dst.as_str())
            .ok_or_else(|| CypherError::UnknownNode(edge.dst.clone()))?;
        let mut props = flattened_attrs(&edge.attrs, &["run_id", "ts"]);
        props.push(("run_id".to_owned(), GrafeoValue::from(edge.run_id.as_str())));
        props.push(("ts".to_owned(), GrafeoValue::Int64(edge.ts)));
        session
            .create_edge_with_props(
                src,
                dst,
                edge.rel.as_str(),
                props.iter().map(|(k, v)| (k.as_str(), v.clone())),
            )
            .map_err(|e| CypherError::Build(e.to_string()))?;
    }
    Ok(())
}

/// Flattens top-level entries of a JSON `attrs` object into graph properties,
/// skipping `reserved` keys (those are set by the mapper and must win).
///
/// Non-object `attrs` (null, scalar) contribute nothing — `ChaosGraph` attrs
/// are objects by construction, so anything else carries no keyed data.
fn flattened_attrs(attrs: &JsonValue, reserved: &[&str]) -> Vec<(String, GrafeoValue)> {
    let JsonValue::Object(map) = attrs else {
        return Vec::new();
    };
    map.iter()
        .filter(|(key, _)| !reserved.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), json_to_prop(value)))
        .collect()
}

/// Converts one JSON attribute value to a grafeo property value.
///
/// Nested objects/arrays are stringified as JSON text rather than mapped to
/// grafeo's list/map types: `ChaosGraph` treats nested attrs as opaque payloads
/// and stringifying keeps the round-trip lossless and predictable.
fn json_to_prop(value: &JsonValue) -> GrafeoValue {
    match value {
        JsonValue::Null => GrafeoValue::Null,
        JsonValue::Bool(b) => GrafeoValue::Bool(*b),
        JsonValue::Number(n) => n.as_i64().map_or_else(
            // Non-i64 numbers (floats, huge u64s) go through f64; NaN cannot
            // occur here because serde_json numbers are always finite.
            || GrafeoValue::Float64(n.as_f64().unwrap_or(f64::NAN)),
            GrafeoValue::Int64,
        ),
        JsonValue::String(s) => GrafeoValue::from(s.as_str()),
        nested @ (JsonValue::Array(_) | JsonValue::Object(_)) => {
            GrafeoValue::from(nested.to_string())
        }
    }
}

/// Converts a grafeo result value to JSON for the caller-facing table.
///
/// Exotic variants (bytes, temporal types, paths) fall back to their debug
/// rendering: `ChaosGraph` properties only produce strings/ints/floats/bools,
/// so those variants can only appear via Cypher expressions and a readable
/// string beats an error.
fn value_to_json(value: &GrafeoValue) -> JsonValue {
    match value {
        GrafeoValue::Null => JsonValue::Null,
        GrafeoValue::Bool(b) => JsonValue::Bool(*b),
        GrafeoValue::Int64(i) => JsonValue::from(*i),
        GrafeoValue::Float64(f) => {
            serde_json::Number::from_f64(*f).map_or(JsonValue::Null, JsonValue::Number)
        }
        GrafeoValue::String(s) => JsonValue::String(s.to_string()),
        GrafeoValue::List(items) => JsonValue::Array(items.iter().map(value_to_json).collect()),
        GrafeoValue::Map(map) => JsonValue::Object(
            map.iter()
                .map(|(k, v)| (k.to_string(), value_to_json(v)))
                .collect(),
        ),
        other => JsonValue::String(format!("{other:?}")),
    }
}

/// Scans for mutating clause keywords outside quoted strings.
///
/// Conservative token scan, not a parse: any bare identifier matching a
/// mutating keyword is rejected, even where a real parser would allow it
/// (e.g. a property named `set` accessed as `n.set`, or a label `:CREATE`).
/// False positives are acceptable here; false negatives are not. Quoted
/// strings (`'…'`, `"…"`) honor backslash escapes; backtick identifiers are
/// skipped as opaque.
fn find_mutation_token(query: &str) -> Option<String> {
    fn flush(token: &mut String) -> Option<String> {
        let hit = MUTATING_TOKENS
            .contains(&token.to_ascii_uppercase().as_str())
            .then(|| token.clone());
        token.clear();
        hit
    }

    let mut chars = query.chars();
    let mut in_quote: Option<char> = None;
    let mut token = String::new();
    while let Some(c) = chars.next() {
        if let Some(q) = in_quote {
            if c == '\\' && q != '`' {
                // Consume the escaped character so an escaped quote does not
                // terminate the string early.
                chars.next();
            } else if c == q {
                in_quote = None;
            }
            continue;
        }
        match c {
            '\'' | '"' | '`' => {
                if let Some(hit) = flush(&mut token) {
                    return Some(hit);
                }
                in_quote = Some(c);
            }
            c if c.is_alphanumeric() || c == '_' => token.push(c),
            _ => {
                if let Some(hit) = flush(&mut token) {
                    return Some(hit);
                }
            }
        }
    }
    flush(&mut token)
}

#[cfg(test)]
mod tests {
    use super::find_mutation_token;

    #[test]
    fn quoted_keywords_are_not_flagged() {
        assert_eq!(
            find_mutation_token("MATCH (n) WHERE n.label = 'please CREATE this' RETURN n"),
            None
        );
        assert_eq!(
            find_mutation_token("MATCH (n) WHERE n.x = \"DELETE\" RETURN n"),
            None
        );
    }

    #[test]
    fn escaped_quote_does_not_end_string() {
        assert_eq!(
            find_mutation_token("MATCH (n) WHERE n.x = 'it\\'s a MERGE inside' RETURN n"),
            None
        );
    }

    #[test]
    fn keyword_substrings_are_not_flagged() {
        // `asset` contains `set`, `dropped` contains `drop`: tokenization
        // must prevent substring matches.
        assert_eq!(
            find_mutation_token("MATCH (n) WHERE n.asset = 1 AND n.dropped = 2 RETURN n"),
            None
        );
    }

    #[test]
    fn bare_keywords_are_flagged_case_insensitively() {
        assert_eq!(
            find_mutation_token("match (n) set n.x = 1 return n").as_deref(),
            Some("set")
        );
        assert_eq!(
            find_mutation_token("CREATE (n:service)").as_deref(),
            Some("CREATE")
        );
    }
}
