---
title: Architecture Decisions
nav_order: 5
has_children: true
---

# Architecture Decisions

Tumult's architectural decisions are captured as ADRs (Architecture Decision Records). Each ADR records the context, the decision made, and the rationale — so the reasoning behind every major design choice is preserved.

| ADR | Decision |
|---|---|
| [ADR-001](ADR-001-platform-runtime.md) | Rust platform with pure-Rust dependencies (russh, kube-rs, DuckDB bundled) |
| [ADR-002](ADR-002-data-observability.md) | TOON data format, `resilience.*` namespace, OpenTelemetry always-on spans |
| [ADR-003](ADR-003-experiment-model.md) | Five-phase experiment lifecycle with statistical baselines and load integration |
| [ADR-004](ADR-004-extensibility.md) | Two-tier plugin model: script-based community + native Rust (K8s, SSH, MCP) |
| [ADR-005](ADR-005-analytics.md) | Embedded DuckDB + Arrow analytics with persistent store and Parquet export |
