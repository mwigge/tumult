---
title: "The Road Ahead: Autonomous Chaos, MCP, and the Future"
parent: Blog
nav_order: 15
---

# <img src="/images/tumult.png" alt="Tumult Logo" width="100" valign="middle"> The Road Ahead: Autonomous Chaos, MCP, and the Future of Resilience Engineering

![Tumult Banner](/images/tumult-banner.png)

*Part 11 of the Tumult series. [← Part 10: Chaos Under Load](./10-chaos-under-load.md)*

---

We have covered a lot of ground in this series: the Rust-native architecture, the TOON format, native observability, the plugin system, the analytics pipeline, Kubernetes chaos, statistical baselines, regulatory compliance, and chaos under realistic load with tumult-network and tumult-loadtest. This final post looks forward — at what Tumult is becoming, and at the broader shift in quality engineering that it is designed to serve.

---

## Where We Are Today

Tumult has delivered Phases 0 through 9:

**Phases 0-2 — Foundation + Plugins**: 11 Rust crates, 10 plugins (48 chaos actions), native Kubernetes (kube-rs) and SSH (russh), DuckDB + Arrow analytics pipeline, Parquet/CSV/JSON export. All production-ready.

**Phases 3-5 — Automation + Analytics + Compliance**: MCP server with 14 tools, persistent DuckDB + ClickHouse dual-mode analytics, 7 regulatory compliance frameworks (DORA EU 2022/2554, NIS2, PCI-DSS, ISO-22301, ISO-27001, SOC2, Basel III) with article-level detail and official source URLs.

**Phase 6 — Hardening**: SSH session pool, proptest, cargo-audit in CI, security assessment, zero unsafe blocks across all crates.

**Phase 7 — Infrastructure**: SigNoz standalone, OTel Collector (contrib), Docker Compose stacks, tumult.rs website.

**Phase 8 — GameDay**: Coordinated experiment campaigns with shared load, resilience scoring, and compliance article mapping. The first GameDay (Q2 PostgreSQL Resilience) ran 4/4 PASS with a resilience score of 1.00.

**Phase 9 — MCP HTTP Transport**: Tumult MCP server now supports HTTP/SSE transport (`--transport http`) for container-to-container and agent fleet communication. Docker images for both CLI and MCP server. Composable Docker bundles (infra, observe, tumult, aqe) with `start.sh` launcher.

162 platform tests at 99.4% pass rate. 585 unit tests. Zero failures.

---

## The MCP Server

The Model Context Protocol server exposes 14 tools over stdio or HTTP/SSE transport:

```
tumult.discover_plugins()        → list available fault injection capabilities
tumult.validate_experiment(toon) → validate an experiment definition
tumult.run_experiment(toon)      → execute an experiment, return journal
tumult.analyze(query)            → SQL analytics over experiment journals
tumult.analyze_store(query)      → SQL over persistent DuckDB store
tumult.list_experiments()        → list available experiments
tumult.gameday_run(path)         → run a coordinated GameDay campaign
tumult.gameday_analyze(path)     → analyze GameDay results with scoring
```

```bash
# Stdio (IDE integration)
tumult-mcp

# HTTP/SSE (containers, agent fleets, CI/CD)
tumult-mcp --transport http --port 3100
```

An AI agent — whether a custom agent, an agentic QE system, or any MCP-compatible orchestrator — can call these functions without knowing anything about Rust, binaries, or experiment formats. The agent asks "what chaos capabilities are available for Kubernetes?" and Tumult responds with the plugin manifest. The agent composes an experiment, validates it, runs it, and reads the TOON journal — compact enough to fit in context.

### The agentic loop

```
AI Agent
  │
  ├── "What plugins are available?"
  │       → tumult.discover_plugins()
  │       ← [tumult-kubernetes, tumult-network, tumult-db-postgres, ...]
  │
  ├── "Run a pod deletion experiment on the payments service"
  │       → tumult.run_experiment(generated_toon)
  │       ← TOON journal (compact, in-context)
  │
  ├── "The recovery took 47 seconds but the RTO is 30 seconds.
  │    What should we investigate?"
  │       → Agent reasons over journal, proposes hypothesis
  │       → tumult.run_experiment(follow_up_experiment)
  │       ← Next journal
  │
  └── "Generate a DORA compliance report"
        → tumult.analyze_journals(path, query=dora_sql)
        ← Compliance evidence table
```

This is not a feature for someday. The architecture was designed for this from the start. The MCP adapter is a surface over an engine that is already fully capable.

### Why TOON matters for MCP

An experiment journal in JSON that covers 200 activities — a typical multi-phase experiment with baseline sampling — is approximately 15,000-20,000 tokens. That exhausts the context window of many LLM calls before the agent can do any reasoning.

The same journal in TOON is 8,000-10,000 tokens. The difference is whether the agent can process a single experiment run in context, or whether it needs to summarize and lose information. For agents that run multiple experiments in sequence and reason over the results, the token efficiency of TOON is the difference between the architecture working and not working.

---

## Phase 4: Persistent Analytics and Cross-Run Intelligence

The analytics capabilities today are per-run and per-batch: you run experiments, export journals, query them with DuckDB. Phase 4 adds persistence — a running DuckDB database that accumulates all experiment history automatically.

**What this enables:**

**Real-time trend detection.** Instead of running a batch query over journals after each experiment, the persistent database updates on every run. Trends — improving, stable, degrading — are computed continuously.

**Cross-run scoring.** The resilience scoring model (Layer 1 of the scoring methodology) gains access to the full run history. Grade A/B/C/D scores are computed against the trailing 10 runs. A new experiment run on a service that has been consistently reliable gets a different baseline than one run on a service with a history of deviations.

**Automatic anomaly detection.** With a complete run history, the engine can detect when a service's recovery time suddenly degrades — even if the experiment still passes. The absolute value might be within tolerance, but the trend is wrong.

**DORA Four Keys integration.** Phase 4 will accept deployment frequency, lead time, change failure rate, and MTTR data from CI/CD systems. This enables the full DORA Four Keys dashboard — correlated with experiment evidence — from a single SQL query.

---

## Phase 5: Regulatory Compliance Automation

Phase 5 extends the regulatory compliance capabilities from evidence collection to automated reporting:

**Automated coverage reporting.** At the end of each quarter, automatically generate a compliance coverage report: which requirements have experiment evidence, which are gaps, what the testing frequency was, and whether RTOs were met.

**Certification-ready artefacts.** For ISO 22301 and SOC 2, generate the formal post-exercise reports that auditors require — structured, signed, with complete evidence chains — from the journal data, without manual report writing.

**Regulatory intelligence.** As framework requirements evolve (DORA implementing technical standards, NIS2 member state transpositions), Tumult's regulatory mapping is updated through the framework definitions rather than requiring individual experiment updates.

---

## The Broader Shift: From Testing to Intelligence

Looking further ahead, the trajectory of Tumult is toward autonomous resilience intelligence.

Today, chaos engineering is a practice: engineers design experiments, run them, interpret results, and take action. The tool executes what humans specify.

The next stage is **agentic chaos engineering**: AI agents design experiments based on system understanding, run them autonomously, interpret results against learned models, and propose — or take — remediation actions. The human role shifts from "designing and running experiments" to "governing the agent's scope and reviewing significant findings."

Tumult's architecture supports this progression:

1. **Structured inputs and outputs** — TOON format is parseable and generatable by any LLM without prompt engineering
2. **Typed data model** — the `resilience.*` attribute namespace gives agents a vocabulary for talking about experiments without ambiguity  
3. **MCP interface** — agents call Tumult as a tool, with no custom integration required
4. **Prediction tracking** — the estimate vs actual model teaches agents what results to expect from what fault types, enabling them to design more informative experiments over time

The Agentic QE Framework — referenced in the Tumult acknowledgements — describes this architecture in full: autonomous quality engineering agents that plan, execute, and interpret experiments, escalating to human review only when something unexpected or high-stakes occurs.

---

## Getting Involved

Tumult is open source (Apache 2.0) and being built in public.

**For engineers**: the most valuable contributions right now are script plugins. If your infrastructure has a component that deserves chaos testing — HAProxy, Vault, Consul, Elasticsearch, custom services — write a plugin. The [Plugin Authoring Guide](../plugins/authoring-guide.md) is the starting point. No Rust required.

**For platform teams**: if you are running Tumult in your organization and have feedback on the experiment format, the analytics queries, or the regulatory compliance mapping — open an issue. The data model is designed to be standard and interoperable; your production use cases make it better.

**For observability engineers**: the `resilience.*` attribute namespace is designed to be an open standard, not a Tumult-specific proprietary format. If your observability tooling can consume `resilience.*` span attributes, your users get Tumult integration for free. Contributions to the metadata model spec are welcome.

**For the AI/ML community**: the TOON journal format and the MCP interface are designed for LLM consumption. If you are building agentic QE systems, Tumult is designed to be your chaos layer. The MCP server adapter (Phase 3) is the integration point — and contributions to that layer are particularly valuable.

---

## A Note on Philosophy

Chaos engineering is, at its core, a scientific practice. You form a hypothesis, you conduct an experiment, you measure the outcome, and you update your understanding of the system.

Tumult's design follows this philosophy literally: the five-phase data lifecycle encodes the scientific method. Phase 0 is the hypothesis. Phases 1-3 are the experiment and measurement. Phase 4 is the analysis and update.

The features that make Tumult different — the estimate vs actual tracking, the baseline derivation, the anomaly detection on the baseline itself, the trend analysis — are all expressions of the same idea: chaos engineering produces evidence, and evidence should be treated rigorously.

An experiment that passes because the tolerance was too loose is not evidence of resilience. An experiment whose estimate was accurate is evidence that the team understands how their system behaves. An experiment whose estimate was wrong is evidence that they learned something. Both are valuable. Neither is available without the structured data model that Tumult provides.

---

## Summary: The Series

## What's Been Delivered

Since this post was first written, Tumult has shipped everything from Phases 0 through 7 — and Phase 8 is in progress:

| Phase | What | Status |
|-------|------|--------|
| 0-6 | Core engine, 10 plugins, DuckDB analytics, OTel, MCP, compliance, hardening | Done |
| 7 | SigNoz observability, Docker Compose stacks, custom collector | Done |
| 8 | **GameDay orchestration** — coordinated campaigns with resilience scoring | In Progress |

**What's new since the original roadmap:**

- **Load testing during chaos** — k6 runs concurrently via `--load` flag, real disruption measured in numbers
- **Native plugin dispatch** — tumult-kubernetes (kube-rs) and tumult-ssh (russh) wired into the experiment runner
- **Pumba network chaos** — 10 container-scoped actions, cross-platform
- **GameDay orchestration** — `tumult gameday run` executes coordinated campaigns with shared load, resilience scoring, and DORA/NIS2 compliance mapping
- **Default `tumult analyze`** — structured summaries without SQL, `--last N`, `--all` aggregate
- **tumult.rs** — live website with landing page and interactive demo
- **162 platform tests, 99.4% pass** — zero failures, zero issues

## What's Next

- **GameDay dashboards** — pre-built SigNoz dashboards filtered by gameday_id
- **GameDay DuckDB table** — `gamedays` table for SQL analytics over campaigns
- **HTTP chaos** — nginx target in Docker stack for HTTP fault injection
- **Agentic QE integration** — autonomous experiment composition via MCP
- **Production GameDay templates** — industry-specific templates for financial services, e-commerce, healthcare

## The Series

1. [Introducing Tumult](./01-introducing-tumult.md) — what it is, why Rust, why now
2. [The AI Advantage](./02-ai-advantage.md) — TOON format and LLM readiness
3. [Built-In Observability](./03-built-in-observability.md) — native OTel spans
4. [The Plugin System](./04-plugin-system.md) — script and native plugins
5. [The Experiment Format](./05-experiment-format.md) — TOON in depth
6. [The Analytics Pipeline](./06-analytics-pipeline.md) — DuckDB + Arrow + Parquet
7. [Kubernetes Chaos](./07-kubernetes-chaos.md) — pod, deployment, node chaos
8. [Statistical Baselines](./08-statistical-baselines.md) — data-derived tolerances
9. [Regulatory Compliance](./09-regulatory-compliance.md) — DORA, NIS2, PCI-DSS
10. [Chaos Under Load](./10-chaos-under-load.md) — network faults + load testing
12. [The Full Span Waterfall](./12-traces-in-production.md) — real SigNoz traces
13. [Load During Chaos](./13-load-during-chaos.md) — proving disruption in numbers
14. [GameDay Is Here](./14-gameday-is-here.md) — compliance programmes with resilience scoring

---

**Start here:**

```bash
curl -sSL https://raw.githubusercontent.com/mwigge/tumult/main/install.sh | sh
tumult run examples/redis-chaos.toon
tumult gameday run gamedays/q2-postgres-resilience.gameday.toon
```

Chaos engineering that proves compliance — not just resilience. That was the premise. Tumult delivers on it.
