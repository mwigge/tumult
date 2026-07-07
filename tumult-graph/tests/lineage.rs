//! Fixture-driven tests for `tumult_graph::lineage::compute_lineage`.

use std::collections::HashMap;

use tumult_graph::lineage::{compute_lineage, ControlServiceStatus, LineageInput};
use tumult_graph::{EdgeRecord, NodeSummary};

const DORA: &str = "compliance:DORA/Art.25";
const NIS2: &str = "compliance:NIS2/Art.21(2)(b)";

fn edge(src: &str, rel: &str, dst: &str, run_id: &str, ts: i64, attrs: &str) -> EdgeRecord {
    EdgeRecord {
        src: src.to_string(),
        rel: rel.to_string(),
        dst: dst.to_string(),
        run_id: run_id.to_string(),
        ts,
        attrs: attrs.to_string(),
    }
}

fn node(id: &str, kind: &str) -> NodeSummary {
    NodeSummary {
        id: id.to_string(),
        kind: kind.to_string(),
        label: id.split(':').next_back().unwrap_or(id).to_string(),
    }
}

fn services() -> Vec<NodeSummary> {
    vec![node("svc:db", "service"), node("svc:gateway", "service")]
}

fn articles() -> Vec<NodeSummary> {
    vec![
        node(DORA, "compliance_article"),
        node(NIS2, "compliance_article"),
    ]
}

/// A run that maps + targets + evidences.
fn evidenced_run(exp: &str, svc: &str, run: &str, ts: i64) -> Vec<EdgeRecord> {
    vec![
        edge(
            exp,
            "maps_to_compliance",
            DORA,
            run,
            ts,
            "{\"framework\":\"DORA\",\"control\":\"Art. 25\"}",
        ),
        edge(exp, "targets", svc, run, ts, "{}"),
        edge(
            exp,
            "evidences",
            DORA,
            run,
            ts,
            "{\"strength\":\"direct\",\"framework\":\"DORA\",\"control\":\"Art. 25\"}",
        ),
    ]
}

/// A run that maps + targets but deviates instead of evidencing.
fn broken_run(exp: &str, svc: &str, run: &str, ts: i64, dev: &str) -> Vec<EdgeRecord> {
    vec![
        edge(exp, "maps_to_compliance", DORA, run, ts, "{}"),
        edge(exp, "targets", svc, run, ts, "{}"),
        edge(exp, "yielded", &format!("run:{run}"), run, ts, "{}"),
        edge(&format!("run:{run}"), "exhibited", dev, run, ts, "{}"),
    ]
}

fn cell<'a>(
    cells: &'a [tumult_graph::lineage::LineageCell],
    article: &str,
    service: &str,
) -> &'a tumult_graph::lineage::LineageCell {
    cells
        .iter()
        .find(|c| c.article_id == article && c.service_id == service)
        .unwrap_or_else(|| panic!("no cell for ({article}, {service})"))
}

#[test]
fn evidenced_run_yields_evidenced_cell_with_strength() {
    let edges = evidenced_run("exp:latency", "svc:db", "r1", 10);
    let attrs = HashMap::new();
    let input = LineageInput {
        edges: &edges,
        services: &services(),
        articles: &articles(),
        deviation_attrs: &attrs,
    };
    let cells = compute_lineage(&input, None, None);

    let c = cell(&cells, DORA, "svc:db");
    assert_eq!(c.status, ControlServiceStatus::Evidenced);
    assert_eq!(c.evidence_strength.as_deref(), Some("direct"));
    assert!(c.cause.is_none());
    assert_eq!(c.experiments, vec!["exp:latency".to_string()]);
    // Output is sorted by (article_id, service_id).
    let keys: Vec<(&str, &str)> = cells
        .iter()
        .map(|c| (c.article_id.as_str(), c.service_id.as_str()))
        .collect();
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    assert_eq!(keys, sorted);
}

#[test]
fn broken_run_with_attribution_builds_full_cause() {
    let mut edges = broken_run("exp:latency", "svc:db", "r1", 10, "dev:abc");
    edges.push(edge(
        "dev:abc",
        "caused_by",
        "fault:tumult-postgres::kill_connections",
        "r1",
        10,
        "{}",
    ));
    let attrs: HashMap<String, serde_json::Value> = [(
        "dev:abc".to_string(),
        serde_json::json!({
            "status": "deviated",
            "halt": { "guard_name": "p95_latency", "observed": 900.0, "safe_condition": "< 500" },
            "failing_actions": ["health-check", "read-replica"],
        }),
    )]
    .into_iter()
    .collect();
    let input = LineageInput {
        edges: &edges,
        services: &services(),
        articles: &articles(),
        deviation_attrs: &attrs,
    };
    let cells = compute_lineage(&input, None, None);

    let c = cell(&cells, DORA, "svc:db");
    assert_eq!(c.status, ControlServiceStatus::Broken);
    assert!(c.evidence_strength.is_none());
    let cause = c.cause.as_ref().expect("broken cell carries a cause");
    assert_eq!(cause.deviation_id, "dev:abc");
    assert_eq!(
        cause.fault_id.as_deref(),
        Some("fault:tumult-postgres::kill_connections")
    );
    assert_eq!(cause.guard_name.as_deref(), Some("p95_latency"));
    assert_eq!(cause.failing_actions, vec!["health-check", "read-replica"]);
    assert_eq!(cause.run_id, "r1");
}

#[test]
fn broken_run_without_attribution_leaves_fault_and_guard_none() {
    let edges = broken_run("exp:latency", "svc:db", "r1", 10, "dev:abc");
    let attrs = HashMap::new();
    let input = LineageInput {
        edges: &edges,
        services: &services(),
        articles: &articles(),
        deviation_attrs: &attrs,
    };
    let cells = compute_lineage(&input, None, None);

    let cause = cell(&cells, DORA, "svc:db").cause.as_ref().expect("cause");
    assert_eq!(cause.deviation_id, "dev:abc");
    assert!(cause.fault_id.is_none());
    assert!(cause.guard_name.is_none());
    assert!(cause.failing_actions.is_empty());
}

#[test]
fn mapped_run_with_no_evidence_and_no_deviation_is_broken_with_bare_cause() {
    let edges = vec![
        edge("exp:latency", "maps_to_compliance", DORA, "r1", 10, "{}"),
        edge("exp:latency", "targets", "svc:db", "r1", 10, "{}"),
    ];
    let attrs = HashMap::new();
    let input = LineageInput {
        edges: &edges,
        services: &services(),
        articles: &articles(),
        deviation_attrs: &attrs,
    };
    let cells = compute_lineage(&input, None, None);

    let c = cell(&cells, DORA, "svc:db");
    assert_eq!(c.status, ControlServiceStatus::Broken);
    let cause = c.cause.as_ref().expect("cause with run_id only");
    assert!(cause.deviation_id.is_empty());
    assert!(cause.fault_id.is_none());
    assert!(cause.guard_name.is_none());
    assert!(cause.failing_actions.is_empty());
    assert_eq!(cause.run_id, "r1");
}

#[test]
fn uncovered_pairs_are_untested() {
    let edges = evidenced_run("exp:latency", "svc:db", "r1", 10);
    let attrs = HashMap::new();
    let input = LineageInput {
        edges: &edges,
        services: &services(),
        articles: &articles(),
        deviation_attrs: &attrs,
    };
    let cells = compute_lineage(&input, None, None);

    // 2 articles × 2 services = 4 cells; only (DORA, db) is covered.
    assert_eq!(cells.len(), 4);
    for (article, service) in [
        (DORA, "svc:gateway"),
        (NIS2, "svc:db"),
        (NIS2, "svc:gateway"),
    ] {
        let c = cell(&cells, article, service);
        assert_eq!(
            c.status,
            ControlServiceStatus::Untested,
            "({article}, {service})"
        );
        assert!(c.evidence_strength.is_none());
        assert!(c.cause.is_none());
        assert!(c.experiments.is_empty());
    }
}

#[test]
fn latest_run_wins_in_both_directions() {
    let attrs = HashMap::new();

    // Deviated at ts=1, completed at ts=2 → Evidenced.
    let mut edges = broken_run("exp:latency", "svc:db", "r1", 1, "dev:abc");
    edges.extend(evidenced_run("exp:latency", "svc:db", "r2", 2));
    let input = LineageInput {
        edges: &edges,
        services: &services(),
        articles: &articles(),
        deviation_attrs: &attrs,
    };
    let cells = compute_lineage(&input, None, None);
    assert_eq!(
        cell(&cells, DORA, "svc:db").status,
        ControlServiceStatus::Evidenced
    );

    // Completed at ts=1, deviated at ts=2 → Broken, cause from the later run.
    let mut edges = evidenced_run("exp:latency", "svc:db", "r1", 1);
    edges.extend(broken_run("exp:latency", "svc:db", "r2", 2, "dev:abc"));
    let input = LineageInput {
        edges: &edges,
        services: &services(),
        articles: &articles(),
        deviation_attrs: &attrs,
    };
    let cells = compute_lineage(&input, None, None);
    let c = cell(&cells, DORA, "svc:db");
    assert_eq!(c.status, ControlServiceStatus::Broken);
    assert_eq!(
        c.cause.as_ref().map(|cause| cause.run_id.as_str()),
        Some("r2")
    );
}

#[test]
fn framework_filter_scopes_articles_case_insensitively() {
    let edges = evidenced_run("exp:latency", "svc:db", "r1", 10);
    let attrs = HashMap::new();
    let input = LineageInput {
        edges: &edges,
        services: &services(),
        articles: &articles(),
        deviation_attrs: &attrs,
    };

    let cells = compute_lineage(&input, Some("dora"), None);
    assert!(cells
        .iter()
        .all(|c| c.article_id.starts_with("compliance:DORA/")));
    assert_eq!(cells.len(), 2); // DORA × {db, gateway}
    assert_eq!(
        cell(&cells, DORA, "svc:db").status,
        ControlServiceStatus::Evidenced
    );

    let cells = compute_lineage(&input, Some("nis2"), None);
    assert!(cells
        .iter()
        .all(|c| c.article_id.starts_with("compliance:NIS2/")));
    assert!(cells
        .iter()
        .all(|c| c.status == ControlServiceStatus::Untested));
}

#[test]
fn control_filter_matches_exact_suffix() {
    let edges = evidenced_run("exp:latency", "svc:db", "r1", 10);
    let attrs = HashMap::new();
    let input = LineageInput {
        edges: &edges,
        services: &services(),
        articles: &articles(),
        deviation_attrs: &attrs,
    };

    let cells = compute_lineage(&input, None, Some("Art.25"));
    assert_eq!(cells.len(), 2);
    assert!(cells.iter().all(|c| c.article_id == DORA));

    // Exact match: a lowercase control does not match.
    let cells = compute_lineage(&input, None, Some("art.25"));
    assert!(cells.is_empty());
}

#[test]
fn tie_on_ts_breaks_by_greater_run_id() {
    let attrs = HashMap::new();
    // Same ts: run "b" (evidenced) beats run "a" (broken) lexicographically.
    let mut edges = broken_run("exp:latency", "svc:db", "a", 5, "dev:abc");
    edges.extend(evidenced_run("exp:latency", "svc:db", "b", 5));
    let input = LineageInput {
        edges: &edges,
        services: &services(),
        articles: &articles(),
        deviation_attrs: &attrs,
    };
    let cells = compute_lineage(&input, None, None);
    assert_eq!(
        cell(&cells, DORA, "svc:db").status,
        ControlServiceStatus::Evidenced
    );
}
