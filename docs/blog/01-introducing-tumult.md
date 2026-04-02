---
title: "Introducing Tumult: Rust-Native Chaos Engineering for the Age of AI"
parent: Blog
nav_order: 1
---

# <img src="/images/tumult.png" alt="Tumult Logo" width="100" valign="middle"> Introducing Tumult: Rust-Native Chaos Engineering for the Age of AI

![Tumult Banner](/images/tumult-banner.png)

**Today, we are launching Tumult — a modern, modular chaos engineering platform built entirely in Rust.**

Chaos engineering has a problem. Not the discipline itself — the discipline is sound. The problem is the tooling. The tools we have were built for a different era: verbose JSON experiment definitions, Python runtime dependencies, observability as an opt-in afterthought, and outputs designed for humans to squint at in a terminal.

The world has changed. Systems are distributed. Deployments are containerized and orchestrated. AI agents are beginning to take on quality engineering roles. And yet, the dominant chaos tools haven't kept pace. We built Tumult to fix that.

---

## What is Tumult?

Tumult is a chaos engineering platform that lets you define, run, and analyze experiments that test how your systems behave under real-world failure conditions. It follows the battle-tested conceptual model of Chaos Toolkit — steady-state hypotheses, fault injection methods, probes, actions, rollbacks — but rebuilds it from the ground up in Rust with three non-negotiable requirements:

1. **Speed and portability**: a single, statically-linked binary, no runtime dependencies
2. **Observability by default**: every experiment emits OpenTelemetry spans without any configuration
3. **Data-first output**: structured, token-efficient journals that feed modern analytics and AI tooling

---

## Why Now? Why Rust?

The shift to Rust wasn't about hype. It was about solving real friction points for platform teams.

**Python tooling carries hidden costs.** Every team running chaos experiments in Python knows the dance: set up a virtualenv, manage dependency conflicts between the framework and its extensions, debug cryptic import errors in CI. And when you get to production, you're deploying a Python runtime alongside your experiment runner. For a tool whose entire purpose is to test your infrastructure, that runtime is itself an infrastructure concern.

**Single binary deployment changes the operational model entirely.** The `tumult` binary is approximately 1.8MB stripped. It runs on macOS (Intel and Apple Silicon), Linux (x86_64 and aarch64), and Windows. There is nothing to install beyond the binary. Copy it to `/usr/local/bin`, and you are running experiments.

**Async-native execution matters.** Tumult runs on Tokio, Rust's async runtime. Background actions — concurrent fault injection while probes observe the system — are first-class citizens, not hacks. Long-running chaos scenarios with multiple simultaneous faults execute without blocking.

---

## The TOON Format: Built for Humans and Machines

Tumult introduces TOON (Token-Oriented Object Notation) as its experiment and journal format.

Here is the same steady-state probe in JSON versus TOON:

**JSON (Chaos Toolkit style)**:
```json
{
  "name": "health-check",
  "type": "probe",
  "provider": {
    "type": "http",
    "method": "GET",
    "url": "http://localhost:8080/health",
    "timeout": 5,
    "expected_status": 200
  }
}
```

**TOON**:
```toon
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
```

The structural difference is modest. But at scale — experiment definitions with dozens of steps, journals capturing hundreds of runs — TOON produces approximately **40-50% fewer tokens** than equivalent JSON. That is not a cosmetic improvement. It directly reduces the cost and latency of feeding experiment results to LLMs for automated analysis, and it makes journals readable by engineers without a JSON formatter.

---

## Observability That Was Never Opt-In

Run an experiment with Tumult and point it at an OpenTelemetry Collector. You get this, automatically:

```
tumult.experiment (root span)
├── tumult.hypothesis.before
│   └── tumult.probe: health-check
├── tumult.method
│   ├── tumult.action: kill-db-connections
│   └── tumult.probe: connection-count
├── tumult.hypothesis.after
│   └── tumult.probe: health-check
└── tumult.rollback
    └── tumult.action: restore-connections
```

Every span carries structured attributes in the `resilience.*` namespace: experiment ID, probe name, plugin name, duration, outcome, hypothesis status. There is no plugin to install, no control to write, no environment variable to set (beyond the collector endpoint). This is the default behavior.

This matters because the gap between "the experiment ran" and "I understand what happened and why" is exactly what observability closes. Without traces, you know the steady-state hypothesis failed. With traces, you see which probe failed, when it failed relative to fault injection, and how long it took to recover.

---

## A Complete Example

Here is a full Tumult experiment that validates database failover with automatic reconnection:

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

rollbacks[1]:
  - name: restore-connections
    activity_type: action
    provider:
      type: native
      plugin: tumult-db
      function: reset_connection_pool
```

Run it:

```bash
tumult run experiment.toon
```

The engine validates the experiment, checks the steady-state hypothesis, executes the method, re-checks the hypothesis, runs rollbacks if needed, writes a structured journal, and flushes OTel spans. Every step is logged, timed, and traceable.

---

## The Plugin Ecosystem

Tumult ships today with a growing set of plugins:

| Plugin | Type | What it does |
|--------|------|-------------|
| `tumult-ssh` | Native Rust | SSH remote execution, key/agent auth |
| `tumult-kubernetes` | Native Rust | Pod delete, deployment scale, node drain, network policies |
| `tumult-stress` | Script | CPU, memory, IO stress via stress-ng |
| `tumult-containers` | Script | Docker/Podman kill, stop, pause, resource limits |
| `tumult-process` | Script | Process kill/suspend/resume |
| `tumult-network` | Script | tc netem latency/loss/corruption, DNS block, host partition |
| `tumult-db-postgres` | Script | Kill connections, lock tables, inject latency |
| `tumult-kafka` | Script | Kill broker, partition broker, consumer lag probes |

The split between native and script plugins is intentional. Native plugins (like `tumult-kubernetes`) use Rust SDKs for deep, typed integration. Script plugins allow community contributors to add capabilities **without knowing Rust** — just write bash scripts and a TOON manifest.

---

## What's Next

Tumult is being built in public, in phases:

- **Phase 0 (Done):** Core engine, CLI, OTel integration
- **Phase 1 (Done):** SSH, stress, containers, process plugins
- **Phase 2 (In Progress):** Kubernetes, databases, Kafka, network, analytics
- **Phase 3 (Planned):** MCP server — AI agents orchestrate chaos experiments directly
- **Phase 4 (Planned):** DuckDB persistence, cross-run trends, Parquet export
- **Phase 5 (Planned):** DORA, NIS2, PCI-DSS regulatory compliance reporting

The MCP integration in Phase 3 is particularly significant. When complete, any AI agent that speaks Model Context Protocol can discover available plugins, compose experiments, run them, and interpret the results — without human intervention. Tumult becomes infrastructure for autonomous resilience validation.

---

## Get Started

```bash
git clone https://github.com/mwigge/tumult.git
cd tumult
cargo build --release
cp target/release/tumult /usr/local/bin/

# Create a new experiment
tumult init

# Validate it
tumult validate experiment.toon

# Run a dry run — see the plan without executing
tumult run experiment.toon --dry-run

# Run it
tumult run experiment.toon
```

The full documentation, plugin guides, and experiment format reference are in the `docs/` directory of the repository.

---

Over the next several posts in this series, we will go deep on the features that make Tumult different: the TOON format and AI readiness, the observability model, the plugin system, the analytics pipeline, Kubernetes chaos, statistical baselines, regulatory compliance mapping, and the roadmap ahead.

Chaos engineering shouldn't create chaos for your platform team. That's the bet Tumult is making.

---
