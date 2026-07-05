---
title: Blog
nav_order: 4
has_children: true
---

# Blog

The Tumult blog series covers the platform end-to-end, from first principles to advanced use cases.

| Post | Topic |
|---|---|
| [Introducing Tumult](01-introducing-tumult.md) | What Tumult is, why it was built, and how it differs from existing tools |
| [The AI Advantage](02-ai-advantage.md) | How TOON's token efficiency enables AI-native chaos analysis |
| [Built-In Observability](03-built-in-observability.md) | OpenTelemetry spans and `resilience.*` attributes — always on, zero config |
| [The Plugin System](04-plugin-system.md) | Script plugins, native plugins, discovery order, and writing your own |
| [The Experiment Format](05-experiment-format.md) | Deep dive into TOON experiment structure, providers, and tolerances |
| [The Analytics Pipeline](06-analytics-pipeline.md) | DuckDB + Arrow + Parquet: SQL over your chaos history |
| [Kubernetes Chaos](07-kubernetes-chaos.md) | `tumult-kubernetes`: pod delete, node drain, deployment scaling |
| [Statistical Baselines](08-statistical-baselines.md) | IQR, percentile, mean/stddev — replacing magic numbers with evidence |
| [Compliance as Code](09-regulatory-compliance.md) | DORA, NIS2, PCI-DSS 4.0 — experiments as regulatory evidence |
| [Chaos Under Load](10-chaos-under-load.md) | Combining `tumult-network` and `tumult-loadtest` for realistic fault testing |
| [The Full Span Waterfall](12-traces-in-production.md) | Real SigNoz traces from a live Tumult experiment — the observability proof |
| [Load During Chaos](13-load-during-chaos.md) | k6 load testing concurrent with fault injection — proving disruption in numbers |
| [GameDay Is Here](14-gameday-is-here.md) | Coordinated campaigns with resilience scoring — 4/4 PASS, Score 1.00, COMPLIANT |
| [The Road Ahead](15-road-ahead.md) | What's delivered (Phases 0-8), what's next, the full series index |
| [Bring Your Own Agent](18-agentic-recommendations.md) | `tumult recommend --agent`: Claude Code / Codex enhance recommendations and propose validated experiments |
| [Chaos Without Root](19-net-chaos-proxy.md) | `tumult-net`: a userspace TCP chaos proxy — latency, throttling, corruption, and connection kills with no root, no tc, no docker |
| [Your Agent Is Now a First-Class Tumult Operator](20-mcp-first-class.md) | The MCP server grows to 24 tools with annotations, structured output schemas, `tumult://` resources — and a run→ingest→recommend loop that closes over MCP |
| [ChaosGraph: Your Agent Stops Re-Reading Journals](21-chaosgraph.md) | A typed knowledge graph over chaos data, built from journals on ingest and served to agents over MCP — a targeted answer stays bounded while journal-reading grows every run (~8× more compact per run, ~20× on store-wide queries) |
| [Agentic Trajectories: Chaos Engineering for Agents That Think in Steps](22-agentic-trajectories.md) | Multi-turn agent-graph fault modeling — inject a fault at one step and watch it cascade across a trajectory, with whole-trajectory contracts and four agentic subscores. The failure modes single-call testing can't see |
| [Windows Chaos, For Real: Native Faults Nobody Else Ships](23-windows-faults.md) | `tumult-windows`: native process-kill, CPU-stress, and firewall-blackhole faults — the fault domain no OSS competitor offers, validated live against a real Windows 11 guest |
