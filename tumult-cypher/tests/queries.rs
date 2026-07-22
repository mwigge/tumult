//! End-to-end tests: fixture snapshot -> grafeo -> Cypher -> JSON table.
//!
//! No `DuckDB` anywhere: the snapshot is hand-built, exactly as a caller would
//! build it from `ChaosGraph` rows.

use serde_json::{json, Value as JsonValue};
use tumult_cypher::{
    run_cypher, run_cypher_capped, CypherError, GraphSnapshot, SnapshotEdge, SnapshotNode,
};

fn node(id: &str, kind: &str, label: &str, attrs: JsonValue) -> SnapshotNode {
    SnapshotNode {
        id: id.to_owned(),
        kind: kind.to_owned(),
        label: label.to_owned(),
        attrs,
    }
}

fn edge(src: &str, rel: &str, dst: &str, run_id: &str, ts: i64) -> SnapshotEdge {
    SnapshotEdge {
        src: src.to_owned(),
        rel: rel.to_owned(),
        dst: dst.to_owned(),
        run_id: run_id.to_owned(),
        ts,
        attrs: json!({}),
    }
}

/// Two services (checkout `depends_on` payments), one experiment that injects a
/// latency fault observed on payments, evidenced by a journal, plus a
/// deviation caused by the fault.
fn fixture() -> GraphSnapshot {
    GraphSnapshot {
        nodes: vec![
            node(
                "svc:checkout",
                "service",
                "Checkout",
                json!({"tier": "critical", "team": "storefront"}),
            ),
            node(
                "svc:payments",
                "service",
                "Payments",
                json!({"tier": "critical"}),
            ),
            node(
                "exp:latency-01",
                "experiment",
                "Payments latency GameDay",
                json!({"hypothesis": "checkout degrades gracefully"}),
            ),
            node(
                "fault:lat-250ms",
                "fault",
                "250ms latency",
                json!({"latency_ms": 250}),
            ),
            node("jrn:run-42", "journal", "Run 42 journal", json!({})),
            node(
                "dev:timeout-spike",
                "deviation",
                "Checkout timeout spike",
                json!({"severity": "high"}),
            ),
        ],
        edges: vec![
            edge(
                "svc:checkout",
                "depends_on",
                "svc:payments",
                "run-42",
                1_000,
            ),
            edge(
                "exp:latency-01",
                "injects",
                "fault:lat-250ms",
                "run-42",
                1_001,
            ),
            edge("exp:latency-01", "targets", "svc:payments", "run-42", 1_002),
            edge("exp:latency-01", "evidences", "jrn:run-42", "run-42", 1_003),
            edge(
                "fault:lat-250ms",
                "observed_on",
                "svc:payments",
                "run-42",
                1_004,
            ),
            edge(
                "dev:timeout-spike",
                "caused_by",
                "fault:lat-250ms",
                "run-42",
                1_005,
            ),
        ],
    }
}

fn string_column(table: &tumult_cypher::CypherTable, col: usize) -> Vec<String> {
    table
        .rows
        .iter()
        .map(|row| row[col].as_str().unwrap_or_default().to_owned())
        .collect()
}

#[test]
fn match_services_by_label() {
    let table = run_cypher(&fixture(), "MATCH (s:service) RETURN s.id ORDER BY s.id").unwrap();
    assert_eq!(table.columns.len(), 1);
    assert!(!table.truncated);
    assert_eq!(
        string_column(&table, 0),
        vec!["svc:checkout", "svc:payments"]
    );
}

#[test]
fn multi_hop_experiments_hitting_dependencies() {
    // Experiments that injected faults observed on services that some other
    // service depends_on — the canonical "blast radius" question.
    let query = "MATCH (e:experiment)-[:injects]->(f:fault)-[:observed_on]->(s:service)\
                 <-[:depends_on]-(dependent:service) \
                 RETURN e.id, s.id, dependent.id";
    let table = run_cypher(&fixture(), query).unwrap();
    assert_eq!(table.rows.len(), 1);
    assert_eq!(string_column(&table, 0), vec!["exp:latency-01"]);
    assert_eq!(string_column(&table, 1), vec!["svc:payments"]);
    assert_eq!(string_column(&table, 2), vec!["svc:checkout"]);
}

#[test]
fn mutation_queries_are_rejected() {
    let snapshot = fixture();
    for query in [
        "CREATE (n:service {id: 'svc:rogue'})",
        "MERGE (n:service {id: 'svc:rogue'}) RETURN n",
        // SET embedded mid-query, after a legitimate MATCH.
        "MATCH (s:service) SET s.tier = 'low' RETURN s.id",
    ] {
        let err = run_cypher(&snapshot, query).unwrap_err();
        assert!(
            matches!(err, CypherError::MutationRejected(_)),
            "expected MutationRejected for {query:?}, got {err:?}"
        );
    }
    // But the same keywords inside string literals must pass the guard (the
    // engine then treats them as plain data).
    let table = run_cypher(
        &snapshot,
        "MATCH (s:service) WHERE s.label = 'CREATE MERGE SET' RETURN s.id",
    )
    .unwrap();
    assert!(table.rows.is_empty());
}

#[test]
fn row_cap_truncates_and_flags() {
    let mut snapshot = GraphSnapshot::default();
    for i in 0..10 {
        snapshot.nodes.push(node(
            &format!("svc:{i:02}"),
            "service",
            &format!("Service {i}"),
            json!({}),
        ));
    }
    let table =
        run_cypher_capped(&snapshot, "MATCH (s:service) RETURN s.id ORDER BY s.id", 3).unwrap();
    assert!(table.truncated);
    assert_eq!(table.rows.len(), 3);

    // Under the cap: full result, not flagged.
    let table = run_cypher(&snapshot, "MATCH (s:service) RETURN s.id").unwrap();
    assert!(!table.truncated);
    assert_eq!(table.rows.len(), 10);
}

#[test]
fn edge_properties_are_reachable() {
    let query = "MATCH (:experiment)-[r:injects]->(:fault) RETURN r.run_id, r.ts";
    let table = run_cypher(&fixture(), query).unwrap();
    assert_eq!(table.rows.len(), 1);
    assert_eq!(table.rows[0][0], json!("run-42"));
    assert_eq!(table.rows[0][1], json!(1_001));
}

#[test]
fn node_attrs_and_deviation_chain() {
    // Flattened attrs are queryable, and the caused_by chain resolves back to
    // the experiment that injected the offending fault.
    let query = "MATCH (d:deviation)-[:caused_by]->(f:fault)<-[:injects]-(e:experiment) \
                 WHERE d.severity = 'high' RETURN e.id, f.latency_ms";
    let table = run_cypher(&fixture(), query).unwrap();
    assert_eq!(table.rows.len(), 1);
    assert_eq!(table.rows[0][0], json!("exp:latency-01"));
    assert_eq!(table.rows[0][1], json!(250));
}

#[test]
fn edge_with_unknown_endpoint_is_an_error() {
    let mut snapshot = fixture();
    snapshot.edges.push(edge(
        "svc:ghost",
        "depends_on",
        "svc:payments",
        "run-42",
        2_000,
    ));
    let err = run_cypher(&snapshot, "MATCH (s:service) RETURN s.id").unwrap_err();
    assert!(matches!(err, CypherError::UnknownNode(id) if id == "svc:ghost"));
}

/// A caller passing an oversized row cap gets the engine-side ceiling, not
/// an unbounded result stream — the MCP layer's own clamp is not the only
/// line of defense.
#[test]
fn row_cap_is_clamped_to_engine_ceiling() {
    use tumult_cypher::MAX_ROW_CAP;

    let mut snapshot = GraphSnapshot::default();
    for i in 0..(MAX_ROW_CAP + 5) {
        snapshot.nodes.push(node(
            &format!("svc:{i:05}"),
            "service",
            &format!("Service {i}"),
            json!({}),
        ));
    }
    let table = run_cypher_capped(&snapshot, "MATCH (s:service) RETURN s.id", usize::MAX).unwrap();
    assert!(table.truncated);
    assert_eq!(table.rows.len(), MAX_ROW_CAP);
}

/// A traversal whose estimated expansion work blows the evaluation budget is
/// rejected before the graph is even built — a whole-graph snapshot must not
/// give an agent unbounded compute.
#[test]
fn expansion_budget_rejects_explosive_traversal() {
    use tumult_cypher::MAX_EXPANSION_STEPS;

    // 2 000 nodes / 20 000 unique edges: average degree 20, so a 3-hop chain
    // estimates ~16.8M expansion steps — far over the budget.
    let mut snapshot = GraphSnapshot::default();
    for i in 0..2_000 {
        snapshot.nodes.push(node(
            &format!("svc:{i:04}"),
            "service",
            &format!("Service {i}"),
            json!({}),
        ));
    }
    for i in 0..2_000 {
        for k in 0..10 {
            snapshot.edges.push(edge(
                &format!("svc:{i:04}"),
                "depends_on",
                &format!("svc:{:04}", (i + 1 + k * 191) % 2_000),
                "run-1",
                1_000,
            ));
        }
    }

    let err = run_cypher(&snapshot, "MATCH (a)-->(b)-->(c)-->(d) RETURN a.id, d.id").unwrap_err();
    assert!(
        matches!(err, CypherError::BudgetExceeded { estimated, budget }
            if estimated > MAX_EXPANSION_STEPS && budget == MAX_EXPANSION_STEPS),
        "expected BudgetExceeded, got {err:?}"
    );
    assert!(
        err.to_string().contains("budget"),
        "message must name the budget: {err}"
    );

    // The same snapshot answers a shallow query: the budget targets explosive
    // traversals, not large graphs per se.
    let table = run_cypher_capped(&snapshot, "MATCH (a)-->(b) RETURN b.id", 5).unwrap();
    assert_eq!(table.rows.len(), 5);
    assert!(table.truncated);
}

/// Unbounded variable-length patterns count against the budget via the
/// documented hop assumption.
#[test]
fn unbounded_variable_length_patterns_count_against_budget() {
    // 500 nodes / 4 000 unique edges: degree 16; five `*` tiers at 5 assumed
    // hops each estimate 500 * 80 + 500 * 80^2 + ... ≈ 260M steps.
    let mut snapshot = GraphSnapshot::default();
    for i in 0..500 {
        snapshot.nodes.push(node(
            &format!("svc:{i:03}"),
            "service",
            &format!("Service {i}"),
            json!({}),
        ));
    }
    for i in 0..500 {
        for k in 0..8 {
            snapshot.edges.push(edge(
                &format!("svc:{i:03}"),
                "depends_on",
                &format!("svc:{:03}", (i + 1 + k * 61) % 500),
                "run-1",
                1_000,
            ));
        }
    }
    let err = run_cypher(
        &snapshot,
        "MATCH (a)-[*]->(b)-[*]->(c)-[*]->(d)-[*]->(e)-[*]->(f) RETURN f.id",
    )
    .unwrap_err();
    assert!(
        matches!(err, CypherError::BudgetExceeded { .. }),
        "expected BudgetExceeded, got {err:?}"
    );
}
