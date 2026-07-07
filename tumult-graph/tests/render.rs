//! View-level tests for `tumult_graph::render`: build, text, Mermaid, JSON.

use tumult_graph::lineage::{BreakCause, ControlServiceStatus, LineageCell};
use tumult_graph::recommend::Recommendation;
use tumult_graph::render::{build_view, ServiceState, TopologyMapView};
use tumult_graph::NodeSummary;

const DORA: &str = "compliance:DORA/Art.25";
const NIS2: &str = "compliance:NIS2/Art.21(2)(b)";

fn svc(id: &str, label: &str, attrs: serde_json::Value) -> (NodeSummary, serde_json::Value) {
    (
        NodeSummary {
            id: id.to_string(),
            kind: "service".to_string(),
            label: label.to_string(),
        },
        attrs,
    )
}

fn cell(article: &str, service: &str, status: ControlServiceStatus) -> LineageCell {
    LineageCell {
        article_id: article.to_string(),
        service_id: service.to_string(),
        status,
        evidence_strength: None,
        cause: None,
        experiments: Vec::new(),
    }
}

fn fixture_view() -> TopologyMapView {
    let services = vec![
        svc(
            "svc:db",
            "db",
            serde_json::json!({"tier": "data", "owner": "team-db"}),
        ),
        svc(
            "svc:gateway",
            "gateway",
            serde_json::json!({"tier": "edge"}),
        ),
        svc("svc:api", "api", serde_json::json!({"owner": "team-core"})),
        svc("svc:demo-app", "demo-app", serde_json::json!({})),
    ];
    let depends_on = vec![
        ("svc:gateway".to_string(), "svc:api".to_string()),
        ("svc:api".to_string(), "svc:db".to_string()),
        ("svc:gateway".to_string(), "svc:db".to_string()),
    ];
    let mut broken = cell(DORA, "svc:db", ControlServiceStatus::Broken);
    broken.cause = Some(BreakCause {
        deviation_id: "dev:abc".to_string(),
        fault_id: Some("fault:tumult-postgres::kill_connections".to_string()),
        guard_name: Some("p95_latency".to_string()),
        failing_actions: vec!["health-check".to_string()],
        run_id: "r2".to_string(),
    });
    let lineage = vec![
        broken,
        cell(NIS2, "svc:gateway", ControlServiceStatus::Untested),
        cell(DORA, "svc:api", ControlServiceStatus::Evidenced),
    ];
    let recommendations = vec![Recommendation {
        service_id: "svc:gateway".to_string(),
        plugin: "tumult-net".to_string(),
        action: "drop_packets".to_string(),
        article_id: NIS2.to_string(),
        strength: "supporting".to_string(),
        score: 0.875,
        reasons: vec!["NIS2 untested on svc:gateway".to_string()],
    }];
    build_view(&services, &depends_on, &lineage, &recommendations)
}

#[test]
fn services_ordered_by_tier_then_id_and_states_rolled_up() {
    let view = fixture_view();
    let order: Vec<&str> = view.services.iter().map(|s| s.id.as_str()).collect();
    // edge < (no tier => other) ... wait: edge < service < data < other.
    // gateway (edge), db (data), then untiered api/demo-app by id.
    assert_eq!(
        order,
        vec!["svc:gateway", "svc:db", "svc:api", "svc:demo-app"]
    );

    let by_id = |id: &str| view.services.iter().find(|s| s.id == id).expect("service");
    assert_eq!(by_id("svc:db").state, ServiceState::Broken);
    assert_eq!(by_id("svc:gateway").state, ServiceState::Untested);
    assert_eq!(by_id("svc:api").state, ServiceState::Evidenced);
    assert_eq!(by_id("svc:demo-app").state, ServiceState::Unknown);

    assert_eq!(by_id("svc:db").broken.len(), 1);
    assert_eq!(by_id("svc:db").tier.as_deref(), Some("data"));
    assert_eq!(by_id("svc:db").owner.as_deref(), Some("team-db"));

    // Determinism: same inputs, same view.
    assert_eq!(view, fixture_view());
}

#[test]
fn text_contains_legend_cause_dependents_and_recommendation() {
    let text = fixture_view().to_text();
    assert!(text.starts_with("legend:"), "legend first: {text}");
    assert!(text.contains("[BROKEN] svc:db (data, team-db)"));
    assert!(text.contains(
        "compliance:DORA/Art.25 broken — fault:tumult-postgres::kill_connections (guard: p95_latency)"
    ));
    assert!(text.contains("<- api, gateway depend on this"));
    assert!(text.contains(
        ">> RECOMMENDED: tumult-net::drop_packets for compliance:NIS2/Art.21(2)(b) (score 0.88)"
    ));
    assert!(text.contains("[UNTESTED] svc:gateway (edge)"));
    assert!(text.contains("compliance:DORA/Art.25 evidenced"));
    assert!(text.contains("[UNKNOWN] svc:demo-app"));
}

#[test]
fn mermaid_sanitizes_ids_and_declares_classes() {
    let mermaid = fixture_view().to_mermaid();
    assert!(mermaid.starts_with("graph TD\n"));

    // Sanitization: colons and dashes become underscores.
    assert!(mermaid.contains("svc_demo_app[\"demo-app UNKNOWN\"]"));
    assert!(mermaid.contains("svc_db[\"db BROKEN\"]"));
    assert!(mermaid.contains("svc_gateway --> svc_api"));
    assert!(
        !mermaid.contains("svc:db"),
        "raw ids must not leak into node ids"
    );

    // Class definitions and assignments.
    assert!(mermaid.contains("classDef evidenced fill:#2e7d32,color:#fff"));
    assert!(mermaid.contains("classDef broken fill:#c62828,color:#fff"));
    assert!(mermaid.contains("classDef untested fill:#f9a825"));
    assert!(mermaid.contains("classDef unknown fill:#546e7a,color:#fff"));
    assert!(mermaid.contains("classDef recommended fill:#6a1b9a,color:#fff"));
    assert!(mermaid.contains("class svc_db broken"));
    assert!(mermaid.contains("class svc_gateway untested"));
    assert!(mermaid.contains("class svc_api evidenced"));
    assert!(mermaid.contains("class svc_demo_app unknown"));
    assert!(mermaid.contains("class rec_0 recommended"));

    // Cause annotation, dash-linked to the broken service.
    assert!(mermaid.contains(
        "cause_svc_db_0[\"fault: fault:tumult-postgres::kill_connections<br/>guard: p95_latency\"]"
    ));
    assert!(mermaid.contains("cause_svc_db_0 -.-> svc_db"));

    // Recommendation node, dash-linked to its service.
    assert!(mermaid
        .contains("rec_0[\"⚡ tumult-net::drop_packets<br/>for compliance:NIS2/Art.21(2)(b)\"]"));
    assert!(mermaid.contains("rec_0 -.-> svc_gateway"));

    // Determinism.
    assert_eq!(mermaid, fixture_view().to_mermaid());
}

#[test]
fn json_round_trips_the_view_shape() {
    let view = fixture_view();
    let json = view.to_json();
    assert_eq!(json["services"].as_array().map(Vec::len), Some(4));
    assert_eq!(json["services"][0]["id"], "svc:gateway");
    assert_eq!(json["services"][0]["state"], "untested");
    assert_eq!(
        json["services"][1]["broken"][0]["guard_name"],
        "p95_latency"
    );
    assert_eq!(json["recommendations"][0]["action"], "drop_packets");
    assert_eq!(json["depends_on"].as_array().map(Vec::len), Some(3));
}
