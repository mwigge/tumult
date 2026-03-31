---
title: "ADR-001: Platform & Runtime"
parent: Architecture Decisions
nav_order: 1
---

# ADR-001: Rust Platform with Pure-Rust Dependencies

## Status

Accepted

## Context

Chaos engineering tools like Chaos Toolkit require Python runtimes, complex dependency deployments, and vendor-specific agent installations. Production environments need tools that are fast, portable, and minimize attack surface.

## Decision

### Rust as Implementation Language

Build Tumult in Rust for:

- **Single binary deployment** — no runtime dependencies, no interpreter, no virtual environment
- **Memory safety** — ownership model prevents data races and use-after-free bugs
- **Performance** — zero-cost abstractions, no garbage collector pauses during experiment execution
- **Cross-compilation** — six target platforms (macOS arm64/x86_64, Linux glibc/musl arm64/x86_64) from a single CI pipeline

### Pure-Rust Dependency Chain

- **SSH transport via `russh`** (pure Rust, no OpenSSL) — enables remote execution without C library dependencies or agent installation on target hosts
- **`kube-rs`** for Kubernetes — native async Rust client, no kubectl shelling
- **DuckDB bundled** — compiles directly into the binary for embedded analytics
- **OpenTelemetry SDK** — Rust-native tracing and OTLP export

### Workspace Structure

Nine crates in a Cargo workspace: `tumult-core` (engine), `tumult-cli` (binary), `tumult-plugin` (script plugins), `tumult-otel` (telemetry), `tumult-baseline` (statistics), `tumult-analytics` (DuckDB/Arrow), `tumult-ssh` (remote execution), `tumult-kubernetes` (K8s operations), `tumult-mcp` (MCP server).

## Consequences

- Binary size is larger (~15-20MB with DuckDB bundled) than a Python script
- Compilation time is significant (DuckDB C++ compilation dominates)
- No runtime plugin installation — all native plugins are compiled in; community extends via script plugins
- Cross-compilation requires cross-compilation toolchains in CI
