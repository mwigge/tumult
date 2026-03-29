# ADR-003 Addendum: Span Hierarchy and Attribute Naming

**Status:** Accepted
**Date:** 2026-03-29
**Parent:** ADR-003 (OpenTelemetry-first observability)

## Context

ADR-003 established that OpenTelemetry is always on. This addendum defines the specific span hierarchy and attribute naming conventions used across all experiments.

## Span Hierarchy

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
├── tumult.during (Phase 2 — continuous sampling)
│   └── tumult.probe (per sample)
├── tumult.hypothesis.after
│   └── tumult.probe (per SSH probe)
├── tumult.post (Phase 3 — recovery measurement)
│   └── tumult.probe (per recovery probe)
└── tumult.rollback
    └── tumult.action (per rollback step)
```

## Attribute Namespace

All span attributes use the `resilience.*` namespace (see ADR-005). Key groups:

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

## Metric Naming

All metrics use the `tumult_` prefix (instrument-level, not attribute namespace):

| Metric | Type | Dimensions |
|--------|------|-----------|
| `tumult_experiments_total` | Counter | outcome |
| `tumult_actions_total` | Counter | plugin, action, outcome |
| `tumult_probes_total` | Counter | plugin, probe, outcome |
| `tumult_action_duration_seconds` | Histogram | plugin, action, outcome |
| `tumult_probe_duration_seconds` | Histogram | plugin, probe, outcome |
| `tumult_hypothesis_deviations_total` | Counter | — |
| `tumult_plugin_errors_total` | Counter | plugin, action/probe, outcome |

## Consequences

- Consistent attribute names across all spans enable cross-experiment queries
- Low-cardinality attributes on metrics prevent cardinality explosion
- `resilience.target.component` is high-cardinality — used in traces only, not metrics
- Metric prefix `tumult_` distinguishes instrument metrics from attribute namespace `resilience.*`
