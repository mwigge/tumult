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

/// Hard ceiling for any caller-supplied row cap.
///
/// The MCP layer clamps its own parameter, but the engine is the last line of
/// defense: a caller passing `u32::MAX` (or any other oversized value) gets
/// this ceiling, so a whole-graph `RETURN` can never stream an unbounded row
/// set into an agent transcript.
pub const MAX_ROW_CAP: usize = 10_000;

/// Maximum estimated expansion steps one query may perform.
///
/// The engine has no per-query compute meter of its own, so the budget is
/// enforced as a pre-execution estimate: each relationship pattern is one
/// expansion tier whose cost is the running result frontier times the average
/// node degree times the pattern's hop multiplier (see
/// [`estimate_expansion_steps`]). Queries whose estimate exceeds this budget
/// are rejected with [`CypherError::BudgetExceeded`] before the graph is
/// even built. Grafeo's own 30-second query timeout remains as the
/// wall-clock backstop for shapes the estimate cannot see (e.g.
/// comma-separated cartesian patterns).
pub const MAX_EXPANSION_STEPS: u64 = 1_000_000;

/// Hop count assumed for an unbounded variable-length pattern (`*` with no
/// upper bound) when estimating expansion work.
const UNBOUNDED_HOP_ASSUMPTION: u64 = 5;

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
    /// The query's estimated expansion work exceeds [`MAX_EXPANSION_STEPS`].
    /// Rejected before execution: an unbounded traversal over a whole-graph
    /// snapshot would burn CPU (and agent latency) without adding signal.
    #[error(
        "query rejected: estimated {estimated} expansion steps exceeds the evaluation budget \
         of {budget} — narrow the query (add node labels, bound variable-length paths, \
         or add LIMIT)"
    )]
    BudgetExceeded {
        /// The estimated expansion steps the query would perform.
        estimated: u64,
        /// The configured budget that was exceeded.
        budget: u64,
    },
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
/// See [`CypherError`]: mutation rejection, evaluation-budget rejection,
/// snapshot integrity failures, or engine parse/execution errors.
pub fn run_cypher(snapshot: &GraphSnapshot, query: &str) -> Result<CypherTable, CypherError> {
    run_cypher_capped(snapshot, query, DEFAULT_ROW_CAP)
}

/// Runs an openCypher query with an explicit row cap.
///
/// The cap is clamped to [`MAX_ROW_CAP`] inside the engine, so no caller —
/// whatever it passes — can stream an unbounded result set out of a
/// whole-graph snapshot. The query is also checked against
/// [`MAX_EXPANSION_STEPS`] before execution; see [`CypherError::BudgetExceeded`].
///
/// # Errors
///
/// See [`CypherError`]: mutation rejection, evaluation-budget rejection,
/// snapshot integrity failures, or engine parse/execution errors.
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
    // Engine-side resource limits: clamp the row cap even when the caller did
    // not, and refuse queries whose estimated expansion work blows the budget
    // — both before paying the graph-rebuild cost.
    let row_cap = row_cap.min(MAX_ROW_CAP);
    enforce_evaluation_budget(snapshot, query)?;
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

/// Rejects a query whose estimated expansion work exceeds
/// [`MAX_EXPANSION_STEPS`].
fn enforce_evaluation_budget(snapshot: &GraphSnapshot, query: &str) -> Result<(), CypherError> {
    let estimated = estimate_expansion_steps(snapshot, query);
    if estimated > MAX_EXPANSION_STEPS {
        return Err(CypherError::BudgetExceeded {
            estimated,
            budget: MAX_EXPANSION_STEPS,
        });
    }
    Ok(())
}

/// Estimates the expansion work a query performs against `snapshot`.
///
/// Every relationship pattern in the query is one expansion tier. A tier's
/// cost is the running frontier (rows produced so far, starting at the node
/// count — a bare `MATCH (n)` binds every node) times the average node degree
/// times the tier's hop multiplier; tiers sum. This is a heuristic upper
/// bound, not a cost model: it errs high on labeled patterns and low on
/// comma-separated cartesian patterns, with grafeo's query timeout as the
/// wall-clock backstop for the latter.
fn estimate_expansion_steps(snapshot: &GraphSnapshot, query: &str) -> u64 {
    let node_count = snapshot.nodes.len() as u64;
    let edge_count = snapshot.edges.len() as u64;
    let avg_degree = edge_count
        .saturating_mul(2)
        .checked_div(node_count)
        .unwrap_or(1)
        .max(1);
    let mut frontier = node_count.max(1);
    let mut steps = 0_u64;
    for hops in expansion_hops(query) {
        frontier = frontier.saturating_mul(avg_degree).saturating_mul(hops);
        steps = steps.saturating_add(frontier);
    }
    steps
}

/// Extracts the hop multiplier of every relationship pattern in the query.
///
/// Heuristic scan, not a parse (same philosophy as the mutation guard): a `-`
/// followed by `[` opens a bracketed relationship (`-[r*1..3]->`), `--` is an
/// anonymous dashed one (`MATCH (a)-->(b)`), and `-(` closes an anonymous bare
/// one (`MATCH (a)-(b)`, `MATCH (a)<-(b)`). Quoted strings and backtick
/// identifiers are skipped, so dashes and brackets inside string literals
/// never register as patterns; a spaced arithmetic minus (`n.x - 1`) is not
/// followed by `[`, `-`, or `(`, so it never counts either. A pathological
/// `x-(1)` subtraction would over-count — erring toward the budget, which is
/// the safe direction for a resource guard.
fn expansion_hops(query: &str) -> Vec<u64> {
    let chars: Vec<char> = query.chars().collect();
    let mut hops = Vec::new();
    let mut in_quote: Option<char> = None;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = in_quote {
            if c == '\\' && q != '`' {
                // Skip the escaped character so an escaped quote does not
                // terminate the string early.
                i += 1;
            } else if c == q {
                in_quote = None;
            }
        } else {
            match c {
                '\'' | '"' | '`' => in_quote = Some(c),
                '-' => match chars.get(i + 1) {
                    Some('[') => {
                        let (multiplier, end) = scan_relationship_bracket(&chars, i + 1);
                        hops.push(multiplier);
                        i = end;
                        // The dash closing this pattern (`-[...]->` or
                        // `-[...]-(b)`) belongs to it — skip it so it cannot
                        // register as a fresh anonymous relationship.
                        if chars.get(i) == Some(&'-') {
                            i += 1;
                        }
                        continue;
                    }
                    Some('-') => {
                        hops.push(1);
                        i += 1;
                    }
                    Some('(') => hops.push(1),
                    _ => {} // arithmetic minus or comparison — not a pattern
                },
                _ => {}
            }
        }
        i += 1;
    }
    hops
}

/// Scans a relationship bracket opened at `start` (the index of `[`) and
/// returns its hop multiplier plus the index just past the closing `]`.
/// Quotes are honored so a `]` inside a property-map string cannot close the
/// bracket early. Unbalanced brackets (invalid Cypher, which the engine will
/// reject anyway) scan to the end of input.
fn scan_relationship_bracket(chars: &[char], start: usize) -> (u64, usize) {
    let mut in_quote: Option<char> = None;
    let mut content = String::new();
    let mut i = start + 1;
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = in_quote {
            if c == '\\' && q != '`' {
                i += 1;
            } else if c == q {
                in_quote = None;
            }
        } else {
            match c {
                '\'' | '"' | '`' => in_quote = Some(c),
                ']' => return (hop_multiplier(&content), i + 1),
                _ => content.push(c),
            }
        }
        i += 1;
    }
    (hop_multiplier(&content), i)
}

/// Derives the hop multiplier from a relationship bracket's contents: `1` for
/// a fixed-length relationship, the upper bound for a variable-length spec
/// (`*2..5` → 5, `*3` → 3), [`UNBOUNDED_HOP_ASSUMPTION`] for an unbounded
/// `*`, and `max(m, UNBOUNDED_HOP_ASSUMPTION)` for an open-ended `*m..`.
fn hop_multiplier(bracket: &str) -> u64 {
    let Some(star) = bracket.find('*') else {
        return 1;
    };
    let spec = &bracket[star + 1..];
    let (min_spec, max_spec) = match spec.split_once("..") {
        Some((lo, hi)) => (lo, Some(hi)),
        None => (spec, None),
    };
    let leading_digits = |s: &str| -> Option<u64> {
        let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            None
        } else {
            digits.parse().ok()
        }
    };
    match max_spec {
        Some(hi) => leading_digits(hi).unwrap_or_else(
            // "*m.." — open-ended upper bound: assume the unbounded hop count,
            // but never less than the stated minimum.
            || {
                leading_digits(min_spec).map_or(UNBOUNDED_HOP_ASSUMPTION, |m| {
                    m.max(UNBOUNDED_HOP_ASSUMPTION)
                })
            },
        ),
        None => leading_digits(min_spec).unwrap_or(UNBOUNDED_HOP_ASSUMPTION),
    }
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
    use super::{
        estimate_expansion_steps, expansion_hops, find_mutation_token, hop_multiplier,
        MAX_EXPANSION_STEPS,
    };
    use crate::snapshot::{GraphSnapshot, SnapshotEdge, SnapshotNode};
    use serde_json::json;

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

    // ── expansion_hops / hop_multiplier ────────────────────────

    #[test]
    fn fixed_length_patterns_count_one_hop_each() {
        assert_eq!(expansion_hops("MATCH (a)-[:depends_on]->(b) RETURN b"), [1]);
        assert_eq!(expansion_hops("MATCH (a)-->(b) RETURN b"), [1]);
        assert_eq!(expansion_hops("MATCH (a)<--(b) RETURN b"), [1]);
        // Bare anonymous relationships (no bracket, no second dash) count too.
        assert_eq!(expansion_hops("MATCH (a)-(b) RETURN b"), [1]);
        assert_eq!(expansion_hops("MATCH (a)<-(b) RETURN b"), [1]);
        assert_eq!(
            expansion_hops("MATCH (a)-[:x]->(b)<-[:y]-(c)--(d) RETURN a"),
            [1, 1, 1]
        );
    }

    #[test]
    fn variable_length_patterns_use_their_upper_bound() {
        assert_eq!(expansion_hops("MATCH (a)-[*2..5]->(b) RETURN b"), [5]);
        assert_eq!(expansion_hops("MATCH (a)-[*3]->(b) RETURN b"), [3]);
        assert_eq!(expansion_hops("MATCH (a)-[:r*..4]->(b) RETURN b"), [4]);
        // Unbounded and open-ended specs fall back to the assumption (or the
        // stated minimum, whichever is larger).
        assert_eq!(expansion_hops("MATCH (a)-[*]->(b) RETURN b"), [5]);
        assert_eq!(expansion_hops("MATCH (a)-[*2..]->(b) RETURN b"), [5]);
        assert_eq!(expansion_hops("MATCH (a)-[*9..]->(b) RETURN b"), [9]);
    }

    #[test]
    fn patterns_inside_string_literals_do_not_count() {
        assert_eq!(
            expansion_hops("MATCH (n) WHERE n.x = 'a-[*9]->b' RETURN n"),
            Vec::<u64>::new()
        );
        assert_eq!(
            expansion_hops("MATCH (n) WHERE n.x = \"a-->b\" RETURN n"),
            Vec::<u64>::new()
        );
    }

    #[test]
    fn arithmetic_minus_is_not_a_relationship() {
        assert_eq!(
            expansion_hops("MATCH (n) WHERE n.x - 1 > -2 RETURN n"),
            Vec::<u64>::new()
        );
    }

    #[test]
    fn hop_multiplier_tolerates_property_maps() {
        assert_eq!(hop_multiplier(":depends_on"), 1);
        assert_eq!(hop_multiplier(":r*1..3 {run_id: 'x'}"), 3);
        assert_eq!(hop_multiplier("r*"), 5);
    }

    // ── estimate_expansion_steps ───────────────────────────────

    fn sized_snapshot(node_count: usize, edge_count: usize) -> GraphSnapshot {
        let nodes = (0..node_count)
            .map(|i| SnapshotNode {
                id: format!("n{i}"),
                kind: "service".into(),
                label: format!("Node {i}"),
                attrs: json!({}),
            })
            .collect::<Vec<_>>();
        let edges = (0..edge_count)
            .map(|i| SnapshotEdge {
                src: format!("n{}", i % node_count.max(1)),
                dst: format!("n{}", (i + 1) % node_count.max(1)),
                rel: "depends_on".into(),
                run_id: "run".into(),
                ts: 0,
                attrs: json!({}),
            })
            .collect::<Vec<_>>();
        GraphSnapshot { nodes, edges }
    }

    #[test]
    fn patternless_query_estimates_zero_steps() {
        let snap = sized_snapshot(100, 200);
        assert_eq!(estimate_expansion_steps(&snap, "MATCH (n) RETURN n"), 0);
    }

    #[test]
    fn chained_expansions_grow_the_estimate_multiplicatively() {
        let snap = sized_snapshot(100, 200); // average degree 4
        let one = estimate_expansion_steps(&snap, "MATCH (a)-->(b) RETURN b");
        let two = estimate_expansion_steps(&snap, "MATCH (a)-->(b)-->(c) RETURN c");
        assert_eq!(one, 400);
        assert!(two > one, "a second expansion tier must cost more: {two}");
    }

    #[test]
    fn large_snapshot_with_deep_chain_exceeds_budget() {
        let snap = sized_snapshot(2_000, 20_000); // average degree 20
        let estimated = estimate_expansion_steps(&snap, "MATCH (a)-->(b)-->(c)-->(d) RETURN d");
        assert!(estimated > MAX_EXPANSION_STEPS);
    }

    #[test]
    fn empty_snapshot_stays_within_budget() {
        let snap = GraphSnapshot::default();
        let estimated = estimate_expansion_steps(&snap, "MATCH (a)-[*]->(b)-[*]->(c) RETURN c");
        assert!(estimated <= MAX_EXPANSION_STEPS);
    }
}
