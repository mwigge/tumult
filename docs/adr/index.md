---
title: Architecture Decisions
nav_order: 5
has_children: true
---

# Architecture Decisions

Tumult's architectural decisions are captured as ADRs (Architecture Decision Records). Each ADR records the context, the decision made, and the rationale — so the reasoning behind every major design choice is preserved.

| ADR | Decision |
|---|---|
| [ADR-001](ADR-001-rust-platform.md) | Why Rust: single binary, no GC, async I/O, cross-compilation |
| [ADR-002](ADR-002-data-formats.md) | TOON over JSON/YAML: token efficiency, serde-compatible |
| [ADR-003](ADR-003-observability.md) | OpenTelemetry-first: always-on spans, OTLP-only, `resilience.*` namespace |
| [ADR-004](ADR-004-plugin-system.md) | Script plugin model: community-first, any language, discovery order |
| [ADR-005](ADR-005-five-phase-model.md) | Five-phase model: Estimate → Baseline → During → Post → Analysis |
| [ADR-006](ADR-006-ssh-transport.md) | SSH via `russh`: pure Rust, no OpenSSL dependency |
| [ADR-007](ADR-007-kubernetes-native.md) | Kubernetes as a native Rust plugin (`kube-rs`) rather than script |
| [ADR-008](ADR-008-arrow-duckdb-analytics.md) | Embedded DuckDB + Apache Arrow for zero-dependency analytics |
| [ADR-009](ADR-009-load-testing.md) | Load tool integration as background activities (k6, JMeter) |
