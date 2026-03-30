---
title: "ADR-003: Observability"
parent: Architecture Decisions
nav_order: 3
---

# ADR-003: Observability: OpenTelemetry-First with Span Hierarchy

**Status:** Accepted
**Date:** 2026-03-29
**Decision Makers:** mwigge

## Context

Observability in Chaos Toolkit is opt-in via extensions. Many users never configure it, which means experiment execution data is lost -- there is no record of what happened, when, or how the system responded. For a platform intended to produce regulatory evidence and support resilience scoring, losing observability data is unacceptable. The question is whether telemetry should be opt-in (user configures it) or always-on (user configures where it goes, not whether it exists).

Beyond the on/off question, the platform needs a well-defined span hierarchy and attribute naming convention so that telemetry data is consistent across all experiments and queryable in any OTel-compatible backend.

## Decision

### Always-On OpenTelemetry

OpenTelemetry is always on. Every Tumult operation (action execution, probe sampling, steady-state evaluation, rollback) emits spans, metrics, and structured logs via the OTel Rust SDK. Users configure WHERE telemetry data is sent (an OTLP endpoint) but not WHETHER it is collected. The OTel Collector is the recommended fan-out point -- Tumult speaks OTLP only and delegates routing, filtering, and export to the Collector.

### Span Hierarchy

Every experiment run produces this span tree:

```
tumult.experiment (root span)
├── tumult.baseline (Phase 1)
│   └── tumult.probe (per baseline probe)
├── tumult.hypothesis.before
│   └── tumult.probe (per SSH probe)
├── tumult.method
│   ├── tumult.action (per method step)
│   │   └── tumult.plugin.execute
│   └── tumult.probe (per method step)
├── tumult.during (Phase 2 -- continuous sampling)
│   └── tumult.probe (per sample)
├── tumult.hypothesis.after
│   └── tumult.probe (per SSH probe)
├── tumult.post (Phase 3 -- recovery measurement)
│   └── tumult.probe (per recovery probe)
└── tumult.rollback
    └── tumult.action (per rollback step)
```

### Attribute Namespace

All span attributes use the `resilience.*` namespace (see ADR-002). Key groups:

| Prefix | Scope | Cardinality |
|--------|-------|-------------|
| `resilience.experiment.*` | Root span (resource-level) | Low |
| `resilience.target.*` | Every span in the experiment | Low-Medium |
| `resilience.fault.*` | Action spans | Low |
| `resilience.action.*` | Action execution spans | Medium |
| `resilience.probe.*` | Probe execution spans | Medium |
| `resilience.plugin.*` | Plugin execution spans | Low |
| `resilience.outcome.*` | Result spans | Low |
| `resilience.execution.*` | Execution context spans | Low |

### Metric Naming

All metrics use the `tumult_` prefix (instrument-level, not attribute namespace):

| Metric | Type | Dimensions |
|--------|------|-----------|
| `tumult_experiments_total` | Counter | outcome |
| `tumult_actions_total` | Counter | plugin, action, outcome |
| `tumult_probes_total` | Counter | plugin, probe, outcome |
| `tumult_action_duration_seconds` | Histogram | plugin, action, outcome |
| `tumult_probe_duration_seconds` | Histogram | plugin, probe, outcome |
| `tumult_hypothesis_deviations_total` | Counter | -- |
| `tumult_plugin_errors_total` | Counter | plugin, action/probe, outcome |

## Consequences

### Positive
- Every experiment is observable by default; no data loss from misconfiguration or omission
- Vendor-neutral: OTLP is supported by all major observability backends (Jaeger, Grafana, Datadog, Splunk, etc.)
- Experiment telemetry can be correlated with existing infrastructure telemetry using standard trace context propagation
- Regulatory evidence chain is unbroken -- every action and probe has a traceable span
- OTel Collector handles fan-out, so Tumult maintains a single export path
- Consistent attribute names across all spans enable cross-experiment queries
- Low-cardinality attributes on metrics prevent cardinality explosion
- Metric prefix `tumult_` distinguishes instrument metrics from attribute namespace `resilience.*`

### Negative
- Slight performance overhead even when no collector is listening (spans are created and dropped)
- Dependency on the OTel Rust SDK, where the traces API is still in beta status
- Users in air-gapped environments must deploy an OTel Collector or accept local-only export
- `resilience.target.component` is high-cardinality -- used in traces only, not metrics

### Risks
- OTel Rust SDK traces API may introduce breaking changes before reaching stable status
- Always-on telemetry may raise concerns in environments with strict data sovereignty requirements -- users must be able to configure a local-only exporter
