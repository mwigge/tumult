use std::collections::BTreeMap;

use super::*;

fn unit(path: &str, kind: &str) -> OrgUnitRow {
    OrgUnitRow {
        path: path.to_string(),
        name: path.rsplit('/').next().unwrap_or_default().to_string(),
        kind: kind.to_string(),
    }
}

fn org() -> Vec<OrgUnitRow> {
    vec![
        unit("", "company"),
        unit("platform", "domain"),
        unit("platform/edge", "unit"),
        unit("platform/edge/edge-team", "team"),
        unit("platform/data", "unit"),
        unit("platform/data/data-team", "team"),
    ]
}

fn glob(pattern: &str, path: &str) -> GlobRow {
    GlobRow {
        pattern: pattern.to_string(),
        path: path.to_string(),
    }
}

fn svc(name: &str) -> ServiceRow {
    ServiceRow {
        service: name.to_string(),
        environment: Some("demo".to_string()),
        spans: 10,
        last_seen_ns: 1_000,
        ..ServiceRow::default()
    }
}

fn svc_ent(name: &str, entity: &str) -> ServiceRow {
    ServiceRow {
        entity_id: Some(entity.to_string()),
        ..svc(name)
    }
}

fn sys(name: &str, statuses: &[(&str, &str)]) -> SystemRow {
    SystemRow {
        system: name.to_string(),
        environment: Some("demo".to_string()),
        disciplines: statuses
            .iter()
            .map(|(k, s)| ((*k).to_string(), (*s).to_string()))
            .collect(),
        ..SystemRow::default()
    }
}

fn exp(name: &str, passed: bool) -> ExperimentRow {
    ExperimentRow {
        name: name.to_string(),
        kind: "chaos".to_string(),
        passed,
        last_run_ns: 500,
    }
}

fn workload(name: &str, labels: &[(&str, &str)]) -> WorkloadRow {
    WorkloadRow {
        name: name.to_string(),
        namespace: "demo".to_string(),
        kind: "Deployment".to_string(),
        labels: labels
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect::<BTreeMap<_, _>>(),
    }
}

fn run(id: &str, experiment: &str, system: &str, failed: bool) -> RunRow {
    RunRow {
        id: id.to_string(),
        experiment: experiment.to_string(),
        system: Some(system.to_string()),
        environment: Some("demo".to_string()),
        started_ns: 900,
        failed,
        manual: false,
    }
}

fn risk(target: &str, severity: &str, suppressed: bool) -> RiskRow {
    RiskRow {
        target: Some(target.to_string()),
        environment: Some("demo".to_string()),
        subject: None,
        severity: severity.to_string(),
        recommended_kind: Some("dr".to_string()),
        suppression: suppressed.then(|| SuppressionRow {
            actor: "morgan".to_string(),
            reason: "accepted until the migration".to_string(),
            expires_at_ns: Some(9_000),
        }),
    }
}

fn node<'a>(map: &'a StarMap, id: &str) -> &'a StarNode {
    map.nodes
        .iter()
        .find(|n| n.id == id)
        .unwrap_or_else(|| panic!("node {id} missing from {:?}", ids(map)))
}

fn ids(map: &StarMap) -> Vec<&str> {
    map.nodes.iter().map(|n| n.id.as_str()).collect()
}

fn has_node(map: &StarMap, id: &str) -> bool {
    map.nodes.iter().any(|n| n.id == id)
}

fn edge<'a>(map: &'a StarMap, from: &str, rel: StarRel, to: &str) -> Option<&'a StarEdge> {
    map.edges
        .iter()
        .find(|e| e.from == from && e.rel == rel && e.to == to)
}

fn build(inputs: &StarInputs) -> StarMap {
    assemble(inputs, &StarQuery::default())
}

#[test]
fn star_kinds_round_trip_and_map_to_chaosgraph() {
    for kind in StarKind::ALL {
        assert_eq!(StarKind::parse(kind.as_str()), Some(kind));
    }
    assert_eq!(StarKind::parse("tgt"), None);
    assert_eq!(
        StarKind::Entity.graph_kind(),
        Some(crate::NodeKind::IctEntity)
    );
    assert_eq!(StarKind::Cluster.graph_kind(), None);
}

#[test]
fn chaosgraph_relations_map_in_one_place() {
    use crate::EdgeRel;
    assert_eq!(
        StarRel::from_graph(EdgeRel::Targets),
        Some(StarRel::TestedBy)
    );
    assert_eq!(
        StarRel::from_graph(EdgeRel::Yielded),
        Some(StarRel::TestedBy)
    );
    assert_eq!(
        StarRel::from_graph(EdgeRel::Evidences),
        Some(StarRel::Covers)
    );
    assert_eq!(
        StarRel::from_graph(EdgeRel::MapsToCompliance),
        Some(StarRel::Covers)
    );
    assert_eq!(
        StarRel::from_graph(EdgeRel::Exhibited),
        Some(StarRel::Broke)
    );
    assert_eq!(
        StarRel::from_graph(EdgeRel::ObservedOn),
        Some(StarRel::Broke)
    );
    assert_eq!(StarRel::from_graph(EdgeRel::Injects), None);
}

#[test]
fn service_with_entity_id_implements_the_entity() {
    let inputs = StarInputs {
        org_units: org(),
        services: vec![ServiceRow {
            entity_tier: Some("critical".to_string()),
            ..svc_ent("checkout", "CIF-0042")
        }],
        ..StarInputs::default()
    };
    let map = build(&inputs);

    let implements = edge(&map, "svc:checkout", StarRel::Implements, "ent:cif-0042")
        .expect("svc implements ent");
    assert_eq!(implements.source, EdgeSource::Telemetry);
    assert_eq!(implements.id, "svc:checkout|implements|ent:cif-0042");
    let entity = node(&map, "ent:cif-0042");
    assert_eq!(entity.kind, StarKind::Entity);
    assert_eq!(entity.tier.as_deref(), Some("critical"));
    assert!((entity.criticality_weight - 3.0).abs() < f64::EPSILON);
    assert_eq!(
        node(&map, "svc:checkout").parent.as_deref(),
        Some("ent:cif-0042")
    );
    assert!(!node(&map, "svc:checkout").conflict);
}

#[test]
fn system_and_component_come_from_the_promoted_target_columns() {
    let inputs = StarInputs {
        org_units: org(),
        services: vec![ServiceRow {
            system: Some("shop".to_string()),
            component: Some("checkout".to_string()),
            ..svc_ent("checkout", "cif-0042")
        }],
        ..StarInputs::default()
    };
    let map = build(&inputs);

    assert_eq!(node(&map, "sys:demo/shop").kind, StarKind::System);
    assert_eq!(
        node(&map, "cmp:demo/shop/checkout").parent.as_deref(),
        Some("sys:demo/shop")
    );
    assert_eq!(
        node(&map, "svc:checkout").parent.as_deref(),
        Some("cmp:demo/shop/checkout")
    );
    assert_eq!(
        node(&map, "sys:demo/shop").parent.as_deref(),
        Some("ent:cif-0042")
    );
    assert!(edge(&map, "ent:cif-0042", StarRel::Implements, "sys:demo/shop").is_some());
    assert!(edge(
        &map,
        "cmp:demo/shop/checkout",
        StarRel::Implements,
        "ent:cif-0042"
    )
    .is_some());
    assert!(map.nodes.iter().all(|n| !n.id.starts_with("tgt:")));
}

#[test]
fn owner_comes_from_telemetry_first() {
    let inputs = StarInputs {
        org_units: org(),
        entity_globs: vec![glob("cif-*", "platform/data/data-team")],
        services: vec![ServiceRow {
            entity_owner: Some("edge-team".to_string()),
            ..svc_ent("checkout", "cif-0042")
        }],
        ..StarInputs::default()
    };
    let map = build(&inputs);

    assert_eq!(
        node(&map, "ent:cif-0042").parent.as_deref(),
        Some("org:platform/edge/edge-team")
    );
    let owns = edge(
        &map,
        "org:platform/edge/edge-team",
        StarRel::Owns,
        "ent:cif-0042",
    )
    .expect("owns edge");
    assert_eq!(owns.source, EdgeSource::Telemetry);
}

#[test]
fn owner_falls_back_to_the_entity_glob() {
    let inputs = StarInputs {
        org_units: org(),
        entity_globs: vec![
            glob("cif-00*", "platform/data/data-team"),
            glob("cif-*", "platform/edge/edge-team"),
        ],
        services: vec![ServiceRow {
            entity_owner: Some("no-such-team".to_string()),
            ..svc_ent("checkout", "cif-0042")
        }],
        ..StarInputs::default()
    };
    let map = build(&inputs);

    assert_eq!(
        node(&map, "ent:cif-0042").parent.as_deref(),
        Some("org:platform/data/data-team")
    );
    let owns = edge(
        &map,
        "org:platform/data/data-team",
        StarRel::Owns,
        "ent:cif-0042",
    )
    .expect("owns edge");
    assert_eq!(owns.source, EdgeSource::Declared);
}

#[test]
fn owner_falls_back_to_the_experiment_name_assignment() {
    let inputs = StarInputs {
        org_units: org(),
        experiment_globs: vec![glob("queue-*", "platform/data/data-team")],
        services: vec![ServiceRow {
            system: Some("message-queue".to_string()),
            ..svc_ent("mq", "cif-0004")
        }],
        systems: vec![
            SystemRow {
                experiments: vec![exp("queue-dependency-loss", false)],
                ..sys("message-queue", &[("dr", "covered")])
            },
            SystemRow {
                experiments: vec![exp("queue-failover", true)],
                ..sys("storage", &[("dr", "covered")])
            },
        ],
        ..StarInputs::default()
    };
    let map = build(&inputs);

    assert_eq!(
        node(&map, "ent:cif-0004").parent.as_deref(),
        Some("org:platform/data/data-team")
    );
    // A system with no entity is owned directly, through the same globs.
    assert_eq!(
        node(&map, "sys:demo/storage").parent.as_deref(),
        Some("org:platform/data/data-team")
    );
    assert!(edge(
        &map,
        "org:platform/data/data-team",
        StarRel::Owns,
        "sys:demo/storage"
    )
    .is_some());
}

#[test]
fn owner_is_unassigned_when_nothing_resolves() {
    let inputs = StarInputs {
        org_units: org(),
        services: vec![svc_ent("checkout", "cif-0042")],
        systems: vec![SystemRow {
            experiments: vec![exp("service partition", true)],
            ..sys("service", &[("chaos", "covered")])
        }],
        ..StarInputs::default()
    };
    let map = build(&inputs);

    assert_eq!(
        node(&map, "ent:cif-0042").parent.as_deref(),
        Some("org:(unassigned)")
    );
    assert_eq!(
        node(&map, "sys:demo/service").parent.as_deref(),
        Some("org:(unassigned)")
    );
    let unassigned = node(&map, "org:(unassigned)");
    assert_eq!(unassigned.parent.as_deref(), Some("org:"));
    assert_eq!(unassigned.org_kind.as_deref(), Some("unassigned"));
    assert_eq!(unassigned.children, 2);
    assert!(map
        .edges
        .iter()
        .all(|e| !(e.rel == StarRel::Owns && e.from == "org:(unassigned)")));
}

#[test]
fn unassigned_unit_is_absent_when_everything_is_owned() {
    let inputs = StarInputs {
        org_units: org(),
        entity_globs: vec![glob("cif-*", "platform/edge/edge-team")],
        services: vec![svc_ent("checkout", "cif-0042")],
        ..StarInputs::default()
    };
    let map = build(&inputs);

    assert!(!has_node(&map, "org:(unassigned)"));
    assert_eq!(node(&map, "org:").label, "(company)");
    assert_eq!(
        node(&map, "org:platform/edge/edge-team").parent.as_deref(),
        Some("org:platform/edge")
    );
    assert_eq!(
        node(&map, "org:platform/edge/edge-team")
            .org_kind
            .as_deref(),
        Some("team")
    );
}

#[test]
fn two_entity_ids_on_one_service_set_conflict() {
    let inputs = StarInputs {
        org_units: org(),
        services: vec![
            svc_ent("checkout", "cif-0042"),
            svc_ent("checkout", "cif-0043"),
        ],
        ..StarInputs::default()
    };
    let map = build(&inputs);

    assert!(node(&map, "svc:checkout").conflict);
    assert!(edge(&map, "svc:checkout", StarRel::Implements, "ent:cif-0042").is_some());
    assert!(edge(&map, "svc:checkout", StarRel::Implements, "ent:cif-0043").is_some());
    assert_eq!(
        node(&map, "svc:checkout").parent.as_deref(),
        Some("ent:cif-0042")
    );
}

#[test]
fn workload_joins_a_service_by_k8s_attributes() {
    let inputs = StarInputs {
        org_units: org(),
        services: vec![ServiceRow {
            k8s_namespace: Some("demo".to_string()),
            k8s_deployment: Some("checkout-v2".to_string()),
            ..svc("checkout")
        }],
        workloads: vec![workload("checkout-v2", &[("app", "something-else")])],
        ..StarInputs::default()
    };
    let map = build(&inputs);

    let runs_on = edge(
        &map,
        "svc:checkout",
        StarRel::RunsOn,
        "wl:demo/deployment/checkout-v2",
    )
    .expect("runs_on by k8s attributes");
    assert_eq!(runs_on.source, EdgeSource::Telemetry);
    let wl = node(&map, "wl:demo/deployment/checkout-v2");
    assert_eq!(wl.kind, StarKind::Workload);
    assert_eq!(wl.parent.as_deref(), Some("svc:checkout"));
    assert_eq!(wl.env.as_deref(), Some("demo"));
}

#[test]
fn workload_joins_a_service_by_app_label_fallback() {
    let inputs = StarInputs {
        org_units: org(),
        services: vec![svc("checkout")],
        workloads: vec![workload("checkout-deploy", &[("app", "checkout")])],
        ..StarInputs::default()
    };
    let map = build(&inputs);

    let runs_on = edge(
        &map,
        "svc:checkout",
        StarRel::RunsOn,
        "wl:demo/deployment/checkout-deploy",
    )
    .expect("runs_on by app label");
    assert_eq!(runs_on.source, EdgeSource::Declared);
}

#[test]
fn workload_labels_declare_system_component_and_entity() {
    let inputs = StarInputs {
        org_units: org(),
        entity_globs: vec![glob("cif-0004", "platform/data/data-team")],
        systems: vec![sys("message-queue", &[("dr", "covered")])],
        workloads: vec![WorkloadRow {
            kind: "StatefulSet".to_string(),
            ..workload(
                "message-queue",
                &[
                    ("resilience.target.system", "message-queue"),
                    ("resilience.target.component", "broker"),
                    ("ict.entity.id", "CIF-0004"),
                    ("ict.entity.tier", "critical"),
                ],
            )
        }],
        ..StarInputs::default()
    };
    let map = build(&inputs);

    let wl = "wl:demo/statefulset/message-queue";
    let runs_on = edge(&map, "cmp:demo/message-queue/broker", StarRel::RunsOn, wl)
        .expect("component runs on workload");
    assert_eq!(runs_on.source, EdgeSource::Declared);
    assert_eq!(
        node(&map, wl).parent.as_deref(),
        Some("cmp:demo/message-queue/broker")
    );
    let implements = edge(
        &map,
        "ent:cif-0004",
        StarRel::Implements,
        "sys:demo/message-queue",
    )
    .expect("entity implements system");
    assert_eq!(implements.source, EdgeSource::Declared);
    assert_eq!(
        node(&map, "sys:demo/message-queue").parent.as_deref(),
        Some("ent:cif-0004")
    );
    assert_eq!(
        node(&map, "ent:cif-0004").parent.as_deref(),
        Some("org:platform/data/data-team")
    );
    assert_eq!(node(&map, "ent:cif-0004").tier.as_deref(), Some("critical"));
}

#[test]
fn workload_with_no_labels_joins_a_system_by_name() {
    let inputs = StarInputs {
        org_units: org(),
        systems: vec![sys("api-worker", &[("chaos", "covered")])],
        workloads: vec![workload("api-worker", &[])],
        ..StarInputs::default()
    };
    let map = build(&inputs);

    assert!(edge(
        &map,
        "sys:demo/api-worker",
        StarRel::RunsOn,
        "wl:demo/deployment/api-worker"
    )
    .is_some());
}

#[test]
fn system_missing_in_every_discipline_is_a_gap() {
    let inputs = StarInputs {
        org_units: org(),
        systems: vec![
            sys("cache", &[("chaos", "missing"), ("dr", "missing")]),
            sys("queue", &[("chaos", "stale"), ("dr", "missing")]),
            sys("api", &[("chaos", "covered"), ("dr", "missing")]),
        ],
        ..StarInputs::default()
    };
    let map = build(&inputs);

    assert_eq!(node(&map, "sys:demo/cache").gap, Some(Gap::Missing));
    assert_eq!(node(&map, "sys:demo/queue").gap, Some(Gap::Stale));
    assert_eq!(node(&map, "sys:demo/api").gap, None);
    assert_eq!(
        node(&map, "sys:demo/api").evidence.by_discipline.get("dr"),
        Some(&"missing".to_string())
    );
    assert_eq!(map.counts.gaps, 2);
    assert_eq!(serde_json::to_value(Gap::Missing).unwrap(), "missing");
}

#[test]
fn declared_system_without_evidence_is_a_gap_and_rolls_up_to_its_entity() {
    let inputs = StarInputs {
        org_units: org(),
        workloads: vec![workload(
            "session-cache",
            &[
                ("resilience.target.system", "session-cache"),
                ("resilience.target.environment", "demo"),
                ("ict.entity.id", "cif-0005"),
            ],
        )],
        ..StarInputs::default()
    };
    let map = build(&inputs);

    let system = node(&map, "sys:demo/session-cache");
    assert_eq!(system.gap, Some(Gap::Missing));
    assert!(system.declared);
    assert_eq!(node(&map, "ent:cif-0005").gap, Some(Gap::Missing));
    assert_eq!(
        node(&map, "ent:cif-0005").parent.as_deref(),
        Some("org:(unassigned)")
    );
}

#[test]
fn called_service_with_no_tested_system_is_a_gap() {
    let inputs = StarInputs {
        org_units: org(),
        services: vec![svc("checkout"), svc("ledger")],
        calls: vec![CallRow {
            client: "checkout".to_string(),
            server: "ledger".to_string(),
            calls: 40,
            errors: 2,
            avg_duration_ns: 1_500.0,
        }],
        ..StarInputs::default()
    };
    let map = build(&inputs);

    let calls = edge(&map, "svc:checkout", StarRel::Calls, "svc:ledger").expect("calls edge");
    assert_eq!(calls.source, EdgeSource::Observed);
    assert_eq!(calls.weight, Some(40));
    assert_eq!(calls.errors, Some(2));
    assert_eq!(node(&map, "svc:ledger").gap, Some(Gap::Missing));
    assert_eq!(node(&map, "svc:checkout").gap, None);
}

#[test]
fn evidence_counts_come_from_runs_and_roll_up_to_the_entity() {
    let inputs = StarInputs {
        org_units: org(),
        services: vec![ServiceRow {
            system: Some("message-queue".to_string()),
            ..svc_ent("mq", "cif-0004")
        }],
        systems: vec![SystemRow {
            experiments: vec![exp("queue-dependency-loss", false)],
            ..sys("message-queue", &[("dr", "covered")])
        }],
        runs: vec![
            run("r1", "queue-dependency-loss", "message-queue", true),
            RunRow {
                started_ns: 950,
                ..run("r2", "queue-dependency-loss", "message-queue", false)
            },
            RunRow {
                manual: true,
                ..run("m1", "tabletop", "message-queue", false)
            },
        ],
        ..StarInputs::default()
    };
    let map = build(&inputs);

    let system = node(&map, "sys:demo/message-queue");
    assert_eq!(system.evidence.runs, 2);
    assert_eq!(system.evidence.failed, 1);
    assert_eq!(system.evidence.manual, 1);
    assert_eq!(system.evidence.last_run_ns, Some(950));
    let entity = node(&map, "ent:cif-0004");
    assert_eq!(entity.evidence.runs, 2);
    assert_eq!(entity.evidence.failed, 1);
    assert_eq!(
        entity.evidence.by_discipline.get("dr"),
        Some(&"covered".to_string())
    );
    assert_eq!(entity.gap, None);
}

#[test]
fn definitions_are_tested_by_and_broke_edges_without_run_nodes() {
    let inputs = StarInputs {
        org_units: org(),
        systems: vec![SystemRow {
            experiments: vec![
                exp("queue-dependency-loss", false),
                exp("queue-failover", true),
            ],
            ..sys("message-queue", &[("dr", "covered")])
        }],
        definitions: vec![DefinitionRow {
            id: Some("def-7".to_string()),
            name: "queue-failover".to_string(),
        }],
        runs: vec![run("r1", "queue-dependency-loss", "message-queue", true)],
        covers: vec![CoverRow {
            from: CoverFrom::Experiment("queue-dependency-loss".to_string()),
            article: "compliance:DORA/Art.25".to_string(),
            evidenced: true,
        }],
        articles: vec![
            ArticleRow {
                id: "compliance:DORA/Art.25".to_string(),
                label: "DORA Art. 25".to_string(),
            },
            ArticleRow {
                id: "compliance:DORA/Art.11".to_string(),
                label: "DORA Art. 11".to_string(),
            },
        ],
        ..StarInputs::default()
    };
    let map = build(&inputs);

    let system = "sys:demo/message-queue";
    let tested = edge(&map, system, StarRel::TestedBy, "def:queue-dependency-loss")
        .expect("tested_by definition");
    assert_eq!(tested.source, EdgeSource::Evidence);
    assert!(edge(&map, "def:queue-dependency-loss", StarRel::Broke, system).is_some());
    assert!(edge(&map, system, StarRel::TestedBy, "def:def-7").is_some());
    assert!(edge(&map, "def:def-7", StarRel::Broke, system).is_none());
    assert_eq!(node(&map, "def:def-7").label, "queue-failover");
    assert_eq!(node(&map, "def:queue-dependency-loss").evidence.failed, 1);
    assert!(edge(
        &map,
        "def:queue-dependency-loss",
        StarRel::Covers,
        "compliance:DORA/Art.25"
    )
    .is_some());
    assert_eq!(node(&map, "compliance:DORA/Art.25").kind, StarKind::Article);
    assert!(!has_node(&map, "compliance:DORA/Art.11"));
    assert!(map.nodes.iter().all(|n| n.kind != StarKind::Run));
}

#[test]
fn include_runs_adds_run_and_manual_nodes() {
    let inputs = StarInputs {
        org_units: org(),
        systems: vec![sys("message-queue", &[("dr", "covered")])],
        runs: vec![
            run("r1", "queue-dependency-loss", "message-queue", true),
            RunRow {
                manual: true,
                ..run("m1", "tabletop", "message-queue", false)
            },
        ],
        covers: vec![CoverRow {
            from: CoverFrom::Manual("m1".to_string()),
            article: "compliance:DORA/Art.11".to_string(),
            evidenced: false,
        }],
        articles: vec![ArticleRow {
            id: "compliance:DORA/Art.11".to_string(),
            label: "DORA Art. 11".to_string(),
        }],
        ..StarInputs::default()
    };
    let query = StarQuery {
        include_runs: true,
        ..StarQuery::default()
    };
    let map = assemble(&inputs, &query);

    let system = "sys:demo/message-queue";
    assert_eq!(node(&map, "run:r1").kind, StarKind::Run);
    assert!(edge(&map, system, StarRel::TestedBy, "run:r1").is_some());
    assert!(edge(
        &map,
        "run:r1",
        StarRel::TestedBy,
        "def:queue-dependency-loss"
    )
    .is_some());
    assert!(edge(&map, "run:r1", StarRel::Broke, system).is_some());
    assert!(edge(&map, system, StarRel::TestedBy, "man:m1").is_some());
    let covers =
        edge(&map, "man:m1", StarRel::Covers, "compliance:DORA/Art.11").expect("manual covers");
    assert_eq!(covers.source, EdgeSource::Declared);
}

#[test]
fn all_suppressed_risks_report_zero_open() {
    let inputs = StarInputs {
        org_units: org(),
        systems: vec![
            sys("message-queue", &[("dr", "covered")]),
            sys("storage", &[("dr", "covered")]),
        ],
        risks: vec![
            risk("message-queue", "high", true),
            RiskRow {
                suppression: Some(SuppressionRow {
                    actor: "ada".to_string(),
                    reason: "later".to_string(),
                    expires_at_ns: Some(12_000),
                }),
                ..risk("message-queue", "medium", true)
            },
            risk("storage", "medium", false),
            risk("storage", "critical", false),
            risk("storage", "high", true),
        ],
        ..StarInputs::default()
    };
    let map = build(&inputs);

    let suppressed = &node(&map, "sys:demo/message-queue").risks;
    assert_eq!(suppressed.open, 0);
    assert_eq!(suppressed.suppressed, 2);
    assert_eq!(suppressed.worst, None);
    assert_eq!(suppressed.suppression_expires_ns, Some(9_000));
    assert_eq!(suppressed.suppressed_by.as_deref(), Some("morgan"));
    assert_eq!(
        suppressed.suppression_reason.as_deref(),
        Some("accepted until the migration")
    );
    let mixed = &node(&map, "sys:demo/storage").risks;
    assert_eq!(mixed.open, 2);
    assert_eq!(mixed.suppressed, 1);
    assert_eq!(mixed.worst.as_deref(), Some("critical"));
    assert_eq!(mixed.recommended, vec!["dr".to_string()]);
    assert_eq!(map.counts.risks_open, 2);
    assert_eq!(map.counts.risks_suppressed, 3);
}

#[test]
fn risks_attach_to_workloads_services_and_definitions_by_subject() {
    let inputs = StarInputs {
        org_units: org(),
        services: vec![svc("checkout"), svc("ledger")],
        systems: vec![sys("api-worker", &[("chaos", "covered")])],
        workloads: vec![workload("api-worker", &[])],
        definitions: vec![DefinitionRow {
            id: Some("def-9".to_string()),
            name: "never-run".to_string(),
        }],
        risks: vec![
            RiskRow {
                subject: Some("demo/api-worker".to_string()),
                ..risk("api-worker", "medium", false)
            },
            RiskRow {
                environment: None,
                ..risk("ledger", "high", false)
            },
            RiskRow {
                target: None,
                environment: None,
                subject: Some("def-9".to_string()),
                ..risk("", "low", false)
            },
        ],
        ..StarInputs::default()
    };
    let map = build(&inputs);

    assert_eq!(node(&map, "wl:demo/deployment/api-worker").risks.open, 1);
    assert_eq!(node(&map, "sys:demo/api-worker").risks.open, 0);
    assert_eq!(node(&map, "svc:ledger").risks.open, 1);
    assert_eq!(node(&map, "def:def-9").risks.open, 1);
}

#[test]
fn scope_hides_other_environments_and_environment_less_nodes() {
    let inputs = StarInputs {
        org_units: org(),
        services: vec![
            svc_ent("checkout", "cif-0042"),
            ServiceRow {
                environment: Some("production".to_string()),
                ..svc_ent("billing", "cif-0099")
            },
            ServiceRow {
                environment: None,
                ..svc("stray")
            },
        ],
        systems: vec![
            sys("shop", &[("chaos", "covered")]),
            SystemRow {
                environment: Some("production".to_string()),
                ..sys("vault", &[("chaos", "covered")])
            },
        ],
        runs: vec![RunRow {
            environment: Some("production".to_string()),
            ..run("r9", "vault-seal", "vault", true)
        }],
        workloads: vec![workload("orphan", &[])],
        ..StarInputs::default()
    };
    let query = StarQuery {
        env: Some("demo".to_string()),
        scope: Some(vec!["demo".to_string()]),
        ..StarQuery::default()
    };
    let map = assemble(&inputs, &query);

    assert!(has_node(&map, "svc:checkout"));
    assert!(has_node(&map, "ent:cif-0042"));
    assert!(has_node(&map, "sys:demo/shop"));
    for hidden in [
        "svc:billing",
        "ent:cif-0099",
        "sys:production/vault",
        "svc:stray",
        "def:vault-seal",
        "wl:demo/deployment/orphan",
    ] {
        assert!(!has_node(&map, hidden), "{hidden} leaked into scope demo");
    }
    assert!(map
        .nodes
        .iter()
        .all(|n| n.env.as_deref().is_none_or(|e| e == "demo")));
    assert_eq!(map.env.as_deref(), Some("demo"));

    let focused = assemble(
        &inputs,
        &StarQuery {
            focus: Some("sys:production/vault".to_string()),
            ..query
        },
    );
    assert!(focused.nodes.is_empty());
    assert!(focused.edges.is_empty());
}

fn chain() -> StarInputs {
    StarInputs {
        org_units: org(),
        entity_globs: vec![glob("cif-*", "platform/edge/edge-team")],
        services: vec![
            svc_ent("a", "cif-0001"),
            svc_ent("b", "cif-0002"),
            svc_ent("c", "cif-0003"),
            svc_ent("d", "cif-0004"),
        ],
        calls: vec![
            CallRow {
                client: "a".to_string(),
                server: "b".to_string(),
                calls: 1,
                ..CallRow::default()
            },
            CallRow {
                client: "b".to_string(),
                server: "c".to_string(),
                calls: 1,
                ..CallRow::default()
            },
            CallRow {
                client: "c".to_string(),
                server: "d".to_string(),
                calls: 1,
                ..CallRow::default()
            },
        ],
        ..StarInputs::default()
    }
}

#[test]
fn focus_keeps_the_neighbourhood_and_its_spine() {
    let inputs = chain();
    let query = StarQuery {
        focus: Some("svc:a".to_string()),
        depth: Some(1),
        ..StarQuery::default()
    };
    let map = assemble(&inputs, &query);

    // One hop: the callee and the entity; plus the spine up to the root.
    assert!(has_node(&map, "svc:a"));
    assert!(has_node(&map, "svc:b"));
    assert!(has_node(&map, "ent:cif-0001"));
    assert!(has_node(&map, "ent:cif-0002"));
    assert!(has_node(&map, "org:platform/edge/edge-team"));
    assert!(has_node(&map, "org:"));
    assert!(!has_node(&map, "svc:c"));
    assert!(!has_node(&map, "svc:d"));
    assert!(!has_node(&map, "org:platform/data"));
    assert_eq!(map.focus.as_deref(), Some("svc:a"));
    assert_eq!(map.depth, Some(1));
    assert!(map.counts.pruned > 0);
    assert!(map
        .edges
        .iter()
        .all(|e| has_node(&map, &e.from) && has_node(&map, &e.to)));
}

#[test]
fn focus_depth_defaults_to_two_and_widens_with_depth() {
    let inputs = chain();
    let default_depth = assemble(
        &inputs,
        &StarQuery {
            focus: Some("svc:a".to_string()),
            ..StarQuery::default()
        },
    );
    assert_eq!(default_depth.depth, Some(DEFAULT_DEPTH));
    assert!(has_node(&default_depth, "svc:c"));
    assert!(!has_node(&default_depth, "svc:d"));

    let wide = assemble(
        &inputs,
        &StarQuery {
            focus: Some("svc:a".to_string()),
            depth: Some(3),
            ..StarQuery::default()
        },
    );
    assert!(has_node(&wide, "svc:d"));
}

#[test]
fn unknown_focus_returns_an_empty_map() {
    let map = assemble(
        &chain(),
        &StarQuery {
            focus: Some("svc:nope".to_string()),
            ..StarQuery::default()
        },
    );
    assert!(map.nodes.is_empty());
    assert_eq!(map.counts.nodes, 0);
}

#[test]
fn kinds_filter_repoints_parents_to_the_nearest_kept_ancestor() {
    let map = assemble(
        &chain(),
        &StarQuery {
            kinds: Some(vec![StarKind::Org, StarKind::Service]),
            ..StarQuery::default()
        },
    );

    assert!(map
        .nodes
        .iter()
        .all(|n| matches!(n.kind, StarKind::Org | StarKind::Service)));
    assert_eq!(
        node(&map, "svc:a").parent.as_deref(),
        Some("org:platform/edge/edge-team")
    );
    assert_eq!(node(&map, "org:platform/edge/edge-team").children, 4);
    assert_eq!(map.counts.pruned, 4);
}

fn large() -> StarInputs {
    let mut inputs = StarInputs {
        org_units: org(),
        entity_globs: vec![
            glob("cif-000*", "platform/edge/edge-team"),
            glob("cif-001*", "platform/data/data-team"),
        ],
        ..StarInputs::default()
    };
    for i in 0..20 {
        let entity = format!("cif-{i:04}");
        let system = format!("system-{i:02}");
        for j in 0..3 {
            let name = format!("svc-{i:02}-{j}");
            inputs.services.push(ServiceRow {
                system: Some(system.clone()),
                ..svc_ent(&name, &entity)
            });
            inputs.workloads.push(workload(&name, &[("app", &name)]));
        }
        inputs.systems.push(SystemRow {
            experiments: vec![exp(&format!("exp-{i:02}"), i % 4 != 0)],
            ..sys(
                &system,
                &[("chaos", if i % 5 == 0 { "missing" } else { "covered" })],
            )
        });
        inputs.risks.push(risk(
            &system,
            if i % 2 == 0 { "high" } else { "low" },
            i % 3 == 0,
        ));
        inputs.calls.push(CallRow {
            client: format!("svc-{i:02}-0"),
            server: format!("svc-{:02}-1", (i + 1) % 20),
            calls: 5,
            errors: 1,
            avg_duration_ns: 100.0,
        });
    }
    inputs
}

#[test]
fn budget_clusters_the_deepest_rings_first() {
    let inputs = large();
    let full = build(&inputs);
    assert!(full.clusters.is_empty());
    assert_eq!(full.counts.clustered, 0);

    let query = StarQuery {
        budget: 120,
        ..StarQuery::default()
    };
    let map = assemble(&inputs, &query);

    assert!(map.nodes.len() <= 120, "{} nodes", map.nodes.len());
    assert_eq!(map.counts.nodes, map.nodes.len());
    assert_eq!(map.counts.edges, map.edges.len());
    assert!(!map.clusters.is_empty());
    assert_eq!(
        map.counts.clustered,
        map.clusters.iter().map(|c| c.members).sum::<usize>()
    );
    assert_eq!(
        full.nodes.len(),
        map.nodes.len() - map.clusters.len() + map.counts.clustered
    );
    // Satellites and workloads go before services and systems.
    assert!(map.nodes.iter().all(|n| n.kind != StarKind::Def));
    assert!(map.nodes.iter().any(|n| n.kind == StarKind::System));
    let cluster = node(&map, "cluster:org:platform/edge/edge-team");
    assert_eq!(cluster.kind, StarKind::Cluster);
    assert_eq!(
        cluster.parent.as_deref(),
        Some("org:platform/edge/edge-team")
    );
    assert_eq!(
        cluster.label,
        format!("edge-team ({})", cluster.members.unwrap_or_default())
    );
    assert!(map
        .edges
        .iter()
        .all(|e| has_node(&map, &e.from) && has_node(&map, &e.to) && e.from != e.to));
    assert!(map
        .nodes
        .iter()
        .all(|n| n.parent.as_deref().is_none_or(|p| has_node(&map, p))));
}

#[test]
fn clustering_merges_parallel_edges_and_keeps_the_worst_state() {
    let inputs = large();
    let map = assemble(
        &inputs,
        &StarQuery {
            budget: 12,
            ..StarQuery::default()
        },
    );

    let edge_team = "cluster:org:platform/edge/edge-team";
    let data_team = "cluster:org:platform/data/data-team";
    let merged = edge(&map, edge_team, StarRel::Calls, data_team).expect("merged calls edge");
    // Only svc-09-0 -> svc-10-1 crosses from the edge team to the data team.
    assert_eq!(merged.weight, Some(5));
    let summary = map
        .clusters
        .iter()
        .find(|c| c.id == edge_team)
        .expect("cluster summary");
    assert_eq!(summary.worst.gap, Some(Gap::Missing));
    assert_eq!(summary.worst.risk.as_deref(), Some("high"));
    assert_eq!(node(&map, edge_team).gap, Some(Gap::Missing));
    assert!(node(&map, edge_team).risks.suppressed > 0);
    assert!(map.nodes.iter().any(|n| n.kind == StarKind::Org));
}

#[test]
fn clustering_is_byte_identical_for_the_same_input() {
    let inputs = large();
    let query = StarQuery {
        budget: 60,
        ..StarQuery::default()
    };
    let first = serde_json::to_string(&assemble(&inputs, &query)).expect("serialise");
    let second = serde_json::to_string(&assemble(&inputs, &query)).expect("serialise");
    assert_eq!(first, second);
}

#[test]
fn output_is_stable_under_input_reordering() {
    let inputs = large();
    let mut shuffled = inputs.clone();
    shuffled.org_units.reverse();
    shuffled.services.reverse();
    shuffled.calls.reverse();
    shuffled.systems.reverse();
    shuffled.workloads.reverse();
    shuffled.risks.reverse();
    shuffled.services.rotate_left(7);
    for system in &mut shuffled.systems {
        system.disciplines.reverse();
        system.experiments.reverse();
    }

    for budget in [DEFAULT_BUDGET, 60] {
        let query = StarQuery {
            budget,
            ..StarQuery::default()
        };
        let a = serde_json::to_string(&assemble(&inputs, &query)).expect("serialise");
        let b = serde_json::to_string(&assemble(&shuffled, &query)).expect("serialise");
        assert_eq!(a, b, "budget {budget}");
    }
}

#[test]
fn nodes_and_edges_are_sorted_and_counted() {
    let map = build(&large());
    let node_ids: Vec<&str> = ids(&map);
    let mut sorted = node_ids.clone();
    sorted.sort_unstable();
    assert_eq!(node_ids, sorted);
    let edge_keys: Vec<(&str, StarRel, &str)> = map
        .edges
        .iter()
        .map(|e| (e.from.as_str(), e.rel, e.to.as_str()))
        .collect();
    let mut sorted_edges = edge_keys.clone();
    sorted_edges.sort_unstable();
    assert_eq!(edge_keys, sorted_edges);
    assert_eq!(map.counts.nodes, map.nodes.len());
    assert_eq!(map.counts.edges, map.edges.len());
}

#[test]
fn overlay_sources_that_do_not_exist_yet_are_unavailable_and_null() {
    let map = build(&large());
    assert_eq!(map.sources.slo, "unavailable");
    assert_eq!(map.sources.dora, "unavailable");
    assert_eq!(map.sources.maturity, "unavailable");
    assert_eq!(map.sources.graph, "ok");
    assert!(map.maturity.is_none());
    assert!(map.disclaimer.contains("EVIDENCE"));
    let json = serde_json::to_value(&map).expect("serialise");
    for n in json["nodes"].as_array().expect("nodes") {
        assert!(n["slo"].is_null());
        assert!(n["dora"].is_null());
        assert!(n["maturity"].is_null());
        assert_eq!(n["slo_count"], 0);
    }
    assert_eq!(json["nodes"][0]["id"], "def:exp-00");
    assert_eq!(json["nodes"][0]["kind"], "def");
    assert_eq!(json["edges"][0]["source"], "evidence");
}

#[test]
fn declared_dependency_links_services_and_keeps_unseen_ones() {
    let inputs = StarInputs {
        org_units: org(),
        services: vec![svc("checkout")],
        depends: vec![DependsRow {
            from: "checkout".to_string(),
            to: "ledger".to_string(),
        }],
        gaps: vec![GapRow {
            id: "gap:tumult-net::partition".to_string(),
            label: "tumult-net::partition".to_string(),
        }],
        ..StarInputs::default()
    };
    let map = build(&inputs);

    let depends =
        edge(&map, "svc:checkout", StarRel::DependsOn, "svc:ledger").expect("declared edge");
    assert_eq!(depends.source, EdgeSource::Declared);
    assert!(node(&map, "svc:ledger").declared);
    let gap = node(&map, "gap:tumult-net::partition");
    assert_eq!(gap.kind, StarKind::Gap);
    assert_eq!(gap.gap, Some(Gap::Missing));
}
