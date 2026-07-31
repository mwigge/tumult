# Tumult SigNoz dashboards

Predefined SigNoz dashboards for the Tumult chaos engineering platform,
plus the import tooling. The stack itself runs via
`docker/docker-compose.observability.yml` (SigNoz standalone on :3301) —
or point Tumult at any existing SigNoz.

## What Tumult emits

Every experiment and every experiment operation is tracked:

- **Traces** (OTLP gRPC): `resilience.experiment` root span per run, with
  `resilience.hypothesis.before/after`, `resilience.action` /
  `resilience.probe` per operation (including during/post-phase probe
  samples), `resilience.rollback`, `resilience.load`, plus plugin-crate
  spans (`k8s.*`, `ssh.*`, `net.*`, `script.execute`, `baseline.acquire`,
  `mcp.tool.call`, …).
- **Metrics**: `tumult.experiments.total`, `tumult.experiment.duration`,
  `tumult.actions.total` / `tumult.probes.total`,
  `tumult.action.duration` / `tumult.probe.duration`,
  `tumult.rollbacks.total`, `tumult.hypothesis.deviations.total`,
  `tumult.plugin.errors.total`, `tumult.plugin.script.executions.total`,
  `resilience.store.*`, `tumult.baseline.*`.
- **Logs**: the full `tracing` stream over OTLP, stamped with the active
  trace/span ids, so any log line jumps to its experiment trace.

Configuration is env-driven (see `tumult-otel/src/config.rs`):

```sh
export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317   # gRPC endpoint
export OTEL_SERVICE_NAME=tumult                            # default
# TUMULT_OTEL_ENABLED=false      # opt out entirely
# TUMULT_OTEL_CONSOLE=true       # also dump spans to stdout
# TUMULT_OTEL_LOG_STDERR=true    # logs on stderr (MCP stdio does this itself)
```

Note: Tumult exports over **gRPC** — the endpoint is the bare
`host:4317` address, no `/v1/...` path suffix.

## Dashboards

| File | Contents |
|---|---|
| `tumult-experiments-overview` | Experiment counts, success rate, deviations, plugin errors |
| `tumult-experiment-phases` | Hypothesis/method/rollback phase breakdown |
| `tumult-experiment-dataflow` | Store ingestion flow |
| `tumult-actions-probes` | Action/probe rates and durations by operation |
| `tumult-logs-traces` | Outcomes, per-operation p95, log stream, recent experiments/operations |
| `tumult-loadtest` | k6 load test throughput/latency |
| `tumult-mcp` | MCP tool calls |
| `tumult-plugins-baseline` | Baseline acquisition, tolerances |
| `tumult-infra-ops` / `tumult-infra-targets` | k8s/ssh/net operation spans |
| `tumult-store-health` / `tumult-duckdb-analytics` / `tumult-clickhouse` | Store gauges and analytics |
| `tumult-resilience-*` / `tumult-compliance-*` / `tumult-postmortem` | Scoring, compliance, MTTR |
| `tumult-postgres` / `tumult-redis` / `tumult-kafka` / `tumult-containers-*` | Target-specific views |

## Import

After starting the stack and creating your SigNoz account:

```sh
./import-dashboards.sh you@example.com yourpassword http://localhost:3301
```

Dashboards are plain SigNoz dashboard JSON — no credentials or instance
ids — and can also be imported via the UI (Dashboards → New Dashboard →
Import JSON).
