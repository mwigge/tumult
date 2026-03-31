//! OTel instrumentation for Kubernetes operations.

use opentelemetry::trace::{SpanKind, TraceContextExt, Tracer};
use opentelemetry::{global, KeyValue};

const TRACER: &str = "tumult-kubernetes";

pub struct SpanGuard {
    _guard: opentelemetry::ContextGuard,
}

fn k8s_span(name: &str, attrs: Vec<KeyValue>) -> SpanGuard {
    let tracer = global::tracer(TRACER);
    let span = tracer
        .span_builder(name.to_string())
        .with_kind(SpanKind::Client)
        .with_attributes(attrs)
        .start(&tracer);
    let cx = opentelemetry::Context::current_with_span(span);
    SpanGuard {
        _guard: cx.attach(),
    }
}

// ── Actions ─────────────────────────────────────────────────

pub fn begin_delete_pod(namespace: &str, name: &str) -> SpanGuard {
    k8s_span(
        "k8s.pod.delete",
        vec![
            KeyValue::new("k8s.namespace.name", namespace.to_string()),
            KeyValue::new("k8s.pod.name", name.to_string()),
        ],
    )
}

pub fn begin_scale_deployment(namespace: &str, name: &str, replicas: i32) -> SpanGuard {
    k8s_span(
        "k8s.deployment.scale",
        vec![
            KeyValue::new("k8s.namespace.name", namespace.to_string()),
            KeyValue::new("k8s.deployment.name", name.to_string()),
            KeyValue::new("k8s.deployment.replicas.desired", i64::from(replicas)),
        ],
    )
}

pub fn begin_cordon_node(name: &str) -> SpanGuard {
    k8s_span(
        "k8s.node.cordon",
        vec![KeyValue::new("k8s.node.name", name.to_string())],
    )
}

pub fn begin_drain_node(name: &str, grace_period: Option<u32>) -> SpanGuard {
    let mut attrs = vec![KeyValue::new("k8s.node.name", name.to_string())];
    if let Some(gp) = grace_period {
        attrs.push(KeyValue::new(
            "k8s.delete.grace_period_seconds",
            i64::from(gp),
        ));
    }
    k8s_span("k8s.node.drain", attrs)
}

pub fn begin_apply_network_policy(namespace: &str, policy_name: &str) -> SpanGuard {
    k8s_span(
        "k8s.networkpolicy.apply",
        vec![
            KeyValue::new("k8s.namespace.name", namespace.to_string()),
            KeyValue::new("k8s.networkpolicy.name", policy_name.to_string()),
        ],
    )
}

pub fn begin_delete_network_policy(namespace: &str, name: &str) -> SpanGuard {
    k8s_span(
        "k8s.networkpolicy.delete",
        vec![
            KeyValue::new("k8s.namespace.name", namespace.to_string()),
            KeyValue::new("k8s.networkpolicy.name", name.to_string()),
        ],
    )
}

// ── Probes ──────────────────────────────────────────────────

pub fn begin_pod_probe(namespace: &str, name: &str) -> SpanGuard {
    k8s_span(
        "k8s.pod.is_ready",
        vec![
            KeyValue::new("k8s.namespace.name", namespace.to_string()),
            KeyValue::new("k8s.pod.name", name.to_string()),
        ],
    )
}

pub fn begin_pods_by_label(namespace: &str, selector: &str) -> SpanGuard {
    k8s_span(
        "k8s.pods.list_by_label",
        vec![
            KeyValue::new("k8s.namespace.name", namespace.to_string()),
            KeyValue::new("k8s.label_selector", selector.to_string()),
        ],
    )
}

pub fn begin_deployment_probe(namespace: &str, name: &str) -> SpanGuard {
    k8s_span(
        "k8s.deployment.is_ready",
        vec![
            KeyValue::new("k8s.namespace.name", namespace.to_string()),
            KeyValue::new("k8s.deployment.name", name.to_string()),
        ],
    )
}

pub fn begin_node_status(name: &str) -> SpanGuard {
    k8s_span(
        "k8s.node.status",
        vec![KeyValue::new("k8s.node.name", name.to_string())],
    )
}

// ── Events ──────────────────────────────────────────────────

pub fn event_drain_completed(evicted: usize, failed: usize, skipped_daemonsets: usize) {
    let cx = opentelemetry::Context::current();
    cx.span().add_event(
        "k8s.drain.completed",
        vec![
            KeyValue::new("k8s.pods.evicted", evicted as i64),
            KeyValue::new("k8s.pods.failed", failed as i64),
            KeyValue::new("k8s.daemonsets.skipped", skipped_daemonsets as i64),
        ],
    );
}

pub fn event_pod_evicted(pod_name: &str) {
    let cx = opentelemetry::Context::current();
    cx.span().add_event(
        "k8s.pod.evicted",
        vec![KeyValue::new("k8s.pod.name", pod_name.to_string())],
    );
}

pub fn event_pods_counted(count: usize) {
    let cx = opentelemetry::Context::current();
    cx.span().add_event(
        "k8s.pods.counted",
        vec![KeyValue::new("k8s.pods.count", count as i64)],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_spans_do_not_panic() {
        let _g = begin_delete_pod("default", "my-pod");
        let _g = begin_scale_deployment("default", "my-deploy", 3);
        let _g = begin_cordon_node("node-1");
        let _g = begin_drain_node("node-1", Some(30));
        event_drain_completed(5, 1, 2);
        event_pod_evicted("pod-a");
    }

    #[test]
    fn probe_spans_do_not_panic() {
        let _g = begin_pod_probe("default", "my-pod");
        let _g = begin_pods_by_label("default", "app=web");
        event_pods_counted(3);
        let _g = begin_deployment_probe("default", "my-deploy");
        let _g = begin_node_status("node-1");
    }

    #[test]
    fn network_policy_spans_do_not_panic() {
        let _g = begin_apply_network_policy("default", "deny-all");
        let _g = begin_delete_network_policy("default", "deny-all");
    }
}
