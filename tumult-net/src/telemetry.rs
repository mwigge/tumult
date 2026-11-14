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
    use opentelemetry::trace::SpanKind;
    use tumult_test_utils::capture_spans;

    use super::*;

    const LISTEN: &str = "127.0.0.1:8080";

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
    fn fault_spans_name_the_fault_and_carry_its_parameter() {
        let listen = || kv("net.listen.addr", LISTEN);
        assert_span(
            || begin_inject_latency(LISTEN, 100),
            "net.latency.inject",
            &[listen(), kv("net.latency.delay_ms", 100_i64)],
        );
        assert_span(
            || begin_throttle_bandwidth(LISTEN, 1024),
            "net.bandwidth.throttle",
            &[listen(), kv("net.bandwidth.rate_bps", 1024_i64)],
        );
        assert_span(
            || begin_fragment_stream(LISTEN, 64),
            "net.fragment.inject",
            &[listen(), kv("net.fragment.slice_bytes", 64_i64)],
        );
        assert_span(
            || begin_corrupt_bytes(LISTEN, 0.01),
            "net.corrupt.inject",
            &[listen(), kv("net.corrupt.probability", 0.01)],
        );
        assert_span(
            || begin_terminate_connections(LISTEN, 0.05),
            "net.terminate.inject",
            &[listen(), kv("net.terminate.probability", 0.05)],
        );
    }

    #[test]
    fn proxy_lifecycle_spans_record_addresses_and_the_daemon_pid() {
        let ((), spans) = capture_spans(|| {
            {
                let _span = begin_start_proxy(LISTEN, "10.0.0.1:80");
                event_proxy_started(4242);
            }
            {
                let _span = begin_stop_proxy(LISTEN);
                event_proxy_stopped(Some(4242));
            }
            let _span = begin_stop_proxy(LISTEN);
            event_proxy_stopped(None);
        });

        let summary: Vec<_> = spans
            .iter()
            .map(|span| {
                let events: Vec<_> = span
                    .events
                    .events
                    .iter()
                    .map(|event| (event.name.as_ref(), event.attributes.as_slice()))
                    .collect();
                (span.name.as_ref(), span.attributes.as_slice(), events)
            })
            .collect();
        let pid = [kv("net.proxy.pid", 4242_i64)];
        let listen = [kv("net.listen.addr", LISTEN)];
        assert_eq!(
            summary,
            [
                (
                    "net.proxy.start",
                    [
                        kv("net.listen.addr", LISTEN),
                        kv("net.upstream.addr", "10.0.0.1:80"),
                    ]
                    .as_slice(),
                    vec![("net.proxy.started", pid.as_slice())],
                ),
                (
                    "net.proxy.stop",
                    listen.as_slice(),
                    vec![("net.proxy.stopped", pid.as_slice())],
                ),
                (
                    "net.proxy.stop",
                    listen.as_slice(),
                    // No pid is known when nothing was running.
                    vec![("net.proxy.stopped", [].as_slice())],
                ),
            ]
        );
    }

    #[test]
    fn probe_spans_name_the_probe_and_its_peer() {
        let peer = [
            kv("net.peer.name", "example.com"),
            kv("net.peer.port", 443_i64),
        ];
        assert_span(
            || begin_reachable("example.com", 443),
            "net.probe.reachable",
            &peer,
        );
        assert_span(
            || begin_measured_latency("example.com", 443),
            "net.probe.latency",
            &peer,
        );
    }
}
