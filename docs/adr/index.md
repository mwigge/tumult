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
| [ADR-006](ADR-006-kronika-stack.md) | Krönika platform stack: axum/tonic + embedded DuckDB, folded in from kronika |
| [ADR-007](ADR-007-ai-layer.md) | AI analytics layer: deterministic math, governed semantics, LLM narrates |
| [ADR-008](ADR-008-typst-report-pipeline.md) | Embedded-Typst report pipeline with renderer-agnostic content model |
| [ADR-009](ADR-009-org-hierarchy-and-manual-evidence.md) | Org hierarchy rollups and manual evidence lifecycle |
| [ADR-010](ADR-010-parquet-export-and-retention.md) | Parquet lake export and watermark-gated retention |
| [ADR-011](ADR-011-daemon-run-experiments.md) | Daemon-run experiments: embedded runner, bounded queue, e-stop, orphan reconciliation |
