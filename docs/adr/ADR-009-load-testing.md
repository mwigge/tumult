---
title: "ADR-009: Load Testing"
parent: Architecture Decisions
nav_order: 9
---

# ADR-009: Load Tool Integration as Background Activity

## Status

Accepted

## Context

Resilience testing under realistic traffic is critical — a system that handles faults at idle may fail under load. We need to integrate load testing tools (k6, JMeter) with the chaos experiment lifecycle.

## Decision

Load tools run as **background activities** during experiments, not as plugins or probes. They are a separate concern: "what load is the system under while we inject faults?"

### Integration Model

```
Phase 1 BASELINE → Start load tool (background)
                    Wait for load to stabilize
                    Measure baseline UNDER LOAD

Phase 2 DURING  → Load continues running
                    Inject fault
                    Observe degradation under realistic traffic

Phase 3 POST    → Load continues running
                    Measure recovery under continued traffic
                    Stop load tool
                    Collect load tool metrics
```

### Tool Support

| Tool | Method | Metrics |
|------|--------|---------|
| k6 | Process driver + JSON output | req duration (p50/p95/p99), error rate, throughput |
| JMeter | Process driver + JTL parsing | response time, error rate, throughput |

### OTLP Correlation

k6 supports OTLP export (`K6_OTEL_EXPORTER_OTLP_ENDPOINT`), enabling trace correlation between load test requests and Tumult experiment spans in the same collector pipeline.

## Consequences

- Load tools are external dependencies (not bundled)
- Experiments can run with or without load — it's optional
- Load metrics flow into the journal alongside chaos results
- k6 OTLP integration enables end-to-end distributed tracing
