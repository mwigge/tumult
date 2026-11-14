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
| [SigNoz Bulk Import](signoz-bulk-import.md) | Backfill lake Parquet spans into SigNoz's ClickHouse, bypassing the collector |
| [Observability Setup](observability-setup.md) | OTel env vars, collector configs, Jaeger, Grafana, SigNoz |
| [OpenTelemetry Auto-instrumentation](otel-autoinstrumentation.md) | The naming contract through the environment alone (Java, Python, Node.js, .NET, Kubernetes), the Collector pipeline that enforces it, `GET /api/otel/conformance` |
| [Grafana Stack](grafana-stack.md) | Reference Tempo + Mimir + Loki stack: one compose file, translated metric names, LogQL/TraceQL examples |
| [Load Testing Guide](load-testing-guide.md) | k6 and oha adapters, ramp/spike/soak profiles, streamed samples |
| [Performance Regression](performance-regression.md) | `kind: performance`: per-`service.version` verdicts with a bootstrap interval, `GET /api/perf/{target}`, `POST /api/perf/{target}/verdicts`, `tumult perf` |
| [MCP Guide](mcp-guide.md) | The MCP server: annotations, structured output, `tumult://` resources, and the run-to-ingest-to-recommend loop |
| [ChaosGraph](chaosgraph.md) | The typed chaos knowledge graph served to agents over MCP: node/edge model, the two query tools, and compact agent context that stays within token budgets (reproducible via make demo-proof) |
| [Agentic Quickstart](agentic-quickstart.md) | Fault injection for AI agents: scenario packs, contracts, replay |
| [Agentic Live Clients](agentic-live-clients.md) | Inject faults into Claude Code, Codex, OpenCode, and Copilot traffic |
| [Agentic Cross-Client Observability](agentic-cross-client-observability.md) | Normalize agent telemetry onto one schema; two-sided spans and trace-nesting tiers per client |
| [Agentic Recommendations](agentic-recommendations.md) | Enhance `tumult recommend` with a local agent CLI (Claude Code, Codex); generate validated experiments |
| [Agentic Observability](agentic-observability.md) | OTel instrumentation for agent runs: spans, metrics, and trace capture for agentic scenarios |
| [Agentic Scenarios](agentic-scenarios.md) | Author and run agentic fault-injection scenario packs |
| [Token Efficiency](token-efficiency.md) | TOON vs JSON token costs; keeping agent context small |
| [Star Map](star-map.md) | One graph from organisation to workload with evidence, risk and conformance lenses; `GET /api/starmap`, `entities` in `org.yaml`, flags and roll-up counts, the proposed maturity roll-up |
| [Topology](topology.md) | Declared service topology, compliance lineage, and injection recommendations |
| [Autopilot](autopilot.md) | Policy-gated autonomous fault injection with audit-before-act decisions |
| [Experiment Scheduling](scheduling.md) | Recurring experiments and GameDays on a schedule |
| [Forecasting and run impact](forecasting.md) | Forecast bands, counterfactual run impact and recovery time; the built-in provider, the optional Timestral sidecar, air-gapped weights |
| [Data portability and recovery](data-portability.md) | Manifest archives, retention limits, complete backup and restore |
| [Execution bindings](execution-bindings.md) | Authoritative environment and target bindings for daemon execution |
| [Functional hardening review](hardening-review.md) | 2.22.0 finding coverage, acceptance scenarios and remaining limits |
| [Quality and release checks](quality-and-release.md) | Source gates, dependency policy and release-artifact verification |
| [Testing policy](testing.md) | What a test must assert, where fakes are allowed, isolation rules and the CI hygiene guard |
| [Production Deployment](production-deployment.md) | Deploy Tumult in production: binaries, containers, hardening |
| [Platform Walkthrough](platform-walkthrough.md) | Click-through of the Tumult web UI on the seeded demo: login → register → approve → run → e-stop → evidence pack, with screenshots |
| [Windows Faults](windows-faults.md) | `tumult-windows`: native process-kill, CPU-stress, and firewall-blackhole faults, validated live against a Windows 11 guest |
