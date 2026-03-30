# <img src="docs/images/tumult.png" alt="Tumult Logo" width="100" valign="middle"> Tumult — Rust-Native Chaos Engineering Platform

![Rust](https://img.shields.io/badge/rust-1.75%2B-orange)
![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue)
![Phase](https://img.shields.io/badge/phase-0%20Foundation-green)

![Tumult Conceptual Banner](docs/images/tumult-banner.png)

## What is Tumult?

Tumult is a modern, modular chaos engineering platform written in Rust. It serves as a fast, portable, and inherently observable alternative to Python-based tools like Chaos Toolkit.

Tumult is designed for the modern cloud-native landscape. It doesn't just create disruption; it provides the **native observability** required to understand exactly how systems respond, and the structured data format necessary for modern analytics and automated tooling to analyze those responses at scale.

## Core Concepts (Why Rust-Native?)

Legacy chaos engineering tools are powerful but face significant hurdles in modern production environments: Python runtime overhead, complex dependency deployments, and verbose JSON data structures that are costly and inefficient for advanced analysis.

Tumult solves these issues by being built in Rust:

1. **Speed & Single Binary:** Compiles to a single binary per platform. It executes faster and "just runs" without runtime dependencies.
2. **Observability-First:** Every action, probe, and lifecycle event is emitted as an OpenTelemetry (OTel) span with structured attributes by default. OTLP export is built-in.
3. **Data-Driven Analysis:** Tumult uses TOON (Token-Oriented Object Notation) for experiments and journals. TOON is structured, human-readable, and highly token-efficient, massively reducing the overhead of processing experiment data for automated insights.

## Architecture

Tumult uses a decoupled engine and adapter layer architecture, allowing the core engine to be orchestrated by a CLI, an API, or any automated orchestration system via the Model Context Protocol (MCP).

![Tumult Architecture Diagram](docs/images/tumult-tech-architecture.png)

### The Chaos Engineering Landscape

![Chaos Engineering Landscape](docs/images/chaos-engineering-landscape.png)

## Experiment Format & Plugin Model

### Compatibility

Tumult retains the familiar conceptual model of Chaos Toolkit, allowing you to transfer existing knowledge of:
* Steady-State Hypotheses
* Methods (Sequential and Background steps)
* Probes & Actions
* Controls (Lifecycle Hooks)
* Rollbacks

### TOON Experiments

Experiments are defined in TOON (.toon), replacing verbose JSON with a concise, token-efficient format designed for both humans and advanced tooling.

### Community Plugins: Script-Based

The script-based plugin model enables the community to contribute chaos capabilities **without needing to know Rust**. Community plugins are simply directories containing executable scripts (Bash, Python, etc.) and a TOON manifest declaring their capabilities.

```text
tumult-plugin-kafka/
├── plugin.toon              # declares actions, probes, arguments
├── actions/
│   ├── kill-broker.sh
├── probes/
│   ├── consumer-lag.sh
```

### Native Rust Plugins

Native plugins (for performance-critical or SDK-heavy tasks like kube-rs or cloud provider SDKs) are built directly into the core and enabled via Cargo feature flags.

```bash
cargo install tumult --features kubernetes,aws
```

## Available Plugins

| Plugin | Type | Capabilities |
|--------|------|-------------|
| **tumult-ssh** | Native (Rust) | SSH remote execution, key/agent auth, file upload |
| **tumult-stress** | Script | CPU/memory/IO stress via stress-ng, utilization probes |
| **tumult-containers** | Script | Docker/Podman kill, stop, pause, resource limits, health probes |
| **tumult-process** | Script | Process kill/suspend/resume by PID/name/pattern, resource probes |
| **tumult-kubernetes** | Native (Rust) | Pod delete, node drain, deployment scale, network policy, label selectors |
| **tumult-db-postgres** | Script | Kill connections, lock tables, inject latency, exhaust connection pool |
| **tumult-db-mysql** | Script | Kill connections, lock tables |
| **tumult-db-redis** | Script | FLUSHALL, CLIENT PAUSE, DEBUG SLEEP, connection/memory probes |
| **tumult-kafka** | Script | Kill broker, partition broker, add latency, consumer lag probes |
| **tumult-network** | Script | tc netem latency/loss/corruption, DNS block, host partition |

See [docs/plugins/](docs/plugins/) for detailed documentation per plugin.

## Analytics

Tumult embeds DuckDB and Apache Arrow for SQL analytics over experiment journals:

```bash
# Run experiments over time...
tumult run experiment.toon

# Analyze with SQL
tumult analyze journals/ --query "SELECT status, count(*), avg(duration_ms) FROM experiments GROUP BY status"

# Export to Parquet for external tools
tumult export journal.toon --format parquet
```

The data pipeline: **TOON Journal → Arrow RecordBatch → DuckDB (SQL) → Parquet (export)**

See [docs/guides/analytics-guide.md](docs/guides/analytics-guide.md) for details.

## Phasing & Roadmap

| Phase | Scope | Status |
|-------|-------|--------|
| **0 — Foundation** | tumult-core, tumult-plugin, tumult-cli, tumult-otel | Done |
| **1 — Essential Plugins** | SSH, stress, containers, process | Done |
| **2 — Platform Plugins** | K8s, databases, Kafka, network, analytics | In Progress |
| **3 — Automation + Cloud** | tumult-mcp, Cloud SDKs, HTML reporting | Planned |
| **4 — Persistent Analytics** | DuckDB persistence, cross-run trends, backup/export | Planned |
| **5 — Regulatory Compliance** | DORA, NIS2, PCI-DSS evidence reporting | Planned |

## Example Experiment

Here's a complete experiment in TOON that validates database failover with automatic reconnection:

```toon
title: Database failover validates automatic reconnection
description: Kill PostgreSQL primary connections and verify app reconnects

tags[2]: database, resilience

configuration:
  db_host:
    type: env
    key: DATABASE_HOST

estimate:
  expected_outcome: recovered
  expected_recovery_s: 15.0
  expected_degradation: moderate
  expected_data_loss: false
  confidence: high
  rationale: Tested monthly with consistent recovery
  prior_runs: 5

baseline:
  duration_s: 120.0
  warmup_s: 15.0
  interval_s: 2.0
  method: mean_stddev
  sigma: 2.0
  confidence: 0.95

steady_state_hypothesis:
  title: Application responds healthy
  probes[1]:
    - name: health-check
      activity_type: probe
      provider:
        type: http
        method: GET
        url: http://localhost:8080/health
        timeout_s: 5.0
      tolerance:
        type: exact
        value: 200

method[1]:
  - name: kill-db-connections
    activity_type: action
    provider:
      type: native
      plugin: tumult-db
      function: terminate_connections
      arguments:
        database: myapp
    pause_after_s: 5.0
    background: false

rollbacks[1]:
  - name: restore-connections
    activity_type: action
    provider:
      type: native
      plugin: tumult-db
      function: reset_connection_pool
    background: false

regulatory:
  frameworks[1]: DORA
  requirements[1]:
    - id: DORA-Art24
      description: ICT resilience testing
      evidence: Recovery within RTO
```

## Quick Start

### Prerequisites

- **Rust 1.75+** — install via [rustup.rs](https://rustup.rs/)
- **Platforms**: macOS (Intel/Apple Silicon), Linux (x86_64, aarch64), Windows (x86_64)
- **Git** (for cloning the repo)

### Install

```bash
# Clone and build
git clone https://github.com/mwigge/tumult.git
cd tumult
cargo build --release

# Binary is at target/release/tumult (~1.8MB stripped)
# Optionally copy to your PATH:
cp target/release/tumult /usr/local/bin/
```

### Usage

```bash
# Create a new experiment
tumult init

# Validate an experiment
tumult validate experiment.toon

# Dry run — see the execution plan without running
tumult run experiment.toon --dry-run

# Run the experiment
tumult run experiment.toon

# Run with custom rollback strategy
tumult run experiment.toon --rollback-strategy always

# List discovered plugins
tumult discover
```

See [CLI Reference](docs/guides/cli-reference.md) for full command documentation.

## Direct Comparison to Chaos Toolkit

| Chaos Toolkit Component | Tumult Equivalent | Key Advantage |
|-------------------------|-------------------|---------------|
| `chaostoolkit` (CLI) | `tumult-cli` | Lighter, single binary. |
| `chaostoolkit-lib` (engine) | `tumult-core` | Rust speed, async execution. |
| Python extensions | Script plugins + Native plugins | No runtime dependencies; community simple scripts. |
| JSON experiments | TOON experiments | Highly Token-Efficient for automated analysis. |
| opentracing control | Built-in OTel (Always On) | Native Observability, not opt-in. |

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
