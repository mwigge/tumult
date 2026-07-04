//! Mapping a [`Journal`] (optionally enriched with its [`Experiment`]) into a
//! deduplicated [`GraphDelta`].
//!
//! The mapping is deterministic and idempotent: the same inputs always produce
//! the same nodes/edges, and no node/edge appears twice in the returned delta.
//!
//! Two levels of fidelity are supported:
//!
//! * **Journal + Experiment** (the richest, used by `tumult_run_experiment`):
//!   faults are `plugin::function`, services are extracted from the injecting
//!   activity's provider arguments, and `Fault -[observed_on]-> Service` edges
//!   are added.
//! * **Journal only** (the CLI auto-ingest path): faults fall back to the
//!   injecting activity's name; no service/target information is available.

use std::collections::HashMap;

use tumult_core::types::{ActivityType, Experiment, ExperimentStatus, Journal, Provider};

use crate::model::{Edge, EdgeRel, GraphDelta, Node, NodeKind};

/// Turn a run into its graph delta. Pass `Some(experiment)` for the full
/// `Fault = plugin::function` + `Service` model; pass `None` for the
/// journal-only fallback (faults keyed by injecting-activity name).
#[must_use]
pub fn journal_to_graph(journal: &Journal, experiment: Option<&Experiment>) -> GraphDelta {
    let mut builder = Builder::default();

    // ── Experiment node (stable identity by title) ───────────────
    let exp_id = format!("exp:{}", journal.experiment_title);
    builder.push_node(Node {
        id: exp_id.clone(),
        kind: NodeKind::Experiment,
        label: journal.experiment_title.clone(),
        attrs: serde_json::json!({}),
    });

    // ── Journal (run) node + Experiment -[yielded]-> Journal ─────
    let run_id = format!("run:{}", journal.experiment_id);
    let status = journal.status.to_string();
    builder.push_node(Node {
        id: run_id.clone(),
        kind: NodeKind::Journal,
        label: status.clone(),
        attrs: serde_json::json!({
            "status": status,
            "started_at_ns": journal.started_at_ns,
            "duration_ms": journal.duration_ms,
        }),
    });
    builder.push_edge(&exp_id, EdgeRel::Yielded, &run_id);

    // ── Deviation node when the run did not complete cleanly ─────
    if journal.status != ExperimentStatus::Completed {
        let dev_id = format!("dev:{}", journal.experiment_id);
        builder.push_node(Node {
            id: dev_id.clone(),
            kind: NodeKind::Deviation,
            label: journal.status.to_string(),
            attrs: serde_json::json!({ "status": journal.status.to_string() }),
        });
        builder.push_edge(&run_id, EdgeRel::Exhibited, &dev_id);
    }

    // ── Faults + services ────────────────────────────────────────
    match experiment {
        Some(exp) => map_from_experiment(&mut builder, &exp_id, exp),
        None => map_from_journal(&mut builder, &exp_id, journal),
    }

    builder.finish()
}

/// Richest path: derive `Fault = plugin::function` and `Service` nodes from the
/// experiment's action activities (native providers).
fn map_from_experiment(builder: &mut Builder, exp_id: &str, exp: &Experiment) {
    for activity in &exp.method {
        if activity.activity_type != ActivityType::Action {
            continue;
        }
        let Provider::Native {
            plugin,
            function,
            arguments,
        } = &activity.provider
        else {
            continue;
        };
        let fault_label = format!("{plugin}::{function}");
        let fault_id = format!("fault:{fault_label}");
        builder.push_node(Node {
            id: fault_id.clone(),
            kind: NodeKind::Fault,
            label: fault_label,
            attrs: serde_json::json!({ "plugin": plugin, "function": function }),
        });
        builder.push_edge(exp_id, EdgeRel::Injects, &fault_id);

        if let Some(service) = service_from_arguments(arguments) {
            let svc_id = format!("svc:{service}");
            builder.push_node(Node {
                id: svc_id.clone(),
                kind: NodeKind::Service,
                label: service,
                attrs: serde_json::json!({}),
            });
            builder.push_edge(exp_id, EdgeRel::Targets, &svc_id);
            builder.push_edge(&fault_id, EdgeRel::ObservedOn, &svc_id);
        }
    }
}

/// Journal-only fallback: faults are the names of the action method results.
fn map_from_journal(builder: &mut Builder, exp_id: &str, journal: &Journal) {
    for result in &journal.method_results {
        if result.activity_type != ActivityType::Action {
            continue;
        }
        let fault_id = format!("fault:{}", result.name);
        builder.push_node(Node {
            id: fault_id.clone(),
            kind: NodeKind::Fault,
            label: result.name.clone(),
            attrs: serde_json::json!({ "activity": result.name }),
        });
        builder.push_edge(exp_id, EdgeRel::Injects, &fault_id);
    }
}

/// Argument keys, in priority order, that name the service/target a native
/// fault acts on. The first present string value wins.
const SERVICE_ARG_KEYS: &[&str] = &[
    "upstream", "target", "host", "service", "endpoint", "address", "url", "pod",
];

/// Extract a short service name from a native provider's arguments, stripping a
/// URL scheme, path, and `:port` suffix so `http://demo-app:8080/health`,
/// `demo-app:8080`, and `demo-app` all collapse to `demo-app`.
fn service_from_arguments(arguments: &HashMap<String, serde_json::Value>) -> Option<String> {
    let raw = SERVICE_ARG_KEYS
        .iter()
        .find_map(|key| arguments.get(*key).and_then(serde_json::Value::as_str))?;
    let host = normalize_service(raw);
    (!host.is_empty()).then_some(host)
}

/// Reduce a raw target string to a bare host name.
fn normalize_service(raw: &str) -> String {
    // Drop a scheme (`http://`, `tcp://`, …).
    let no_scheme = raw.split_once("://").map_or(raw, |(_, rest)| rest);
    // Drop any path/query.
    let authority = no_scheme.split(['/', '?']).next().unwrap_or(no_scheme);
    // Drop a `:port` suffix.
    let host = authority
        .rsplit_once(':')
        .map_or(authority, |(host, port)| {
            if port.chars().all(|c| c.is_ascii_digit()) && !port.is_empty() {
                host
            } else {
                authority
            }
        });
    host.trim().to_string()
}

/// Accumulates nodes/edges while deduplicating by id / `(src, rel, dst)`.
#[derive(Default)]
struct Builder {
    nodes: Vec<Node>,
    seen_nodes: std::collections::HashSet<String>,
    edges: Vec<Edge>,
    seen_edges: std::collections::HashSet<(String, &'static str, String)>,
}

impl Builder {
    fn push_node(&mut self, node: Node) {
        if self.seen_nodes.insert(node.id.clone()) {
            self.nodes.push(node);
        }
    }

    fn push_edge(&mut self, src: &str, rel: EdgeRel, dst: &str) {
        let key = (src.to_string(), rel.as_str(), dst.to_string());
        if self.seen_edges.insert(key) {
            self.edges.push(Edge {
                src: src.to_string(),
                rel,
                dst: dst.to_string(),
            });
        }
    }

    fn finish(self) -> GraphDelta {
        GraphDelta {
            nodes: self.nodes,
            edges: self.edges,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tumult_core::types::{Activity, ActivityResult, ActivityStatus, ExperimentStatus, Journal};

    fn base_journal(id: &str, status: ExperimentStatus) -> Journal {
        Journal {
            experiment_title: "Latency drill".into(),
            experiment_id: id.into(),
            status,
            started_at_ns: 1_774_980_000_000_000_000,
            ended_at_ns: 1_774_980_300_000_000_000,
            duration_ms: 300_000,
            steady_state_before: None,
            steady_state_after: None,
            method_results: vec![ActivityResult {
                name: "inject-latency".into(),
                activity_type: ActivityType::Action,
                status: ActivityStatus::Succeeded,
                started_at_ns: 1_774_980_135_000_000_000,
                duration_ms: 500,
                output: Some("proxy up".into()),
                error: None,
                trace_id: "t1".into(),
                span_id: "s1".into(),
            }],
            rollback_results: vec![],
            rollback_failures: 0,
            estimate: None,
            baseline_result: None,
            during_result: None,
            post_result: None,
            load_result: None,
            analysis: None,
            regulatory: None,
            halt: None,
            blast_radius: None,
        }
    }

    fn latency_experiment() -> Experiment {
        Experiment {
            title: "Latency drill".into(),
            method: vec![Activity {
                name: "inject-latency".into(),
                activity_type: ActivityType::Action,
                provider: Provider::Native {
                    plugin: "tumult-net".into(),
                    function: "inject_latency".into(),
                    arguments: HashMap::from([(
                        "upstream".into(),
                        serde_json::Value::String("demo-app:8080".into()),
                    )]),
                },
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn node_ids(delta: &GraphDelta) -> Vec<&str> {
        delta.nodes.iter().map(|n| n.id.as_str()).collect()
    }

    fn edge_tuples(delta: &GraphDelta) -> Vec<(String, &'static str, String)> {
        delta
            .edges
            .iter()
            .map(|e| (e.src.clone(), e.rel.as_str(), e.dst.clone()))
            .collect()
    }

    #[test]
    fn journal_with_experiment_produces_expected_kinds_and_edges() {
        let journal = base_journal("run-1", ExperimentStatus::Completed);
        let exp = latency_experiment();
        let delta = journal_to_graph(&journal, Some(&exp));

        let ids = node_ids(&delta);
        assert!(ids.contains(&"exp:Latency drill"));
        assert!(ids.contains(&"fault:tumult-net::inject_latency"));
        assert!(ids.contains(&"svc:demo-app"));
        assert!(ids.contains(&"run:run-1"));
        // Completed run → no deviation node.
        assert!(!ids.iter().any(|id| id.starts_with("dev:")));

        let edges = edge_tuples(&delta);
        assert!(edges.contains(&(
            "exp:Latency drill".into(),
            "injects",
            "fault:tumult-net::inject_latency".into()
        )));
        assert!(edges.contains(&("exp:Latency drill".into(), "targets", "svc:demo-app".into())));
        assert!(edges.contains(&("exp:Latency drill".into(), "yielded", "run:run-1".into())));
        assert!(edges.contains(&(
            "fault:tumult-net::inject_latency".into(),
            "observed_on",
            "svc:demo-app".into()
        )));
    }

    #[test]
    fn deviated_run_adds_deviation_node_and_edge() {
        let journal = base_journal("run-2", ExperimentStatus::Deviated);
        let exp = latency_experiment();
        let delta = journal_to_graph(&journal, Some(&exp));

        assert!(node_ids(&delta).contains(&"dev:run-2"));
        assert!(edge_tuples(&delta).contains(&(
            "run:run-2".into(),
            "exhibited",
            "dev:run-2".into()
        )));
    }

    #[test]
    fn journal_only_falls_back_to_activity_name_faults() {
        let journal = base_journal("run-3", ExperimentStatus::Completed);
        let delta = journal_to_graph(&journal, None);

        let ids = node_ids(&delta);
        assert!(ids.contains(&"fault:inject-latency"));
        // No experiment → no service/target information.
        assert!(!ids.iter().any(|id| id.starts_with("svc:")));
        assert!(edge_tuples(&delta).contains(&(
            "exp:Latency drill".into(),
            "injects",
            "fault:inject-latency".into()
        )));
    }

    #[test]
    fn mapping_is_idempotent_and_deduplicated() {
        let journal = base_journal("run-4", ExperimentStatus::Completed);
        let exp = latency_experiment();
        let a = journal_to_graph(&journal, Some(&exp));
        let b = journal_to_graph(&journal, Some(&exp));
        assert_eq!(a, b);

        // No duplicate node ids, no duplicate edges.
        let mut ids = node_ids(&a);
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "node ids must be unique");

        let mut edges = edge_tuples(&a);
        let ecount = edges.len();
        edges.sort();
        edges.dedup();
        assert_eq!(edges.len(), ecount, "edges must be unique");
    }

    #[test]
    fn service_extraction_strips_scheme_path_and_port() {
        assert_eq!(normalize_service("http://demo-app:8080/health"), "demo-app");
        assert_eq!(normalize_service("demo-app:8080"), "demo-app");
        assert_eq!(normalize_service("demo-app"), "demo-app");
        assert_eq!(normalize_service("10.0.0.1:5432"), "10.0.0.1");
        // A non-numeric suffix is kept (not a port).
        assert_eq!(normalize_service("cache:main"), "cache:main");
    }
}
