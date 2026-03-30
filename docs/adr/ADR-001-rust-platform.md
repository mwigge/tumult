---
title: "ADR-001: Rust Platform"
parent: Architecture Decisions
nav_order: 1
---

# ADR-001: Rust for Chaos Engineering

**Status:** Accepted
**Date:** 2026-03-29
**Decision Makers:** mwigge

## Context

We need a chaos engineering platform that is fast, portable, and observable. Existing tools in this space are written in Python (Chaos Toolkit) or Go (LitmusChaos, Chaos Mesh). No Rust-based chaos engineering platform exists today. The platform must produce single-binary deployments for operator simplicity, guarantee memory safety without a garbage collector for predictable latency during fault injection, and support native async I/O for concurrent probe execution. A runtime dependency (Python interpreter, Go runtime) complicates container images and cross-compilation for edge and air-gapped environments.

## Decision

Use Rust as the implementation language for the entire Tumult platform: CLI, core engine, OTel integration, and native plugin SDK.

## Consequences

### Positive
- Single statically-linked binary per target; no runtime dependencies to ship or manage
- Memory safety enforced at compile time without garbage collection pauses
- Native async (tokio) for concurrent probe and action execution
- No language runtime required in container images; minimal attack surface
- Cross-compilation to Linux, macOS, Windows, and ARM targets via cargo and cross
- Performance characteristics suitable for high-frequency probe sampling during fault injection

### Negative
- Steeper learning curve for contributors unfamiliar with Rust's ownership model
- Smaller ecosystem for some integrations (e.g., cloud provider SDKs are less mature than Go/Python equivalents)
- Longer compile times compared to Go, especially for full release builds
- Borrow checker friction can slow initial development velocity on new abstractions

### Risks
- Community contribution rate may be lower than Python/Go alternatives due to language barrier
- Some cloud provider SDKs may lag behind their Go/Python counterparts, requiring manual API integration
