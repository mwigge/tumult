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
| [Load Testing Guide](load-testing-guide.md) | k6 and JMeter integration with chaos experiments |
| [MCP Guide](mcp-guide.md) | The 26-tool MCP server: annotations, structured output, `tumult://` resources, and the closed run→ingest→recommend loop |
| [ChaosGraph](chaosgraph.md) | The typed chaos knowledge graph served to agents over MCP: node/edge model, the two query tools, and ~37× token savings over raw journals |
| [Agentic Quickstart](agentic-quickstart.md) | Fault injection for AI agents: scenario packs, contracts, replay |
| [Agentic Live Clients](agentic-live-clients.md) | Inject faults into Claude Code, Codex, OpenCode, and Copilot traffic |
| [Agentic Cross-Client Observability](agentic-cross-client-observability.md) | Normalize agent telemetry onto one schema; two-sided spans and trace-nesting tiers per client |
| [Agentic Recommendations](agentic-recommendations.md) | Enhance `tumult recommend` with a local agent CLI (Claude Code, Codex); generate validated experiments |
