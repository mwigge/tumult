---
title: "Tumult 1.0: Release Retrospective"
parent: Blog
nav_order: 18
updated: 2026-07-21
---

# Tumult 1.0: Release Retrospective

*Originally published 2026-04-05; retained as historical release context and reviewed on 2026-07-21.*

Tumult 1.0 marked the first stable release. The counts below describe that
release, not the current repository. Use `tumult discover` and the current
README for the present interface.

## What 1.0 Means

At release time, the repository recorded these quality-gate results:

- **634 unit tests**, all passing
- **Clippy pedantic** enforced on every commit
- **cargo-audit** configured in CI

## Platform Summary

```
┌─────────────────────────────────────────────────────────────────┐
│  Tumult 1.0                                                      │
│                                                                   │
│  11 Rust crates   10 plugins   48 chaos actions                  │
│  16 MCP tools     7 compliance frameworks                        │
│  634 unit tests   0 unsafe blocks                                │
│                                                                   │
│  Transports: stdio (IDE) + Streamable HTTP (containers, agents)  │
│  Analytics: DuckDB embedded SQL, Arrow columnar, Parquet export  │
│  Observability: OTel traces → SigNoz dashboards                  │
│  Compliance: DORA EU 2022/2554, NIS2, PCI-DSS, ISO-22301,       │
│              ISO-27001, SOC 2, Basel III                          │
│  Docker: ghcr.io/mwigge/tumult, ghcr.io/mwigge/tumult-mcp      │
└─────────────────────────────────────────────────────────────────┘
```

## What's New Since Phase 8

### MCP Streamable HTTP transport

The MCP server now supports both stdio (for IDE integration) and Streamable HTTP (for containers, agent fleets, and CI/CD), with session management and resumability.

```bash
tumult-mcp --transport http --port 3100
```

### Intelligence Tools

Two new tools that help agents reason about what to test:

- **tumult_recommend**; coverage gap analysis, failure patterns, stale experiments, actionable next steps
- **tumult_coverage**; per-plugin breakdown (FULL/PARTIAL/NONE), store statistics

### DNS Chaos

Enhanced `tumult-network` with DNS-specific fault injection:
- `delay-dns`; tc netem latency on port 53
- `redirect-dns`; domain → wrong IP via /etc/hosts
- `block-dns` enhanced with targeted domain blocking
- `dns-latency` probe; measure resolution time

### Docker Images on GHCR

Pre-built images published on every release; no Rust toolchain needed:

```bash
docker pull ghcr.io/mwigge/tumult:latest
docker pull ghcr.io/mwigge/tumult-mcp:latest
```

### One-Command GameDay Demo

```bash
./scripts/gameday-demo.sh
```

Starts infrastructure, connects via MCP, runs 4 PostgreSQL resilience experiments, scores results, maps to DORA compliance; all in 30 seconds.

### Code Review: 230/230 Resolved

A full 230-finding code review covering all 11 crates was completed and every finding resolved:
- 3 CRITICAL (SQL injection, runtime panic, expect in production)
- 32 HIGH (security, API design, resource leaks)
- 77 MEDIUM (performance, async correctness, missing annotations)
- 118 LOW (style, naming, hygiene)

## The Road from 0 to 1

| Phase | What | PRs |
|-------|------|-----|
| 0-2 | Foundation, plugins, analytics |; |
| 3 | MCP server (16 tools) |; |
| 4 | Persistent analytics (DuckDB + ClickHouse) |; |
| 5 | Regulatory compliance |; |
| 6 | Hardening (proptest, SSH pool, auth) |; |
| 7 | Infrastructure (SigNoz, Docker) |; |
| 8 | GameDay orchestration |; |
| 9 | Streamable HTTP, GHCR images, e2e demo | #103-#109 |
| 10 | Intelligence tools, DNS chaos, code review | #110-#115 |

## Try It

```bash
# Install
curl -sSL https://raw.githubusercontent.com/mwigge/tumult/main/install.sh | sh

# Or Docker
docker pull ghcr.io/mwigge/tumult:latest

# Run a GameDay
./scripts/gameday-demo.sh
```

---

*Tumult 1.0 at [tumult.rs](https://tumult.rs); [GitHub](https://github.com/mwigge/tumult)*
