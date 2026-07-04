//! `OTel` instrumentation for TCP chaos-proxy operations.
//!
//! Span names use the crate domain prefix `net.*` (mirroring `k8s.*` and
//! `ssh.*`). The resilience experiment / action / probe / rollback spans are
//! emitted by `tumult-core` and become the parent of these spans automatically
//! through context attachment.

use opentelemetry::trace::TraceContextExt;
use opentelemetry::KeyValue;
use tumult_otel::{client_span, SpanGuard};

const TRACER: &str = "tumult-net";

/// Thin wrapper over [`tumult_otel::client_span`] that fixes the tracer name.
fn net_span(name: &'static str, attrs: Vec<KeyValue>) -> SpanGuard {
    client_span(TRACER, name, attrs)
}

// ── Actions ─────────────────────────────────────────────────

pub(crate) fn begin_start_proxy(listen: &str, upstream: &str) -> SpanGuard {
    net_span(
        "net.proxy.start",
        vec![
            KeyValue::new("net.listen.addr", listen.to_string()),
            KeyValue::new("net.upstream.addr", upstream.to_string()),
        ],
    )
}

pub(crate) fn begin_stop_proxy(listen: &str) -> SpanGuard {
    net_span(
        "net.proxy.stop",
        vec![KeyValue::new("net.listen.addr", listen.to_string())],
    )
}

pub(crate) fn begin_inject_latency(listen: &str, delay_ms: u64) -> SpanGuard {
    net_span(
        "net.latency.inject",
        vec![
            KeyValue::new("net.listen.addr", listen.to_string()),
            KeyValue::new(
                "net.latency.delay_ms",
                i64::try_from(delay_ms).unwrap_or(i64::MAX),
            ),
        ],
    )
}

pub(crate) fn begin_throttle_bandwidth(listen: &str, rate_bps: usize) -> SpanGuard {
    net_span(
        "net.bandwidth.throttle",
        vec![
            KeyValue::new("net.listen.addr", listen.to_string()),
            KeyValue::new(
                "net.bandwidth.rate_bps",
                i64::try_from(rate_bps).unwrap_or(i64::MAX),
            ),
        ],
    )
}

pub(crate) fn begin_fragment_stream(listen: &str, slice_bytes: usize) -> SpanGuard {
    net_span(
        "net.fragment.inject",
        vec![
            KeyValue::new("net.listen.addr", listen.to_string()),
            KeyValue::new(
                "net.fragment.slice_bytes",
                i64::try_from(slice_bytes).unwrap_or(i64::MAX),
            ),
        ],
    )
}

pub(crate) fn begin_corrupt_bytes(listen: &str, probability: f64) -> SpanGuard {
    net_span(
        "net.corrupt.inject",
        vec![
            KeyValue::new("net.listen.addr", listen.to_string()),
            KeyValue::new("net.corrupt.probability", probability),
        ],
    )
}

pub(crate) fn begin_terminate_connections(listen: &str, probability: f64) -> SpanGuard {
    net_span(
        "net.terminate.inject",
        vec![
            KeyValue::new("net.listen.addr", listen.to_string()),
            KeyValue::new("net.terminate.probability", probability),
        ],
    )
}

// ── Probes ──────────────────────────────────────────────────

pub(crate) fn begin_reachable(host: &str, port: u16) -> SpanGuard {
    net_span(
        "net.probe.reachable",
        vec![
            KeyValue::new("net.peer.name", host.to_string()),
            KeyValue::new("net.peer.port", i64::from(port)),
        ],
    )
}

pub(crate) fn begin_measured_latency(host: &str, port: u16) -> SpanGuard {
    net_span(
        "net.probe.latency",
        vec![
            KeyValue::new("net.peer.name", host.to_string()),
            KeyValue::new("net.peer.port", i64::from(port)),
        ],
    )
}

// ── Events ──────────────────────────────────────────────────

pub(crate) fn event_proxy_started(pid: u32) {
    let cx = opentelemetry::Context::current();
    cx.span().add_event(
        "net.proxy.started",
        vec![KeyValue::new("net.proxy.pid", i64::from(pid))],
    );
}

pub(crate) fn event_proxy_stopped(pid: Option<u32>) {
    let cx = opentelemetry::Context::current();
    let attrs = pid.map_or_else(Vec::new, |p| {
        vec![KeyValue::new("net.proxy.pid", i64::from(p))]
    });
    cx.span().add_event("net.proxy.stopped", attrs);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_spans_do_not_panic() {
        let _g = begin_start_proxy("127.0.0.1:8080", "10.0.0.1:80");
        let _g = begin_stop_proxy("127.0.0.1:8080");
        let _g = begin_inject_latency("127.0.0.1:8080", 100);
        let _g = begin_throttle_bandwidth("127.0.0.1:8080", 1024);
        let _g = begin_fragment_stream("127.0.0.1:8080", 64);
        let _g = begin_corrupt_bytes("127.0.0.1:8080", 0.01);
        let _g = begin_terminate_connections("127.0.0.1:8080", 0.05);
        event_proxy_started(4242);
        event_proxy_stopped(Some(4242));
        event_proxy_stopped(None);
    }

    #[test]
    fn probe_spans_do_not_panic() {
        let _g = begin_reachable("example.com", 443);
        let _g = begin_measured_latency("example.com", 443);
    }
}
