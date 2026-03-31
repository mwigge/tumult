# <img src="docs/images/tumult.png" alt="Tumult Logo" width="100" valign="middle"> Tumult — Rust-Native Chaos Engineering Platform

![Rust](https://img.shields.io/badge/rust-1.75%2B-orange)
![License](https://img.shields.io/badge/license-Apache--2.0-blue)
![Phase](https://img.shields.io/badge/phase-3%20Automation-blue)
![Crates](https://img.shields.io/badge/crates-9-green)

![Tumult Conceptual Banner](docs/images/tumult-banner.png)

## What is Tumult?

Tumult is a modern, modular chaos engineering platform written in Rust. It serves as a fast, portable, and inherently observable alternative to Python-based tools like Chaos Toolkit.

Tumult is designed for the modern cloud-native landscape. It doesn't just create disruption; it provides the **native observability** required to understand exactly how systems respond, and the structured data format necessary for modern analytics and automated tooling to analyze those responses at scale.

## Core Concepts (Why Rust-Native?)

Legacy chaos engineering tools are powerful but face significant hurdles in modern production environments: Python runtime overhead, complex dependency deployments, and verbose JSON data structures that are costly and inefficient for advanced analysis.

Tumult solves these issues by being built in Rust:

1. **Speed & Single Binary:** Compiles to a single binary per platform. It executes faster and "just runs" without runtime dependencies.
2. **Observability-First:** Every action, probe, and lifecycle event is emitted as a real OpenTelemetry span with `resilience.*` attributes. Each activity gets its own span with unique trace/span IDs. OTLP gRPC export is built-in.
3. **Data-Driven Analysis:** Tumult uses TOON (Token-Oriented Object Notation) for experiments and journals. Journals flow through Apache Arrow into embedded DuckDB for SQL analytics, and export to Parquet for any data tool. TOON is 40-50% more token-efficient than JSON.

## Table of Contents

- [Architecture](#architecture)
- [Experiment Format & Plugin Model](#experiment-format--plugin-model)
- [Available Plugins](#available-plugins)
- [MCP Server (AI Integration)](#mcp-server-ai-integration)
- [Data-Driven Chaos Engineering](#data-driven-chaos-engineering)
- [OpenTelemetry Observability](#opentelemetry-observability)
- [Docker Test Infrastructure](#docker-test-infrastructure)
- [Phasing & Roadmap](#phasing--roadmap)
- [Example Experiment](#example-experiment)
- [Quick Start](#quick-start)
- [Direct Comparison to Chaos Toolkit](#direct-comparison-to-chaos-toolkit)
- [Acknowledgements](#acknowledgements)
- [License](#license)

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
| **tumult-core** | Native (Rust) | Experiment runner, five-phase lifecycle, controls, rollbacks |
| **tumult-otel** | Native (Rust) | OTLP gRPC export, per-activity spans, resilience.* attributes |
| **tumult-analytics** | Native (Rust) | DuckDB embedded SQL, Arrow columnar, Parquet/CSV/IPC export |
| **tumult-baseline** | Native (Rust) | Statistical baseline derivation, percentiles, deviation detection |
| **tumult-ssh** | Native (Rust) | SSH remote execution, key/agent auth, file upload |
| **tumult-kubernetes** | Native (Rust) | Pod delete, node drain, deployment scale, network policy, label selectors |
| **tumult-mcp** | Native (Rust) | MCP server with 8 tools for AI-assisted chaos engineering |
| **tumult-stress** | Script | CPU/memory/IO stress via stress-ng, utilization probes |
| **tumult-containers** | Script | Docker/Podman kill, stop, pause, resource limits, health probes |
| **tumult-process** | Script | Process kill/suspend/resume by PID/name/pattern, resource probes |
| **tumult-db-postgres** | Script | Kill connections, lock tables, inject latency, exhaust connection pool |
| **tumult-db-mysql** | Script | Kill connections, lock tables |
| **tumult-db-redis** | Script | FLUSHALL, CLIENT PAUSE, DEBUG SLEEP, connection/memory probes |
| **tumult-kafka** | Script | Kill broker, partition broker, add latency, consumer lag probes |
| **tumult-network** | Script | tc netem latency/loss/corruption, DNS block, host partition |

See [docs/plugins/](docs/plugins/) for detailed documentation per plugin.

## MCP Server (AI Integration)

Tumult ships a built-in [Model Context Protocol](https://modelcontextprotocol.io/) server, enabling AI assistants to run, analyze, and create chaos experiments natively.

```bash
# Start the MCP server (stdio transport)
tumult mcp
```

| MCP Tool | Description |
|----------|-------------|
| `tumult_run_experiment` | Execute an experiment and return the journal |
| `tumult_validate` | Validate experiment syntax and provider support |
| `tumult_analyze` | SQL query over journals via embedded DuckDB |
| `tumult_read_journal` | Read a TOON journal and return contents |
| `tumult_list_journals` | List .toon journal files in a directory |
| `tumult_discover` | List all plugins, actions, and probes |
| `tumult_create_experiment` | Create a new experiment from a template |
| `tumult_query_traces` | Query trace data (trace/span IDs) for observability correlation |

## Data-Driven Chaos Engineering

Tumult is **data-driven by design**. Every experiment produces structured evidence — not just pass/fail, but columnar analytics data that flows through a modern data pipeline.

```
Experiment → TOON Journal → Apache Arrow (columnar) → DuckDB (embedded SQL) → Parquet (export)
```

Every probe result, every action timing, every hypothesis evaluation is captured as structured columnar data — queryable with SQL, exportable as Parquet for any data tool, and token-efficient for LLM analysis.

```bash
# Run experiments — data is captured automatically
tumult run experiment.toon

# Query your experiment data with SQL
tumult analyze journals/ --query "
    SELECT status, count(*) as runs, avg(duration_ms) as avg_ms
    FROM experiments GROUP BY status"

# Export to Parquet — portable to Spark, Polars, pandas, Jupyter
tumult export journal.toon --format parquet
```

**Why this matters:**
- **Transparency** — all experiment evidence is in standard Parquet format, auditable by anyone
- **Reusability** — query across hundreds of experiment runs with SQL, no custom scripts
- **LLM-friendly** — TOON journals are 40-50% fewer tokens than JSON equivalents
- **No infrastructure** — DuckDB is embedded, Arrow is in-memory, Parquet is a file

See [Analytics Guide](docs/guides/analytics-guide.md) for table schemas, SQL examples, and export options.

## OpenTelemetry Observability

Tumult creates **real OpenTelemetry spans** for every activity in an experiment — not just a single span per run. Each action, probe, and hypothesis evaluation gets its own span with structured attributes:

```
resilience.experiment       (root span)
├── resilience.hypothesis.before
│   └── resilience.probe    (per probe, with resilience.probe.name)
├── resilience.action       (per action, with resilience.action.name)
├── resilience.hypothesis.after
│   └── resilience.probe
└── resilience.rollback     (if triggered)
```

Every span carries `resilience.*` attributes for correlation with your existing observability stack. Trace and span IDs are recorded in the journal for post-hoc analysis.

```bash
# Export spans to any OTLP-compatible backend
TUMULT_OTEL_ENDPOINT=http://localhost:4317 tumult run experiment.toon

# Query trace data from a journal
tumult mcp  # then call tumult_query_traces
```

## Docker Test Infrastructure

Tumult provides a Docker Compose stack for end-to-end testing against real services. All ports use the `1xxxx` range to avoid conflicts with local services.

```bash
cd docker/
docker compose up -d       # Start all services
docker compose ps          # Check health
docker compose down -v     # Stop + remove volumes
```

| Service | Port | Purpose |
|---------|------|---------|
| PostgreSQL 16 | 15432 | Database chaos testing |
| Redis 7 | 16379 | Cache chaos testing |
| Kafka 3.8 (KRaft) | 19092 | Message broker chaos testing |
| SSH Server | 12222 | Remote execution testing |
| OTel Collector | 14317 (gRPC), 14318 (HTTP) | Span collection |
| Jaeger | 16686 | Trace visualization |

See [docker/README.md](docker/README.md) for detailed setup instructions.

## Phasing & Roadmap

| Phase | Scope | Status |
|-------|-------|--------|
| **0 — Foundation** | tumult-core, tumult-plugin, tumult-cli, tumult-otel | Done |
| **1 — Essential Plugins** | SSH, stress, containers, process, Kubernetes | Done |
| **2 — Analytics & Data** | DuckDB, Arrow, Parquet export, trend analysis, databases, Kafka, network | Done |
| **3 — Automation** | MCP server (8 tools), AI-assisted chaos engineering | Done |
| **4 — Persistent Analytics** | Persistent DuckDB, incremental ingestion, backup/restore, retention | Done |
| **5 — Regulatory Compliance** | DORA, NIS2, PCI-DSS evidence reporting | Planned |
| **6 — Advanced Capabilities** | Async background activities, competitive review, label selectors | Planned |
| **7 — Infrastructure** | Docker Compose e2e test stack, CI integration | Done |
| **8 — Deployment** | AQE integration, GameDay orchestration, dashboards | Planned |

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
- **Platforms**: macOS (Intel/Apple Silicon), Linux (x86_64/aarch64, glibc/musl)
- **Git** (for cloning the repo)

### Install

**From GitHub Releases** (pre-built binaries for 6 targets):

Download the latest release from [Releases](https://github.com/mwigge/tumult/releases) and place the binary on your PATH.

**From source:**

```bash
git clone https://github.com/mwigge/tumult.git
cd tumult
cargo build --release

# Binary is at target/release/tumult
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

# Analyze experiment results with SQL
tumult analyze journal.toon
tumult analyze journals/ --query "SELECT status, count(*) FROM experiments GROUP BY status"

# Cross-run trend analysis
tumult trend journals/ --metric resilience_score

# Regulatory compliance report
tumult compliance journals/ --framework dora

# Export to Parquet for external tools
tumult export journal.toon --format parquet
```

See [CLI Reference](docs/guides/cli-reference.md) for full command documentation.

## Direct Comparison to Chaos Toolkit

| Chaos Toolkit Component | Tumult Equivalent | Key Advantage |
|-------------------------|-------------------|---------------|
| `chaostoolkit` (CLI) | `tumult-cli` | Single binary, no runtime dependencies |
| `chaostoolkit-lib` (engine) | `tumult-core` | Rust speed, five-phase lifecycle with baseline |
| Python extensions | Script plugins + Native Rust plugins | Community plugins without Rust; native for performance |
| JSON experiments | TOON experiments | 40-50% fewer tokens, human-readable |
| opentracing control | Built-in OTel (per-activity spans) | Real spans with `resilience.*` attributes, always on |
| Manual analysis | `tumult-analytics` (DuckDB + Arrow) | Embedded SQL over journals, Parquet export |
| No AI integration | `tumult-mcp` (8 MCP tools) | AI assistants run experiments natively |
| Ad-hoc infrastructure | Docker Compose e2e stack | One command to spin up test services |

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
