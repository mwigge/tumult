//! `OTel` instrumentation for Kubernetes operations.

use opentelemetry::trace::TraceContextExt;
use opentelemetry::KeyValue;
use tumult_otel::{client_span, SpanGuard};

const TRACER: &str = "tumult-kubernetes";

/// Thin wrapper over [`tumult_otel::client_span`] that fixes the tracer name.
fn k8s_span(name: &'static str, attrs: Vec<KeyValue>) -> SpanGuard {
    client_span(TRACER, name, attrs)
}

// ── Actions ─────────────────────────────────────────────────

pub(crate) fn begin_delete_pod(namespace: &str, name: &str) -> SpanGuard {
    k8s_span(
        "k8s.pod.delete",
        vec![
            KeyValue::new("k8s.namespace.name", namespace.to_string()),
            KeyValue::new("k8s.pod.name", name.to_string()),
        ],
    )
}

pub(crate) fn begin_scale_deployment(namespace: &str, name: &str, replicas: i32) -> SpanGuard {
    k8s_span(
        "k8s.deployment.scale",
        vec![
            KeyValue::new("k8s.namespace.name", namespace.to_string()),
            KeyValue::new("k8s.deployment.name", name.to_string()),
            KeyValue::new("k8s.deployment.replicas.desired", i64::from(replicas)),
        ],
    )
}

pub(crate) fn begin_cordon_node(name: &str) -> SpanGuard {
    k8s_span(
        "k8s.node.cordon",
        vec![KeyValue::new("k8s.node.name", name.to_string())],
    )
}

pub(crate) fn begin_uncordon_node(name: &str) -> SpanGuard {
    k8s_span(
        "k8s.node.uncordon",
        vec![KeyValue::new("k8s.node.name", name.to_string())],
    )
}

pub(crate) fn begin_drain_node(name: &str, grace_period: Option<u32>) -> SpanGuard {
    let mut attrs = vec![KeyValue::new("k8s.node.name", name.to_string())];
    if let Some(gp) = grace_period {
        attrs.push(KeyValue::new(
            "k8s.delete.grace_period_seconds",
            i64::from(gp),
        ));
    }
    k8s_span("k8s.node.drain", attrs)
}

pub(crate) fn begin_apply_network_policy(namespace: &str, policy_name: &str) -> SpanGuard {
    k8s_span(
        "k8s.networkpolicy.apply",
        vec![
            KeyValue::new("k8s.namespace.name", namespace.to_string()),
            KeyValue::new("k8s.networkpolicy.name", policy_name.to_string()),
        ],
    )
}

pub(crate) fn begin_delete_network_policy(namespace: &str, name: &str) -> SpanGuard {
    k8s_span(
        "k8s.networkpolicy.delete",
        vec![
            KeyValue::new("k8s.namespace.name", namespace.to_string()),
            KeyValue::new("k8s.networkpolicy.name", name.to_string()),
        ],
    )
}

// ── In-pod data-plane injection ─────────────────────────────

pub(crate) fn begin_pod_network_latency(
    namespace: &str,
    pod: &str,
    container: &str,
    delay_ms: u32,
    duration_s: u32,
) -> SpanGuard {
    k8s_span(
        "k8s.pod.network_latency",
        vec![
            KeyValue::new("k8s.namespace.name", namespace.to_string()),
            KeyValue::new("k8s.pod.name", pod.to_string()),
            KeyValue::new("k8s.ephemeral_container.name", container.to_string()),
            KeyValue::new("tumult.fault.delay_ms", i64::from(delay_ms)),
            KeyValue::new("tumult.fault.duration_s", i64::from(duration_s)),
        ],
    )
}

pub(crate) fn begin_pod_stress(
    namespace: &str,
    pod: &str,
    container: &str,
    summary: &str,
    duration_s: u32,
) -> SpanGuard {
    k8s_span(
        "k8s.pod.stress",
        vec![
            KeyValue::new("k8s.namespace.name", namespace.to_string()),
            KeyValue::new("k8s.pod.name", pod.to_string()),
            KeyValue::new("k8s.ephemeral_container.name", container.to_string()),
            KeyValue::new("tumult.fault.stress", summary.to_string()),
            KeyValue::new("tumult.fault.duration_s", i64::from(duration_s)),
        ],
    )
}

// ── Probes ──────────────────────────────────────────────────

pub(crate) fn begin_pod_probe(namespace: &str, name: &str) -> SpanGuard {
    k8s_span(
        "k8s.pod.is_ready",
        vec![
            KeyValue::new("k8s.namespace.name", namespace.to_string()),
            KeyValue::new("k8s.pod.name", name.to_string()),
        ],
    )
}

pub(crate) fn begin_pods_by_label(namespace: &str, selector: &str) -> SpanGuard {
    k8s_span(
        "k8s.pods.list_by_label",
        vec![
            KeyValue::new("k8s.namespace.name", namespace.to_string()),
            KeyValue::new("k8s.label_selector", selector.to_string()),
        ],
    )
}

pub(crate) fn begin_deployment_probe(namespace: &str, name: &str) -> SpanGuard {
    k8s_span(
        "k8s.deployment.is_ready",
        vec![
            KeyValue::new("k8s.namespace.name", namespace.to_string()),
            KeyValue::new("k8s.deployment.name", name.to_string()),
        ],
    )
}

pub(crate) fn begin_discover_services(namespace_count: usize) -> SpanGuard {
    k8s_span(
        "k8s.services.discover",
        vec![KeyValue::new(
            "k8s.namespaces.requested",
            i64::try_from(namespace_count).unwrap_or(i64::MAX),
        )],
    )
}

pub(crate) fn begin_node_status(name: &str) -> SpanGuard {
    k8s_span(
        "k8s.node.status",
        vec![KeyValue::new("k8s.node.name", name.to_string())],
    )
}

// ── Events ──────────────────────────────────────────────────

pub(crate) fn event_drain_completed(evicted: usize, failed: usize, skipped_daemonsets: usize) {
    let cx = opentelemetry::Context::current();
    cx.span().add_event(
        "drain.completed",
        vec![
            KeyValue::new(
                "k8s.pods.evicted",
                i64::try_from(evicted).unwrap_or(i64::MAX),
            ),
            KeyValue::new("k8s.pods.failed", i64::try_from(failed).unwrap_or(i64::MAX)),
            KeyValue::new(
                "k8s.daemonsets.skipped",
                i64::try_from(skipped_daemonsets).unwrap_or(i64::MAX),
            ),
        ],
    );
}

// Retained for future integration with eviction probes/actions; not yet called from an active code path.
#[allow(dead_code)] // Intentional: reserved for upcoming eviction probe integration; not yet reachable.
pub(crate) fn event_pod_evicted(pod_name: &str) {
    let cx = opentelemetry::Context::current();
    cx.span().add_event(
        "k8s.pod.evicted",
        vec![KeyValue::new("k8s.pod.name", pod_name.to_string())],
    );
}

// Retained for future integration with pod-count probes; not yet called from an active code path.
#[allow(dead_code)] // Intentional: reserved for upcoming pod-count probe integration; not yet reachable.
pub(crate) fn event_pods_counted(count: usize) {
    let cx = opentelemetry::Context::current();
    cx.span().add_event(
        "k8s.pods.counted",
        vec![KeyValue::new(
            "k8s.pods.count",
            i64::try_from(count).unwrap_or(i64::MAX),
        )],
    );
}

#[cfg(test)]
mod tests {
    use opentelemetry::trace::SpanKind;
    use tumult_test_utils::capture_spans;

    use super::*;

    fn kv(key: &'static str, value: impl Into<opentelemetry::Value>) -> KeyValue {
        KeyValue::new(key, value)
    }

    /// Runs `begin` and checks the one client span it produced.
    fn assert_span(begin: impl FnOnce() -> SpanGuard, name: &str, attributes: &[KeyValue]) {
        let ((), spans) = capture_spans(|| drop(begin()));
        assert_eq!(spans.len(), 1, "{name}");
        assert_eq!(spans[0].name, name);
        assert_eq!(spans[0].span_kind, SpanKind::Client, "{name}");
        assert_eq!(spans[0].attributes, attributes, "{name}");
    }

    #[test]
    fn action_spans_name_the_operation_and_its_target() {
        let namespace = || kv("k8s.namespace.name", "prod");
        assert_span(
            || begin_delete_pod("prod", "api-0"),
            "k8s.pod.delete",
            &[namespace(), kv("k8s.pod.name", "api-0")],
        );
        assert_span(
            || begin_scale_deployment("prod", "web", 3),
            "k8s.deployment.scale",
            &[
                namespace(),
                kv("k8s.deployment.name", "web"),
                kv("k8s.deployment.replicas.desired", 3_i64),
            ],
        );
        assert_span(
            || begin_cordon_node("worker-1"),
            "k8s.node.cordon",
            &[kv("k8s.node.name", "worker-1")],
        );
        assert_span(
            || begin_uncordon_node("worker-1"),
            "k8s.node.uncordon",
            &[kv("k8s.node.name", "worker-1")],
        );
        assert_span(
            || begin_apply_network_policy("prod", "deny-all"),
            "k8s.networkpolicy.apply",
            &[namespace(), kv("k8s.networkpolicy.name", "deny-all")],
        );
        assert_span(
            || begin_delete_network_policy("prod", "deny-all"),
            "k8s.networkpolicy.delete",
            &[namespace(), kv("k8s.networkpolicy.name", "deny-all")],
        );
    }

    #[test]
    fn drain_span_records_grace_period_only_when_set_and_the_completion_counts() {
        assert_span(
            || begin_drain_node("worker-1", None),
            "k8s.node.drain",
            &[kv("k8s.node.name", "worker-1")],
        );

        let ((), spans) = capture_spans(|| {
            let _span = begin_drain_node("worker-1", Some(30));
            event_drain_completed(5, 1, 2);
        });

        assert_eq!(spans.len(), 1);
        assert_eq!(
            spans[0].attributes,
            [
                kv("k8s.node.name", "worker-1"),
                kv("k8s.delete.grace_period_seconds", 30_i64),
            ]
        );
        let events = &spans[0].events.events;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].name, "drain.completed");
        assert_eq!(
            events[0].attributes,
            [
                kv("k8s.pods.evicted", 5_i64),
                kv("k8s.pods.failed", 1_i64),
                kv("k8s.daemonsets.skipped", 2_i64),
            ]
        );
    }

    #[test]
    fn probe_spans_name_the_query_and_its_target() {
        let namespace = || kv("k8s.namespace.name", "prod");
        assert_span(
            || begin_pod_probe("prod", "api-0"),
            "k8s.pod.is_ready",
            &[namespace(), kv("k8s.pod.name", "api-0")],
        );
        assert_span(
            || begin_pods_by_label("prod", "app=web"),
            "k8s.pods.list_by_label",
            &[namespace(), kv("k8s.label_selector", "app=web")],
        );
        assert_span(
            || begin_deployment_probe("prod", "web"),
            "k8s.deployment.is_ready",
            &[namespace(), kv("k8s.deployment.name", "web")],
        );
        assert_span(
            || begin_node_status("worker-1"),
            "k8s.node.status",
            &[kv("k8s.node.name", "worker-1")],
        );
        assert_span(
            || begin_discover_services(2),
            "k8s.services.discover",
            &[kv("k8s.namespaces.requested", 2_i64)],
        );
    }

    #[test]
    fn injection_spans_carry_the_target_container_and_fault_parameters() {
        let target = |container: &'static str| {
            [
                kv("k8s.namespace.name", "prod"),
                kv("k8s.pod.name", "web-0"),
                kv("k8s.ephemeral_container.name", container),
            ]
        };
        assert_span(
            || begin_pod_network_latency("prod", "web-0", "tumult-netem-0", 100, 30),
            "k8s.pod.network_latency",
            &[
                target("tumult-netem-0").as_slice(),
                &[
                    kv("tumult.fault.delay_ms", 100_i64),
                    kv("tumult.fault.duration_s", 30_i64),
                ],
            ]
            .concat(),
        );
        assert_span(
            || begin_pod_stress("prod", "web-0", "tumult-stress-0", "2 CPU workers", 45),
            "k8s.pod.stress",
            &[
                target("tumult-stress-0").as_slice(),
                &[
                    kv("tumult.fault.stress", "2 CPU workers"),
                    kv("tumult.fault.duration_s", 45_i64),
                ],
            ]
            .concat(),
        );
    }
}
