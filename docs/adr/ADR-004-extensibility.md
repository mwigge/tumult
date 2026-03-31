---
title: "ADR-004: Extensibility"
parent: Architecture Decisions
nav_order: 4
---

# ADR-004: Plugin Architecture — Script-Based and Native Rust

## Status

Accepted

## Context

Chaos engineering requires diverse capabilities: process management, database operations, Kubernetes API calls, cloud provider actions, network manipulation. No single tool can implement all of these. The plugin model must enable community contribution without requiring Rust expertise, while allowing performance-critical integrations to be native.

## Decision

### Two-Tier Plugin Model

**Script-based plugins** (community, any language):
- Directory with executable scripts and a TOON manifest (`plugin.toon`)
- Any language: Bash, Python, Go — whatever the script author knows
- Manifest declares actions, probes, and argument schemas
- Discovered automatically via filesystem search

**Native Rust plugins** (performance-critical):
- Compiled into the Tumult binary
- Enabled via Cargo feature flags
- Sealed trait (`TumultPlugin`) — only implemented in-tree
- Used when SDK access or performance demands it

### Discovery Priority Order

Script plugins are discovered in priority order:
1. `./tumult-plugins/` (local project)
2. `~/.tumult/plugins/` (user-wide)
3. `$TUMULT_PLUGIN_PATH` (custom)
4. Built-in native plugins (always available)

Higher-priority plugins shadow lower-priority ones with the same name.

### When to Use Native vs. Script

| Criterion | Script Plugin | Native Plugin |
|-----------|--------------|---------------|
| Requires SDK (kube-rs, cloud SDKs) | No | Yes |
| Performance-critical (sub-ms) | No | Yes |
| Community contributed | Yes | No (in-tree only) |
| Any language | Yes | Rust only |

### Native Plugin Examples

- **tumult-kubernetes** — K8s operations via `kube-rs`: pod delete, deployment scale, node drain/cordon, network policy application with server-side apply
- **tumult-ssh** — Remote execution via `russh`: command execution, file upload, key/agent authentication
- **tumult-analytics** — DuckDB + Arrow analytics engine
- **tumult-mcp** — Model Context Protocol server for AI integration

### Script Plugin Examples

- **tumult-stress** — CPU/memory/IO stress via stress-ng
- **tumult-db-postgres/mysql/redis** — Database chaos operations
- **tumult-kafka** — Broker kill, partition, consumer lag probes
- **tumult-network** — tc netem latency/loss/corruption

## Consequences

- Community can contribute without knowing Rust
- Script plugins have process spawn overhead (~10ms) — acceptable for chaos timescales
- Native plugins increase binary size and compilation time
- Plugin API is sealed — changes require coordination with all native plugin authors
