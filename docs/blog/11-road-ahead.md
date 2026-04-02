---
title: "The Road Ahead: Autonomous Chaos, MCP, and the Future"
parent: Blog
nav_order: 11
---

# <img src="/images/tumult.png" alt="Tumult Logo" width="100" valign="middle"> The Road Ahead: Autonomous Chaos, MCP, and the Future of Resilience Engineering

![Tumult Banner](/images/tumult-banner.png)

*Part 11 of the Tumult series. [← Part 10: Chaos Under Load](./10-chaos-under-load.md)*

---

We have covered a lot of ground in this series: the Rust-native architecture, the TOON format, native observability, the plugin system, the analytics pipeline, Kubernetes chaos, statistical baselines, regulatory compliance, and chaos under realistic load with tumult-network and tumult-loadtest. This final post looks forward — at what Tumult is becoming, and at the broader shift in quality engineering that it is designed to serve.

---

## Where We Are Today

Tumult has completed its first two phases:

**Phase 0 — Foundation**: The core engine (`tumult-core`), CLI (`tumult-cli`), OTel integration (`tumult-otel`), and plugin framework (`tumult-plugin`) are production-ready. The five-phase experiment lifecycle, the TOON format, the journal structure, and the `resilience.*` attribute namespace are stable.

**Phase 1 — Essential Plugins**: SSH remote execution (`tumult-ssh`), resource stress (`tumult-stress`), container chaos (`tumult-containers`), and process chaos (`tumult-process`) are complete and documented.

**Phase 2 — Platform Plugins (In Progress)**: Kubernetes (`tumult-kubernetes`), database chaos (PostgreSQL, MySQL, Redis), Kafka, network chaos, and the analytics pipeline (`tumult-analytics`) are being actively developed.

The binary runs. Experiments execute. Journals are produced. OTel spans appear in Jaeger. SQL queries run against DuckDB. This is working software, not a roadmap document.

---

## Phase 3: The MCP Server

The most significant upcoming capability is the Model Context Protocol (MCP) server adapter. This is where Tumult's design decisions — the compact TOON format, the structured data model, the clean API separation between engine and adapter — pay off most clearly.

### What MCP enables

MCP is a standard protocol for AI agents to discover and call tools. An MCP server exposes a set of capabilities that any MCP-compatible agent can call. When Tumult becomes an MCP server, it exposes:

```
tumult.discover_plugins()        → list available fault injection capabilities
tumult.validate_experiment(toon) → validate an experiment definition
tumult.run_experiment(toon)      → execute an experiment, return journal
tumult.analyze_journals(path)    → SQL analytics over experiment history
tumult.list_experiments()        → list available experiments with metadata
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

We have covered:

1. [Introducing Tumult](./01-introducing-tumult.md) — what it is, why Rust, why now
2. [The AI Advantage](./02-ai-advantage.md) — the TOON format and token efficiency for LLM analysis
3. [Built-In Observability](./03-built-in-observability.md) — native OTel, span hierarchy, the `resilience.*` namespace
4. [The Plugin System](./04-plugin-system.md) — script plugins, native plugins, the no-Rust requirement
5. [The Experiment Format](./05-experiment-format.md) — TOON in depth, all sections and providers
6. [The Analytics Pipeline](./06-analytics-pipeline.md) — DuckDB, SQL queries, Parquet export
7. [Kubernetes Chaos](./07-kubernetes-chaos.md) — tumult-kubernetes, pod/deployment/node/network scenarios
8. [Statistical Baselines](./08-statistical-baselines.md) — data-derived tolerances, baseline methods
9. [Regulatory Compliance](./09-regulatory-compliance.md) — DORA, NIS2, PCI-DSS evidence generation
10. [The Road Ahead](./10-road-ahead.md) — MCP, autonomous chaos, Phase 3-5 roadmap

Each post in this series corresponds to a capability you can use today (Phases 0-2) or will be able to use in the near term (Phases 3-5). The foundation is built. The trajectory is clear.

---

**Start here:**

```bash
git clone https://github.com/mwigge/tumult.git
cd tumult
cargo build --release
cp target/release/tumult /usr/local/bin/
tumult init
tumult validate experiment.toon
tumult run experiment.toon --dry-run
```

Chaos engineering shouldn't create chaos for your platform team. That was the premise. Tumult is how we deliver on it.
