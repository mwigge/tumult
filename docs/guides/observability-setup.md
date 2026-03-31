---
title: Observability Setup
parent: Guides
nav_order: 6
---

# Observability Setup

Tumult emits OpenTelemetry traces, metrics, and logs for every experiment run. This guide covers the complete span hierarchy, attribute reference, structured events, and how to route telemetry to your backend.

## Architecture

```
Tumult ──OTLP──▶ OTel Collector ──▶ Your Backend
                  (the fan-out)
                       │
                       ├──▶ Jaeger / Tempo (traces)
                       ├──▶ Prometheus / Mimir (metrics)
                       ├──▶ Loki / Elasticsearch (logs)
                       └──▶ SigNoz / Datadog / etc.
```

Tumult speaks OTLP only. The OTel Collector routes telemetry to your backend of choice. You never need to change Tumult configuration when switching backends.

## Quick Start (Development)

The fastest way to see traces locally:

```bash
cd docker/
docker compose up -d
```

This starts:
- **OTel Collector** on `localhost:14317` (gRPC) and `localhost:14318` (HTTP)
- **SigNoz UI** on `http://localhost:13301`
- **Jaeger UI** (classic stack) on `http://localhost:16686`

Then run an experiment:

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:14317 tumult run experiment.toon
```

Open SigNoz at `http://localhost:13301` → Services → `tumult`, and you'll see the experiment trace with all phases.

## Configuration

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `TUMULT_OTEL_ENABLED` | `true` | Enable/disable telemetry collection |
| `TUMULT_OTEL_CONSOLE` | `false` | Also print spans to stdout |
| `TUMULT_MCP_TOKEN` | — | Bearer token for MCP server auth (unset = no auth) |
| `TUMULT_CLICKHOUSE_URL` | — | ClickHouse URL for SigNoz cross-correlation mode |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317` | OTLP collector endpoint |
| `OTEL_SERVICE_NAME` | `tumult` | Service name in telemetry |
| `OTEL_RESOURCE_ATTRIBUTES` | — | Additional resource attributes (e.g., `deployment.environment=staging`) |

### Disabling Telemetry

```bash
TUMULT_OTEL_ENABLED=false tumult run experiment.toon
```

Telemetry is still collected internally (for the journal), but nothing is exported via OTLP.

## Collector Configurations

Reference configs are provided in the `collector/` directory:

| File | Backend | Use Case |
|------|---------|----------|
| `otel-collector-config.yaml` | Console (stdout) | Development, debugging |
| `otel-collector-dev.yaml` | Jaeger | Local development with docker-compose |
| `otel-collector-signoz.yaml` | SigNoz | All-in-one observability |
| `otel-collector-grafana.yaml` | Tempo + Mimir + Loki | Grafana stack |
| `otel-collector-e2e.yaml` | Multi-backend | E2E test environment |

### SigNoz

```bash
# Start via Docker (recommended — see docker/README.md)
make up-observe
open http://localhost:13301

# Or run the collector standalone:
otelcol --config collector/otel-collector-signoz.yaml
```

### Grafana Stack (Tempo + Mimir + Loki)

```bash
# Requires Tempo, Mimir, and Loki running
otelcol --config collector/otel-collector-grafana.yaml
```

## Span Hierarchy

Every experiment produces the following span tree. The root span is `resilience.experiment`; all nested spans are children.

```
resilience.experiment                    (root — tumult-core runner)
├── resilience.hypothesis.before
│   └── resilience.probe                 (one per hypothesis probe)
│       └── script.execute              (tumult-plugin)
│           └── [subprocess spans via TRACEPARENT env var]
├── resilience.action                    (one per method step)
│   ├── script.execute                  (for script plugins)
│   │   └── [subprocess spans via TRACEPARENT env var]
│   ├── ssh.connect                     (tumult-ssh — when target is Ssh)
│   ├── ssh.execute                     (tumult-ssh — remote command)
│   ├── k8s.pod.delete                  (tumult-kubernetes)
│   ├── k8s.node.drain
│   ├── k8s.deployment.scale
│   └── k8s.network_policy.apply
├── resilience.hypothesis.after
│   └── resilience.probe
├── resilience.rollback                  (one per rollback step)
│   └── resilience.action
├── baseline.acquire                     (tumult-baseline)
│   └── baseline.sample                 (repeated per interval)
├── resilience.analytics.ingest         (tumult-analytics → DuckDB or ClickHouse)
│   ├── resilience.analytics.query
│   └── resilience.analytics.export
└── mcp.tool.call                        (tumult-mcp — when run via MCP)
```

### Trace Context Propagation

When Tumult executes a script plugin, it injects `TRACEPARENT` and `TRACESTATE` environment variables into the subprocess. This allows scripts that emit their own OTel spans to attach as children of the `script.execute` span:

```bash
#!/usr/bin/env bash
# The TRACEPARENT env var is automatically set by Tumult.
# Any OTel-instrumented process launched here inherits the trace context.
my-otel-instrumented-service --do-chaos
```

When running via the MCP server, you can pass a `parent_context` in `RunConfig` to link the experiment root span to the calling agent's trace.

## Span Attributes Reference

### `resilience.experiment` (root span)

| Attribute | Type | Description |
|-----------|------|-------------|
| `resilience.experiment.id` | string | UUID for this experiment run |
| `resilience.experiment.name` | string | Experiment title |
| `resilience.experiment.status` | string | `Completed`, `Failed`, `Aborted`, `Interrupted` |
| `resilience.experiment.duration_ms` | int | Total experiment wall-clock time |
| `resilience.hypothesis.met` | bool | Did the steady-state hypothesis hold? |
| `resilience.hypothesis.deviations` | int | Number of probe deviations detected |

### `resilience.action` / `resilience.probe`

| Attribute | Type | Description |
|-----------|------|-------------|
| `resilience.action.name` | string | Activity name from experiment definition |
| `resilience.probe.name` | string | Probe name from experiment definition |
| `resilience.plugin.name` | string | Plugin executing the activity |
| `resilience.activity.duration_ms` | int | Activity execution duration |
| `resilience.activity.status` | string | `success`, `failure`, `timeout` |
| `resilience.activity.phase` | string | `before`, `method`, `after`, `rollback` |

### `script.execute`

| Attribute | Type | Description |
|-----------|------|-------------|
| `script.plugin_name` | string | Script plugin name |
| `script.function_name` | string | Action or probe function name |
| `script.exit_code` | int | Script process exit code |
| `script.duration_ms` | int | Script execution duration |

### `ssh.connect` / `ssh.execute`

| Attribute | Type | Description |
|-----------|------|-------------|
| `net.peer.name` | string | SSH target hostname |
| `net.peer.port` | int | SSH port (default 22) |
| `ssh.user` | string | SSH username |
| `ssh.auth_method` | string | `key_file`, `agent`, `password` |
| `ssh.command_exit_code` | int | Remote command exit code |

### `baseline.acquire`

| Attribute | Type | Description |
|-----------|------|-------------|
| `baseline.probe_name` | string | Name of the probe being baselined |
| `baseline.method` | string | `mean_stddev`, `percentile`, `iqr`, `error_rate` |
| `baseline.sample_count` | int | Number of samples collected |
| `baseline.duration_ms` | int | Baseline acquisition wall time |
| `baseline.anomaly_detected` | bool | Whether the baseline itself was anomalous |

### `resilience.analytics.ingest`

| Attribute | Type | Description |
|-----------|------|-------------|
| `analytics.backend` | string | `duckdb` or `clickhouse` |
| `analytics.experiment_id` | string | Experiment ID being ingested |
| `analytics.rows_inserted` | int | Number of activity rows written |

## Structured Span Events

Tumult emits structured span events (not logs) at key lifecycle points. These appear as timeline markers within spans in Jaeger/SigNoz.

| Event Name | Parent Span | Fields | Description |
|------------|-------------|--------|-------------|
| `journal.ingested` | `resilience.analytics.ingest` | `experiment_id`, `activity_count` | Journal successfully written to store |
| `drain.completed` | `resilience.experiment` | `spans_exported`, `metrics_exported` | OTel flush completed at experiment end |
| `tolerance.derived` | `baseline.acquire` | `probe_name`, `method`, `lower`, `upper` | Baseline tolerance bounds calculated |
| `anomaly.detected` | `baseline.acquire` | `probe_name`, `reason`, `cv` | Baseline anomaly found before experiment |
| `script.completed` | `script.execute` | `exit_code`, `stdout_bytes`, `stderr_bytes` | Script plugin finished execution |
| `experiment.started` | `resilience.experiment` | `experiment_id`, `title`, `triggered_by` | Audit event: experiment begins |
| `experiment.completed` | `resilience.experiment` | `experiment_id`, `status`, `duration_ms` | Audit event: experiment ends |

### Audit Events

The `experiment.started` and `experiment.completed` events are also emitted as structured `tracing::info!` log records with fields compatible with SIEM ingestion:

```
INFO experiment.started experiment_id="abc-123" title="Kill DB connections" triggered_by="cli"
INFO experiment.completed experiment_id="abc-123" status="Completed" duration_ms=45231
```

These events appear in log aggregators (Loki, Elasticsearch) correlated with the experiment trace via `trace_id`.

## Metrics Reference

All metrics use the `resilience.` namespace.

### Counters

| Metric | Labels | Description |
|--------|--------|-------------|
| `resilience.experiments.total` | `status` | Experiments run, by outcome |
| `resilience.actions.total` | `plugin`, `outcome` | Actions executed |
| `resilience.probes.total` | `plugin`, `outcome` | Probes executed |
| `resilience.hypothesis.deviations.total` | `experiment` | Steady-state violations, by experiment name |
| `resilience.script.executions.total` | `plugin`, `function`, `outcome` | Script plugin invocations |
| `resilience.rollbacks.total` | `outcome` | Rollback executions |
| `resilience.rollback.failures` | — | Rollback steps that failed (non-fatal) |

### Histograms

| Metric | Labels | Description |
|--------|--------|-------------|
| `resilience.action.duration_seconds` | `plugin` | Action execution latency |
| `resilience.probe.duration_seconds` | `plugin` | Probe execution latency |
| `resilience.experiment.duration_seconds` | `status` | Total experiment duration |
| `resilience.baseline.duration_seconds` | `method` | Baseline acquisition time |

### Gauges (Store)

| Metric | Description |
|--------|-------------|
| `resilience.store.experiments` | Total experiments in persistent store |
| `resilience.store.activities` | Total activity rows in store |
| `resilience.store.size_bytes` | DuckDB file size in bytes |
| `resilience.store.disk_usage_pct` | Store disk usage as percentage of volume |

## Trace-to-Metrics Correlation (SigNoz)

When using SigNoz with the ClickHouse backend, experiment data lands in the same database as SigNoz traces and metrics. This enables powerful cross-signal queries:

```sql
-- Find all traces for a specific experiment
SELECT e.title, e.status, t.traceID, t.serviceName
FROM tumult.experiments e
JOIN signoz_traces.signoz_index_v2 t
  ON e.experiment_id = t.traceID
WHERE e.status = 'Failed'

-- Correlate experiment timing with infrastructure metrics
SELECT e.title, s.unix_milli, s.value AS cpu_pct
FROM tumult.experiments e
JOIN signoz_metrics.samples_v4 s
  ON s.unix_milli BETWEEN e.started_at AND e.completed_at
WHERE s.metric_name = 'system.cpu.utilization'
```

To enable:

```bash
TUMULT_CLICKHOUSE_URL=http://localhost:8123 tumult run experiment.toon
```

## Trace Context from MCP Callers

When an AI agent or orchestration system calls Tumult via MCP (`tumult_run_experiment`), the experiment's root span can be linked to the agent's trace. Pass the W3C trace context in the MCP call metadata:

```json
{
  "_meta": {
    "extra": {
      "authorization": "Bearer <token>",
      "traceparent": "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
    }
  }
}
```

The MCP handler extracts the `traceparent` header and wires it into `RunConfig.parent_context`, making the experiment a child span of the calling agent.

## Troubleshooting

**No traces appearing?**
1. Check `TUMULT_OTEL_ENABLED` is not `false`
2. Verify the collector is running: `curl -v localhost:4317`
3. Check collector logs: `docker compose logs otel-collector`
4. Try `TUMULT_OTEL_CONSOLE=true tumult run experiment.toon` to dump spans to stdout

**Traces appear but no metrics?**
- Ensure your collector config has a `metrics` pipeline
- Verify the backend supports OTLP metrics ingestion

**`hypothesis.deviations.total` not broken down by experiment?**
- This metric carries the `experiment` label. Ensure your metrics backend supports high-cardinality labels, or filter with `--experiment <name>` in queries.

**Subprocess spans not connecting to parent?**
- The subprocess must read `TRACEPARENT`/`TRACESTATE` from environment and use them as its OTel context. Most OTel SDKs do this automatically if you call `opentelemetry::global::get_text_map_propagator`.

**SigNoz not showing experiment data?**
- Confirm `TUMULT_CLICKHOUSE_URL` is set correctly.
- Check ClickHouse is healthy: `curl http://localhost:8123/ping`
- The ClickHouse backend retries 3 times with exponential backoff (2s/4s/8s) before failing.
