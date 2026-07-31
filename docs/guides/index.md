---
title: Guides
nav_order: 2
has_children: true
---

# Guides

Step-by-step guides covering all major aspects of Tumult.

| Guide | Description |
|---|---|
| [Experiment Format](experiment-format.md) | TOON experiment structure, all fields and provider types |
| [Execution Flow](execution-flow.md) | Five-phase lifecycle, orchestration internals |
| [CLI Reference](cli-reference.md) | All commands: `run`, `validate`, `analyze`, `export`, `compliance` |
| [Statistical Baselines](baseline-guide.md) | Data-derived tolerance methods: percentile, IQR, mean/stddev |
| [Analytics Guide](analytics-guide.md) | DuckDB SQL queries over experiment journals, Parquet export |
| [Observability Setup](observability-setup.md) | OTel env vars, collector configs, Jaeger, Grafana, SigNoz |
| [Load Testing Guide](load-testing-guide.md) | k6 integration with chaos experiments |
| [MCP Guide](mcp-guide.md) | The MCP server: annotations, structured output, `tumult://` resources, and the closed run-to-ingest-to-recommend loop |
| [ChaosGraph](chaosgraph.md) | The typed chaos knowledge graph served to agents over MCP: node/edge model, the two query tools, and bounded, token-efficient agent context (reproducible via make demo-proof) |
| [Agentic Quickstart](agentic-quickstart.md) | Fault injection for AI agents: scenario packs, contracts, replay |
| [Agentic Live Clients](agentic-live-clients.md) | Inject faults into Claude Code, Codex, OpenCode, and Copilot traffic |
| [Agentic Cross-Client Observability](agentic-cross-client-observability.md) | Normalize agent telemetry onto one schema; two-sided spans and trace-nesting tiers per client |
| [Agentic Recommendations](agentic-recommendations.md) | Enhance `tumult recommend` with a local agent CLI (Claude Code, Codex); generate validated experiments |
| [Agentic Observability](agentic-observability.md) | OTel instrumentation for agent runs: spans, metrics, and trace capture for agentic scenarios |
| [Agentic Scenarios](agentic-scenarios.md) | Author and run agentic fault-injection scenario packs |
| [Token Efficiency](token-efficiency.md) | TOON vs JSON token costs; keeping agent context small |
| [Topology](topology.md) | Declared service topology, compliance lineage, and injection recommendations |
| [Autopilot](autopilot.md) | Policy-gated autonomous fault injection with audit-before-act decisions |
| [Experiment Scheduling](scheduling.md) | Recurring experiments and GameDays on a schedule |
| [Production Deployment](production-deployment.md) | Deploy Tumult in production: binaries, containers, hardening |
| [Platform Walkthrough](platform-walkthrough.md) | Click-through of the Krönika web UI on the seeded demo: login → register → approve → run → e-stop → evidence pack, with screenshots |
| [Windows Faults](windows-faults.md) | `tumult-windows`: native process-kill, CPU-stress, and firewall-blackhole faults, validated live against a Windows 11 guest |
