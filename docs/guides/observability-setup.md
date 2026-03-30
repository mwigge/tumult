---
title: Observability Setup
parent: Guides
nav_order: 6
---

# Observability Setup

Tumult emits OpenTelemetry traces, metrics, and logs for every experiment run. This guide covers how to configure where that telemetry goes.

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

Tumult speaks OTLP only. The OTel Collector routes telemetry to your backend of choice. This means you never need to change Tumult configuration when switching backends.

## Quick Start (Development)

The fastest way to see traces locally:

```bash
cd collector/
docker compose up -d
```

This starts:
- **OTel Collector** on `localhost:4317` (gRPC) and `localhost:4318` (HTTP)
- **Jaeger UI** on `http://localhost:16686`

Then run an experiment:

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317 tumult run experiment.toon
```

Open Jaeger at `http://localhost:16686`, search for service `tumult`, and you'll see the experiment trace with all phases.

## Configuration

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `TUMULT_OTEL_ENABLED` | `true` | Enable/disable telemetry collection |
| `TUMULT_OTEL_CONSOLE` | `false` | Also print spans to stdout |
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

### SigNoz

```bash
# Start SigNoz (see https://signoz.io/docs/install/)
# Then use the SigNoz collector config:
otelcol --config collector/otel-collector-signoz.yaml
```

### Grafana Stack (Tempo + Mimir + Loki)

```bash
# Requires Tempo, Mimir, and Loki running
otelcol --config collector/otel-collector-grafana.yaml
```

## Span Hierarchy

Every experiment produces this span tree:

```
tumult.experiment (root)
├── tumult.hypothesis.before
│   └── tumult.probe (per hypothesis probe)
├── tumult.method
│   ├── tumult.action (per method step)
│   │   └── tumult.plugin.execute
│   └── tumult.probe (per method step)
├── tumult.hypothesis.after
│   └── tumult.probe (per hypothesis probe)
└── tumult.rollback
    └── tumult.action (per rollback step)
```

## Key Attributes

All spans carry `resilience.*` namespace attributes:

| Attribute | Type | Description |
|-----------|------|-------------|
| `resilience.experiment_id` | string | UUID for this experiment run |
| `resilience.experiment_name` | string | Experiment title |
| `resilience.action_name` | string | Current action name |
| `resilience.probe_name` | string | Current probe name |
| `resilience.plugin_name` | string | Plugin executing the action |
| `resilience.outcome` | string | success, failure, timeout |
| `resilience.hypothesis_met` | boolean | Did steady state hold? |
| `resilience.duration_ms` | int | Execution duration |

## Metrics

| Metric | Type | Description |
|--------|------|-------------|
| `tumult_experiments_total` | counter | Experiments run |
| `tumult_actions_total` | counter | Actions executed (by plugin, outcome) |
| `tumult_probes_total` | counter | Probes executed (by plugin, outcome) |
| `tumult_action_duration_seconds` | histogram | Action execution time |
| `tumult_probe_duration_seconds` | histogram | Probe execution time |
| `tumult_hypothesis_deviations_total` | counter | Steady state violations |

## Troubleshooting

**No traces appearing?**
1. Check `TUMULT_OTEL_ENABLED` is not `false`
2. Verify the collector is running: `curl -v localhost:4317`
3. Check collector logs: `docker compose logs otel-collector`

**Traces appear but no metrics?**
- Ensure your collector config has a `metrics` pipeline
- Verify the backend supports OTLP metrics ingestion
