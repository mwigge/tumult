//! Fixture-driven tests for `tumult_graph::recommend::recommend`.

use std::collections::{HashMap, HashSet};

use tumult_graph::lineage::{ControlServiceStatus, LineageCell};
use tumult_graph::recommend::{recommend, RecommendationInput};
use tumult_graph::AvailableAction;

const DORA: &str = "compliance:DORA/Art.25";
const NIS2: &str = "compliance:NIS2/Art.21(2)(b)";


fn empty_criticality() -> &'static std::collections::HashMap<String, f64> {
    use std::sync::OnceLock;
    static EMPTY: OnceLock<std::collections::HashMap<String, f64>> = OnceLock::new();
    EMPTY.get_or_init(std::collections::HashMap::new)
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

/// gateway -> api -> db.
fn fixture_edges() -> Vec<(String, String)> {
    vec![
        ("svc:gateway".to_string(), "svc:api".to_string()),
        ("svc:api".to_string(), "svc:db".to_string()),
    ]
}

fn fixture<'a>(
    lineage: &'a [LineageCell],
    depends_on: &'a [(String, String)],
    actions: &'a [AvailableAction],
    tested: &'a HashSet<String>,
    strengths: &'a HashMap<String, String>,
) -> RecommendationInput<'a> {
    RecommendationInput {
        criticality: empty_criticality(),
        lineage,
        depends_on,
        available_actions: actions,
        tested_action_names: tested,
        article_strength: strengths,
    }
}

/// gateway -> api -> db; DORA/Art.25 broken on db; NIS2/Art.21(2)(b)
/// untested on gateway. Expected scores derived by hand in comments.
#[test]
fn ranks_broken_central_service_first_with_reasons() {
    let lineage = vec![
        cell(DORA, "svc:db", ControlServiceStatus::Broken),
        cell(NIS2, "svc:gateway", ControlServiceStatus::Untested),
    ];
    let depends_on = fixture_edges();
    let actions = vec![
        AvailableAction::new("tumult-net", "inject_latency"),
        AvailableAction::new("tumult-net", "drop_packets"),
    ];
    let tested: HashSet<String> = ["inject_latency".to_string()].into_iter().collect();
    let strengths: HashMap<String, String> = [
        (DORA.to_string(), "direct".to_string()),
        (NIS2.to_string(), "supporting".to_string()),
    ]
    .into_iter()
    .collect();
    let input = fixture(&lineage, &depends_on, &actions, &tested, &strengths);

    let recs = recommend(&input, 10);
    assert_eq!(recs.len(), 2);

    // db: 1.0 (broken) × 1.0 (direct) × 2.0 (in-degree 1/1) × 1.0 (d=0)
    //     × 1.25 (drop_packets untested) = 2.5
    assert_eq!(recs[0].service_id, "svc:db");
    assert_eq!(recs[0].article_id, DORA);
    assert_eq!(recs[0].plugin, "tumult-net");
    assert_eq!(recs[0].action, "drop_packets");
    assert_eq!(recs[0].strength, "direct");
    assert!(
        (recs[0].score - 2.5).abs() < 1e-9,
        "score {}",
        recs[0].score
    );

    // gateway: 0.6 × 0.7 (supporting) × 1.0 (in-degree 0) × 1/3 (d=2)
    //          × 1.25 = 0.175
    assert_eq!(recs[1].service_id, "svc:gateway");
    assert_eq!(recs[1].article_id, NIS2);
    assert!(
        (recs[1].score - 0.175).abs() < 1e-9,
        "score {}",
        recs[1].score
    );

    for rec in &recs {
        assert!(!rec.reasons.is_empty());
        assert!(rec.reasons.iter().all(|r| !r.is_empty()));
    }
    assert!(recs[0]
        .reasons
        .iter()
        .any(|r| r.contains("broken on svc:db")));
    assert!(recs[1]
        .reasons
        .iter()
        .any(|r| r.contains("untested on svc:gateway")));
    assert!(recs[0].reasons.iter().any(|r| r.contains("never tested")));
}

#[test]
fn deterministic_and_limited() {
    let lineage = vec![
        cell(DORA, "svc:db", ControlServiceStatus::Broken),
        cell(NIS2, "svc:gateway", ControlServiceStatus::Untested),
    ];
    let depends_on = fixture_edges();
    let actions = vec![
        AvailableAction::new("tumult-net", "drop_packets"),
        AvailableAction::new("tumult-net", "inject_latency"),
    ];
    let tested: HashSet<String> = HashSet::new();
    let strengths: HashMap<String, String> = HashMap::new();
    let input = fixture(&lineage, &depends_on, &actions, &tested, &strengths);

    let first = recommend(&input, 10);
    let second = recommend(&input, 10);
    assert_eq!(first, second, "identical calls must yield identical output");

    let limited = recommend(&input, 1);
    assert_eq!(limited.len(), 1);
    assert_eq!(limited[0], first[0]);
    assert_eq!(limited[0].service_id, "svc:db");
}

#[test]
fn all_actions_tested_falls_back_to_first_sorted_without_bonus() {
    let lineage = vec![cell(DORA, "svc:db", ControlServiceStatus::Broken)];
    let actions = vec![
        AvailableAction::new("tumult-net", "inject_latency"),
        AvailableAction::new("tumult-net", "drop_packets"),
    ];
    let tested: HashSet<String> = ["inject_latency".to_string(), "drop_packets".to_string()]
        .into_iter()
        .collect();
    let strengths = HashMap::new();
    let depends_on: Vec<(String, String)> = Vec::new();
    let input = fixture(&lineage, &depends_on, &actions, &tested, &strengths);

    let recs = recommend(&input, 10);
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0].action, "drop_packets"); // first in sorted order
                                                // 1.0 (broken) × 0.4 (default indirect) × 1.0 (no edges) × 1.0 (d=0)
    assert!((recs[0].score - 0.4).abs() < 1e-9);
    assert!(!recs[0].reasons.iter().any(|r| r.contains("never tested")));
}

#[test]
fn empty_catalog_or_no_actionable_cells_yields_nothing() {
    let actions = vec![AvailableAction::new("tumult-net", "drop_packets")];
    let tested = HashSet::new();
    let strengths = HashMap::new();
    let depends_on = Vec::new();

    // Evidenced-only lineage: nothing to recommend.
    let lineage = vec![cell(DORA, "svc:db", ControlServiceStatus::Evidenced)];
    let input = fixture(&lineage, &depends_on, &actions, &tested, &strengths);
    assert!(recommend(&input, 10).is_empty());

    // Broken cell but empty catalog: nothing to recommend either.
    let broken = vec![cell(DORA, "svc:db", ControlServiceStatus::Broken)];
    let no_actions: Vec<AvailableAction> = Vec::new();
    let input = fixture(&broken, &depends_on, &no_actions, &tested, &strengths);
    assert!(recommend(&input, 10).is_empty());
}
