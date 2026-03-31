---
title: "ADR-002: Data & Observability"
parent: Architecture Decisions
nav_order: 2
---

# ADR-002: TOON Data Format and OpenTelemetry Observability

## Status

Accepted

## Context

Chaos experiment data needs to be human-readable, machine-parseable, and efficient for automated analysis. Observability should be built-in, not opt-in — every experiment run must produce structured telemetry.

## Decision

### TOON as Primary Data Format

Use **TOON (Token-Oriented Object Notation)** for experiments and journals:

- 40-50% fewer tokens than equivalent JSON — critical for LLM analysis cost
- Human-readable with clean syntax (no closing brackets, no quoting keys)
- Full `serde` compatibility via `toon-format` crate
- `.toon` file extension for experiments and journals

### `resilience.*` Attribute Namespace

All structured data uses the `resilience.*` namespace:

```
resilience.experiment.title
resilience.experiment.status
resilience.action.name
resilience.probe.name
resilience.hypothesis.met
resilience.analysis.resilience_score
```

This namespace is shared between TOON journal fields and OpenTelemetry span attributes, ensuring consistent naming across data storage and observability.

### OpenTelemetry Always-On

Every activity creates a **real OpenTelemetry span** with `resilience.*` attributes:

```
resilience.experiment       (root span)
├── resilience.hypothesis.before
│   └── resilience.probe    (per probe)
├── resilience.action       (per action)
├── resilience.hypothesis.after
│   └── resilience.probe
└── resilience.rollback     (if triggered)
```

- **OTLP gRPC export** built-in via `opentelemetry-otlp`
- Trace and span IDs recorded in journals for post-hoc correlation
- No separate "enable observability" step — spans are created by the experiment runner

### Timestamps

Epoch nanoseconds (`i64`) for all timestamps. Enables sub-microsecond precision and direct use in DuckDB queries without parsing.

## Consequences

- TOON dependency (`toon-format` crate) — less ecosystem support than JSON
- OTel span overhead per activity (~microseconds) — negligible for chaos engineering timescales
- `resilience.*` namespace avoids collisions with application telemetry
- Journals are both a data record and an observability artifact
