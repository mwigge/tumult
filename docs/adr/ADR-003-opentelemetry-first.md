# ADR-003: OpenTelemetry First

**Status:** Accepted
**Date:** 2026-03-29
**Decision Makers:** mwigge

## Context

Observability in Chaos Toolkit is opt-in via extensions. Many users never configure it, which means experiment execution data is lost -- there is no record of what happened, when, or how the system responded. For a platform intended to produce regulatory evidence and support resilience scoring, losing observability data is unacceptable. The question is whether telemetry should be opt-in (user configures it) or always-on (user configures where it goes, not whether it exists).

## Decision

OpenTelemetry is always on. Every Tumult operation (action execution, probe sampling, steady-state evaluation, rollback) emits spans, metrics, and structured logs via the OTel Rust SDK. Users configure WHERE telemetry data is sent (an OTLP endpoint) but not WHETHER it is collected. The OTel Collector is the recommended fan-out point -- Tumult speaks OTLP only and delegates routing, filtering, and export to the Collector.

## Consequences

### Positive
- Every experiment is observable by default; no data loss from misconfiguration or omission
- Vendor-neutral: OTLP is supported by all major observability backends (Jaeger, Grafana, Datadog, Splunk, etc.)
- Experiment telemetry can be correlated with existing infrastructure telemetry using standard trace context propagation
- Regulatory evidence chain is unbroken -- every action and probe has a traceable span
- OTel Collector handles fan-out, so Tumult maintains a single export path

### Negative
- Slight performance overhead even when no collector is listening (spans are created and dropped)
- Dependency on the OTel Rust SDK, where the traces API is still in beta status
- Users in air-gapped environments must deploy an OTel Collector or accept local-only export

### Risks
- OTel Rust SDK traces API may introduce breaking changes before reaching stable status
- Always-on telemetry may raise concerns in environments with strict data sovereignty requirements -- users must be able to configure a local-only exporter
