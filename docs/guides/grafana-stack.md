---
title: Grafana Stack
parent: Guides
nav_order: 23
---

# Grafana Stack (Tempo + Mimir + Loki)

This guide walks through the reference Grafana-stack setup in `docker/docker-compose.grafana-full.yml`: one command boots an OTel Collector, Tempo, Mimir, Loki, and a pre-wired Grafana. It's a reference implementation for local exploration — not a production deployment (single-node everything, no auth, no TLS, filesystem storage). The general telemetry model (span hierarchy, attributes, metrics reference) lives in [Observability Setup](observability-setup.md); this guide is just about the Grafana flavor.

## Architecture

```
tumult / tumultd ──OTLP──▶ otelcol-contrib ──▶ Tempo   (traces, OTLP gRPC)
                    :4317                       ──▶ Mimir   (metrics, remote write)
                                                ──▶ Loki    (logs, OTLP HTTP /otlp)
                                                          ▲
                                                        Grafana ── queries all three
```

The collector config is `collector/otel-collector-grafana.yaml`. Metrics land in Mimir via Prometheus remote write, so metric names go through the OTLP→Prometheus translation — see the [name translation table](#metric-name-translation) below, it matters.

## Running it

```bash
# from the repo root
docker compose -f docker/docker-compose.grafana-full.yml up -d
```

Wait ~30 seconds, then check the pieces:

```bash
curl -s http://localhost:13200/ready          # Tempo   → "ready"
curl -s http://localhost:19009/ready          # Mimir   → "ready"
curl -s http://localhost:13100/ready          # Loki    → "ready"
curl -s http://localhost:23133/               # collector health extension
```

Point Tumult at the collector and run an experiment:

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317 \
  tumult run examples/cpu-stress.toon
```

`tumultd` picks up the same `OTEL_EXPORTER_OTLP_ENDPOINT` environment variable, so the daemon path is identical.

Then open Grafana at <http://localhost:13001> (anonymous Viewer; admin login is `admin`/`tumult`):

- **Dashboards → "Tumult — Grafana Full Stack (reference)"** — four panels: experiments by status, action p95, recent traces, experiment logs.
- **Explore → Tempo** — search traces or use TraceQL (below).
- **Explore → Loki** — query logs with structured metadata (below).
- **Explore → Mimir** — PromQL against the translated names.

Port notes: the collector binds host `4317/4318` so Tumult's default endpoint works unchanged. Don't run this stack alongside `docker-compose.observability.yml` (SigNoz owns `4317` there) or the demo stack (owns `13133`, which is why this stack's collector health port is `23133`). Tear down with `docker compose -f docker/docker-compose.grafana-full.yml down -v`.

## Metric name translation

OTLP metric names use dots; Prometheus doesn't allow them. The collector's remote-write exporter translates names before pushing to Mimir. Verified live against otelcol-contrib `0.157.0` + Mimir `3.1.4`:

| OTLP name (what Tumult emits) | In Mimir / PromQL |
|---|---|
| `resilience.experiments.total` | `resilience_experiments_total` |
| `resilience.actions.total` | `resilience_actions_total` |
| `resilience.probes.total` | `resilience_probes_total` |
| `resilience.hypothesis.deviations.total` | `resilience_hypothesis_deviations_total` |
| `resilience.script.executions.total` | `resilience_script_executions_total` |
| `resilience.rollbacks.total` | `resilience_rollbacks_total` |
| `resilience.rollback.failures` | `resilience_rollback_failures_total` |
| `resilience.action.duration_seconds` | `resilience_action_duration_seconds_bucket` / `_sum` / `_count` |
| `resilience.probe.duration_seconds` | `resilience_probe_duration_seconds_bucket` / `_sum` / `_count` |
| `resilience.experiment.duration_seconds` | `resilience_experiment_duration_seconds_bucket` / `_sum` / `_count` |
| `resilience.baseline.duration_seconds` | `resilience_baseline_duration_seconds_bucket` / `_sum` / `_count` |
| `resilience.store.experiments` | `resilience_store_experiments` |
| `resilience.store.activities` | `resilience_store_activities` |
| `resilience.store.size_bytes` | `resilience_store_size_bytes` |
| `resilience.store.disk_usage_pct` | `resilience_store_disk_usage_pct` |

Rules of thumb, confirmed in practice:

- Dots become underscores.
- Counters get one `_total` suffix. Names that already end in `.total` (most of Tumult's) end up with a single `_total` — no doubling with the 0.157.0 translator. A counter like `resilience.rollback.failures` (no `.total` in the name) *gains* the suffix.
- Histograms expand into `_bucket`/`_sum`/`_count` series.
- Labels survive: `status`, `plugin`, `outcome`, `experiment`. `service.name` becomes the `job` label, and the collector adds `otel_scope_name`.

To see the real list in your running stack:

```bash
curl -s http://localhost:19009/prometheus/api/v1/label/__name__/values
```

Example PromQL:

```promql
# experiment outcomes
sum by (status) (resilience_experiments_total)

# action p95 per plugin
histogram_quantile(0.95,
  sum by (le, plugin) (rate(resilience_action_duration_seconds_bucket[$__rate_interval])))
```

## Loki: logs and structured metadata

Loki 3.x ingests OTLP natively (the collector's `otlp_http/loki` exporter posts to `http://loki:3100/otlp` — the old contrib `loki` exporter is deprecated). Two things to know:

- **Only a few labels are indexed**: `service_name` and `deployment_environment`. Everything else — including all `resilience.*` attributes, with dots converted to underscores — is stored as **structured metadata**: queryable per line, but not indexed. Filter on it *after* the stream selector.
- **JSON log bodies** (Tumult's audit events are JSON strings) need `| json` before you can use their fields.

```logql
# all tumult logs
{service_name="tumult"}

# logs for one experiment run (structured metadata filter)
{service_name="tumult"} | resilience_experiment_id="418efc30-..."

# logs from one plugin
{service_name="tumult"} | resilience_plugin_name="tumult-process"

# parse JSON audit events and extract fields
{service_name="tumult"} | json | line_format "{{.event}} {{.status}}"

# failures only
{service_name="tumult"} | json | status="Failed"

# count audit events per minute
sum by (event) (count_over_time({service_name="tumult"} | json [1m]))
```

Because `trace_id` is attached to every log record as structured metadata, Grafana's Tempo→Loki correlation works: from a trace span, "Logs for this span" jumps to the matching log lines.

## Tempo: TraceQL examples

Explore → Tempo → TraceQL:

```traceql
# all tumult traces
{ resource.service.name = "tumult" }

# failed experiments
{ name = "resilience.experiment" &&
  span.resilience.experiment.status = "Failed" }

# experiments where the hypothesis didn't hold
{ name = "resilience.experiment" && span.resilience.hypothesis.met = false }

# slow actions
{ name = "resilience.action" && duration > 5s }

# find a run by experiment ID, then jump to its logs
{ span.resilience.experiment.id = "418efc30-..." }
```

## Limitations

This stack is a reference, full stop:

- **No auth, no TLS** on any component. Grafana is anonymous-Viewer with a hardcoded `admin`/`tumult` password. Do not expose any of these ports.
- **Single-node everything.** Mimir, Loki, and Tempo all run monolithic with filesystem storage; Loki retention is 7 days, Tempo compacts after 24h. Fine for a laptop, not for a team.
- **No remote storage for Grafana itself** — dashboards/datasources come from provisioning files; UI edits are allowed but not durable across `down -v`.
- The classic observability profile shares `docker/grafana/provisioning`, so this stack's Grafana also shows Jaeger/Prometheus datasources that don't resolve here (and vice versa). Harmless; they only error when queried.
- Pinned versions (`otelcol 0.157.0`, `tempo 2.9.4`, `mimir 3.1.4`, `loki 3.6.2`, `grafana 12.4.6`) were verified together in August 2026. If you bump them, re-check the collector log for deprecation warnings — exporter kind names changed once already (`otlphttp` → `otlp_http`, etc.).

For production-shaped deployments, use the vendor's Helm charts (mimir-distributed, loki, tempo) with object storage and proper auth, and keep `collector/otel-collector-grafana.yaml` as the routing reference.
