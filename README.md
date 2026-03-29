# Tumult

**Rust-native chaos engineering platform. Fast. Portable. Observable.**

Tumult is a modular chaos engineering framework written in Rust that uses [TOON](https://github.com/toon-format/spec) (Token-Oriented Object Notation) for experiment definitions and produces OpenTelemetry-native observability data from every operation.

```
tumult run experiment.toon
```

One binary. No runtime dependencies. Every action traced.

---

## Chaos Engineering

Chaos engineering is the discipline of experimenting on a system to build confidence in its capability to withstand turbulent conditions in production. The practice was pioneered by Netflix in 2011 with Chaos Monkey and formalised through the [Principles of Chaos Engineering](https://principlesofchaos.org/).

A chaos experiment follows a scientific method:

1. **Define steady state** — what does "healthy" look like? (HTTP 200, latency < 500ms, queue depth < 100)
2. **Hypothesize** — the system will remain in steady state during and after the experiment
3. **Inject faults** — kill a process, stress CPU, partition the network, drop database connections
4. **Observe** — did the system recover? How fast? What degraded?
5. **Learn** — fix weaknesses, update runbooks, improve alerting

### The Landscape Today

The chaos engineering ecosystem has matured significantly, with tools spanning from open-source frameworks to enterprise platforms:

| Tool | Language | Focus | Maintained |
|------|----------|-------|------------|
| [Chaos Toolkit](https://github.com/chaostoolkit/chaostoolkit) | Python | CLI-driven, declarative experiments, 40+ extensions | Active |
| [LitmusChaos](https://litmuschaos.io/) | Go | Kubernetes-native, ChaosHub experiment library | Active (CNCF) |
| [Chaos Mesh](https://chaos-mesh.org/) | Go | Kubernetes operator, fine-grained fault injection | Active (CNCF) |
| [ChaosBlade](https://github.com/chaosblade-io/chaosblade) | Go | Alibaba, multi-platform, rich scenario library | Active |
| [Gremlin](https://gremlin.com/) | — | Commercial SaaS, enterprise features | Active |
| [Steadybit](https://steadybit.com/) | — | Commercial, reliability platform | Active |
| [AWS Fault Injection Service](https://aws.amazon.com/fis/) | — | AWS-native, managed service | Active |
| [Azure Chaos Studio](https://azure.microsoft.com/en-us/products/chaos-studio) | — | Azure-native, managed service | Active |

### Rust in the Chaos Space

Today, Rust has only small utility crates for chaos testing — none are full platforms:

| Crate | Purpose | Scope |
|-------|---------|-------|
| [chaos-rs](https://crates.io/crates/chaos-rs) | Macro-based failure injection for unit tests | Library-level |
| [kaos](https://github.com/vertexclique/kaos) | Chaotic testing harness, random failures | Library-level |
| [tower-resilience-chaos](https://crates.io/crates/tower-resilience-chaos) | Chaos layer for Tower services | Middleware-level |
| [fracture](https://lib.rs/crates/fracture) | Deterministic async chaos testing | Simulation-level |
| [fault-injection](https://crates.io/crates/fault-injection) | Fault injection primitives | Library-level |

**There is no Rust-based chaos engineering platform** — no equivalent of Chaos Toolkit, LitmusChaos, or Chaos Mesh that provides a full experiment lifecycle (hypothesis, method, rollback, reporting) with a plugin ecosystem and observability.

### Where Tumult Fits

Tumult fills this gap as the first Rust-native chaos engineering platform:

```
                    CHAOS ENGINEERING LANDSCAPE
    ════════════════════════════════════════════════════════

    Enterprise/SaaS          Open Source (Go)        Open Source (Python)
    ┌─────────────┐         ┌──────────────┐        ┌──────────────────┐
    │ Gremlin     │         │ LitmusChaos  │        │ Chaos Toolkit    │
    │ Steadybit   │         │ Chaos Mesh   │        │ (40+ extensions) │
    │ AWS FIS     │         │ ChaosBlade   │        │                  │
    │ Azure Chaos │         │              │        │                  │
    └─────────────┘         └──────────────┘        └──────────────────┘

                            Open Source (Rust)
                         ┌──────────────────────┐
                         │       TUMULT          │
                         │                       │
                         │ • Single binary       │
                         │ • TOON format         │
                         │ • OTel-first          │
                         │ • Plugin ecosystem    │
                         │ • Token-efficient     │
                         │   data for LLM        │
                         │   analysis            │
                         └──────────────────────┘
```

---

## Why Tumult?

| Problem | Tumult's Answer |
|---------|-----------------|
| Python runtime overhead, virtualenvs, pip conflicts | Single compiled binary — `cargo install tumult` |
| JSON experiments are verbose and expensive for LLM analysis | TOON format: ~40-50% fewer tokens for journals and results |
| Observability is opt-in (install an extension, configure it) | OTel is always on — every operation is a span, every metric is emitted |
| Extensions require the same language (Python) | Script plugins: anyone who writes bash can write a Tumult plugin |
| Deployment is complex (Python + dependencies per target) | Cross-compiled static binary for Linux/macOS/Windows |

---

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│                    tumult-core                           │
│              (the engine — Rust library)                 │
│                                                         │
│   • Load .toon experiments                              │
│   • Plugin registry (actions, probes)                   │
│   • Execute method steps                                │
│   • Evaluate steady-state hypothesis                    │
│   • Rollbacks + controls lifecycle                      │
│   • OTel instrumentation (always on)                    │
│   • Journal output in TOON                              │
└──────────┬─────────────────────┬────────────────────────┘
           │                     │
┌──────────▼──────────┐  ┌───────▼──────────────────────┐
│   tumult CLI        │  │   tumult-mcp                  │
│                     │  │   (MCP server adapter)        │
│   tumult run        │  │                               │
│   tumult validate   │  │   Exposes tumult-core as      │
│   tumult discover   │  │   MCP tools for AI agent      │
│   tumult report     │  │   platforms                   │
└─────────────────────┘  └───────────────────────────────┘
```

See `docs/architecture/tumult-architecture.drawio` for detailed diagrams.

---

## Experiment Format

Experiments are defined in `.toon` files — human-readable, token-efficient, serde-compatible:

```toon
title: Database failover validates automatic reconnection
description: Kill PostgreSQL connections and verify the app reconnects
tags[2]: database,resilience

steady-state-hypothesis:
  title: Application responds healthy
  probes[1]:
    - name: health-check
      type: probe
      tolerance: 200
      provider:
        type: http
        url: ${app_url}/health
        timeout: 5

method[2]:
  - type: action
    name: kill-db-connections
    plugin: tumult-db
    action: terminate_connections
    arguments:
      host: ${db_host}
      database: myapp

  - type: probe
    name: verify-reconnection
    plugin: tumult-db
    probe: connection_count
    arguments:
      host: ${db_host}

rollbacks[1]:
  - type: action
    name: restore-connections
    plugin: tumult-db
    action: reset_connection_pool
```

---

## Plugin System

### Script Plugins (Community)

Anyone can write a Tumult plugin — no Rust toolchain required. A plugin is a directory with a TOON manifest and executable scripts:

```
tumult-plugin-kafka/
├── plugin.toon              # declares actions, probes, arguments
├── actions/
│   ├── kill-broker.sh
│   └── partition-topic.sh
├── probes/
│   ├── consumer-lag.sh
│   └── broker-health.sh
└── README.md
```

Scripts receive arguments as `TUMULT_*` environment variables. Write in any language.

See `docs/plugins/authoring-guide.md` for the full guide.

### Native Plugins (Feature-Flagged)

For performance-critical or SDK-dependent plugins, native Rust crates compile into the binary:

```bash
cargo install tumult                              # core only
cargo install tumult --features kubernetes,aws    # with native plugins
cargo install tumult --features full              # everything
```

### Available Plugins

| Plugin | Type | Scope |
|--------|------|-------|
| `tumult-ssh` | Native | SSH remote execution |
| `tumult-stress` | Script | CPU/memory/IO stress (stress-ng) |
| `tumult-containers` | Script | Docker/Podman kill, pause, stress |
| `tumult-process` | Script | Process kill/restart/signal |
| `tumult-kubernetes` | Native | Pod kill, node drain, network policy |
| `tumult-db` | Native | PostgreSQL, MySQL, Redis chaos |
| `tumult-kafka` | Script | Broker kill, partition, JMX probes |
| `tumult-http` | Native | HTTP fault injection, latency |
| `tumult-cloud-aws` | Native | EC2, RDS, ECS, Lambda |
| `tumult-cloud-gcp` | Native | Compute, GKE, Cloud SQL |
| `tumult-cloud-azure` | Native | VMs, AKS, PostgreSQL Flexible |

---

## Observability

Every Tumult operation emits OpenTelemetry data — traces, metrics, and logs. This is not opt-in; it's how the engine works.

```
tumult.experiment (root span)
├── tumult.hypothesis.before
│   └── tumult.probe
├── tumult.method
│   ├── tumult.action
│   │   └── tumult.plugin.execute
│   └── tumult.probe
├── tumult.hypothesis.after
│   └── tumult.probe
└── tumult.rollback
    └── tumult.action
```

Configure export via standard OTel environment variables:

```bash
export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317
export OTEL_SERVICE_NAME=tumult
tumult run experiment.toon
# → traces appear in Jaeger/Tempo/Datadog
```

---

## Quick Start

```bash
# Install
cargo install tumult

# Validate an experiment
tumult validate my-experiment.toon

# Run with OTel export
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317 tumult run my-experiment.toon

# Discover available plugins
tumult discover

# Generate a report from a journal
tumult report journal.toon --output report.html
```

---

## Development

### Prerequisites

- Rust 1.85+ (edition 2024)
- cargo-tarpaulin (for coverage)
- cargo-audit (for security)

### Build

```bash
git clone https://github.com/tumult-chaos/tumult.git
cd tumult
cargo build
cargo test
```

### Quality Gates

```bash
cargo fmt --check              # formatting
cargo clippy -- -D warnings    # linting
cargo test                     # tests (100% pass required)
cargo tarpaulin --out Html     # coverage (≥ 90%)
cargo audit                    # CVE check (0 HIGH/CRITICAL)
cargo doc --no-deps            # documentation builds
```

---

## Roadmap

| Phase | Status | Milestone |
|-------|--------|-----------|
| **0 — Foundation** | In Progress | Core engine, plugin system, CLI, OTel |
| **1 — Essential Plugins** | Planned | SSH, stress, containers, process |
| **2 — Platform Plugins** | Planned | Kubernetes, databases, Kafka |
| **3 — AQE + Cloud** | Planned | MCP server, AWS/GCP/Azure, reporting |

---

## Acknowledgements

Tumult is inspired by and builds upon the concepts pioneered by the [Chaos Toolkit](https://chaostoolkit.org/) project. Chaos Toolkit's experiment model — steady-state hypothesis, method, rollbacks, controls, and declarative experiment format — established the foundational patterns that Tumult reimagines in Rust.

We are grateful to [Russ Miles](https://github.com/russmiles), the ChaosIQ team, and the entire Chaos Toolkit community for making chaos engineering accessible and standardized.

Tumult also leverages:
- [TOON](https://github.com/toon-format/spec) by Johann Schopplich — token-efficient data format
- [OpenTelemetry](https://opentelemetry.io/) — vendor-neutral observability standard
- [Agentic QE Framework](https://agentic-qe.dev/) by Dragan Spiridonov — autonomous quality engineering

---

## License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for details.

Copyright 2026 Tumult Contributors.
