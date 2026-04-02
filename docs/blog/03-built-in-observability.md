---
title: "Built-In Proof: Native Observability with OpenTelemetry"
parent: Blog
nav_order: 3
---

# <img src="/images/tumult.png" alt="Tumult Logo" width="100" valign="middle"> Built-In Proof: Native Observability with OpenTelemetry

![Tumult Banner](/images/tumult-banner.png)

*Part 3 of the Tumult series. [← Part 2: The AI Advantage](./02-ai-advantage.md)*

---

There is a moment every chaos engineering practitioner knows. The experiment runs. The steady-state hypothesis fails. And then comes the question that the tool cannot answer: **what actually happened?**

With most chaos tools, answering that question requires correlating the experiment log with your APM traces, your application metrics, your infrastructure dashboards — each in a different system, each with its own time format and naming convention. You are doing forensic archaeology on your own infrastructure, after the fact.

Tumult takes a different position: **the experiment itself is the trace**. Every action, every probe, every hypothesis evaluation is a span. The fault injection and the system's response are correlated by trace ID from the moment the experiment starts. By the time you open Jaeger or Datadog, the full causal chain is already assembled.

---

## Why Observability Was Never Optional

The traditional approach to chaos tooling treats observability as an integration concern. You run the experiment; separately, you have your observability stack; and somehow you are expected to correlate them.

This creates two problems.

**First, the timing problem.** Your chaos tool records "action started at 14:23:11.234". Your APM system records "latency spike at 14:23:11.891". Are these the same event? Which clock is authoritative? Did the network latency between the chaos tool and the monitoring backend skew the timestamps? Without a shared trace context, you cannot be certain.

**Second, the causation problem.** Observability tools are excellent at showing you that something went wrong. They are poor at telling you what caused it. When your error rate spikes in production, you scroll through dashboards trying to find the correlated event. In a chaos experiment, the cause is known — you injected it — but without native trace context, that knowledge is trapped in the chaos tool's log.

Tumult solves both problems by generating the trace context itself.

---

## The Span Hierarchy

Every Tumult experiment produces a structured span tree:

```
tumult.experiment (root span)
│  experiment_id: 550e8400-e29b-41d4-a716-446655440000
│  experiment_name: "PostgreSQL failover recovery"
│  status: deviated
│
├── tumult.hypothesis.before
│   │  hypothesis_met: true
│   │  duration_ms: 234
│   │
│   └── tumult.probe: health-check
│          outcome: success
│          duration_ms: 107
│          resilience.probe_name: "health-check"
│
├── tumult.method
│   │  step_count: 2
│   │
│   ├── tumult.action: kill-db-connections
│   │      plugin: tumult-db
│   │      outcome: success
│   │      duration_ms: 18
│   │      resilience.fault.type: state
│   │      resilience.fault.subtype: connection-kill
│   │
│   └── tumult.probe: connection-count
│          outcome: success
│          duration_ms: 31
│          output: "0"
│
├── tumult.hypothesis.after
│   │  hypothesis_met: false
│   │  duration_ms: 189
│   │
│   └── tumult.probe: health-check
│          outcome: failure
│          duration_ms: 5003
│          error: "timeout after 5s"
│
└── tumult.rollback
    └── tumult.action: restore-connections
           outcome: success
           duration_ms: 22
```

This is not a log. It is a distributed trace. Open it in Jaeger's UI and you see the timeline: the hypothesis passing before fault injection, the action executing, the hypothesis probe timing out after fault injection, the rollback restoring state. The entire causal story in a single view.

---

## The `resilience.*` Attribute Namespace

Tumult defines a structured attribute namespace for all experiment data. Every span carries relevant attributes from this namespace.

### Required on every experiment span

| Attribute | Example |
|-----------|---------|
| `resilience.experiment.id` | `550e8400-e29b-41d4-a716-446655440000` |
| `resilience.experiment.name` | `postgresql-failover-recovery` |
| `resilience.target.system` | `database` |
| `resilience.target.technology` | `postgresql` |
| `resilience.target.environment` | `staging` |
| `resilience.fault.type` | `state` |
| `resilience.fault.subtype` | `connection-kill` |
| `resilience.fault.severity` | `major` |
| `resilience.fault.blast_radius` | `single-instance` |
| `resilience.outcome.status` | `deviated` |
| `resilience.outcome.hypothesis_met` | `false` |

### Phase data on child spans

| Attribute | Phase | Example |
|-----------|-------|---------|
| `resilience.baseline.method` | Baseline | `mean_stddev` |
| `resilience.baseline.mean` | Baseline | `45.2` |
| `resilience.during.peak_deviation_pct` | Fault active | `339.0` |
| `resilience.during.shape` | Fault active | `catastrophic` |
| `resilience.post.recovery_time_s` | Post-fault | `47.3` |
| `resilience.post.full_recovery` | Post-fault | `true` |
| `resilience.analysis.estimate_accuracy` | Analysis | `0.0` |
| `resilience.analysis.resilience_score` | Analysis | `0.41` |

The full namespace is documented in the [Tumult Metadata Model](../resilience-metadata-standard.md). It follows OpenTelemetry Semantic Convention naming rules and is designed to be interoperable with any OTel-instrumented system.

---

## Setting Up the Local Stack

The fastest way to see experiment traces locally:

```bash
# Start the OTel Collector + Jaeger
cd collector/
docker compose up -d
```

This starts:
- OTel Collector on `localhost:4317` (gRPC) and `localhost:4318` (HTTP)
- Jaeger UI on `http://localhost:16686`

Run an experiment:

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317 \
  tumult run experiment.toon
```

Open `http://localhost:16686`, search for service `tumult`, and you will see the full trace for the experiment run. Every phase, every probe, every action — with timing and attributes.

---

## Routing to Your Existing Backend

Tumult speaks OTLP only. It does not integrate directly with Jaeger, Datadog, Prometheus, or any other backend. This is a deliberate design decision: the OTel Collector handles routing.

```
Tumult ──OTLP──▶ OTel Collector ──▶ Your Backend
                  (the fan-out)
                       │
                       ├──▶ Jaeger / Tempo (traces)
                       ├──▶ Prometheus / Mimir (metrics)
                       ├──▶ Loki / Elasticsearch (logs)
                       └──▶ SigNoz / Datadog / New Relic / etc.
```

The collector configuration determines where telemetry goes. Switching from Jaeger to Grafana Tempo is a collector config change — zero Tumult code changes required. The `collector/` directory in the repository ships with reference configurations for common backends:

| Config file | Backend |
|-------------|---------|
| `otel-collector-dev.yaml` | Jaeger (local development) |
| `otel-collector-signoz.yaml` | SigNoz (all-in-one observability) |
| `otel-collector-grafana.yaml` | Tempo + Mimir + Loki (Grafana stack) |

---

## Metrics: Experiment Data as Time Series

Beyond traces, Tumult emits OTel metrics for experiment-level aggregation:

| Metric | Type | What it measures |
|--------|------|-----------------|
| `tumult_experiments_total` | Counter | Total experiments run, by status |
| `tumult_actions_total` | Counter | Actions executed, by plugin and outcome |
| `tumult_probes_total` | Counter | Probes executed, by plugin and outcome |
| `tumult_action_duration_seconds` | Histogram | Action execution time distribution |
| `tumult_probe_duration_seconds` | Histogram | Probe execution time distribution |
| `tumult_hypothesis_deviations_total` | Counter | Steady-state hypothesis failures |

These metrics feed directly into Prometheus (or any OTLP-compatible metrics backend). You can build dashboards showing:

- Deviation rate by system over time — is your service getting more or less resilient?
- Action execution time trends — are your chaos actions slower in certain environments?
- Hypothesis failure heatmaps — which experiments are consistently failing pre-conditions?

---

## What Correlated Traces Change About Post-Incident Reviews

Here is a scenario that illustrates why this matters.

Your team runs a weekly chaos experiment on the payment service: kill the database primary connection and verify automatic reconnection within 15 seconds. This week, the hypothesis fails. Recovery takes 47 seconds instead of 15.

**Without correlated traces**, your post-incident review looks like this:
- Chaos tool log: "hypothesis probe failed at 14:23:16, recovered at 14:24:03"
- APM dashboard: shows latency spike from 14:23:14 to 14:24:01
- Database metrics: connection count drops at 14:23:14, recovers at 14:24:02
- Application log: reconnection warnings starting at 14:23:15

You spend the review matching timestamps across four systems, with different clock skews and different precision.

**With Tumult traces**, the review looks like this:
- Open the experiment trace
- See the root span: `status: deviated`, `resilience.post.recovery_time_s: 47.3`
- Drill into the `tumult.hypothesis.after` span: probe timeout at 5003ms
- See the `resilience.during.shape: catastrophic` attribute — the connection count didn't degrade gracefully, it went to zero immediately
- See `resilience.analysis.estimate_accuracy: 0.0` — the prediction of 15-second recovery was significantly wrong

The trace is the review artifact. Everything needed to understand the outcome is in the same view, with the same timestamps, correlated by trace ID.

---

## Disabling Telemetry

If you are running Tumult in an environment without an OTel Collector, telemetry collection is configurable:

```bash
# Disable OTLP export entirely
TUMULT_OTEL_ENABLED=false tumult run experiment.toon

# Print spans to stdout for debugging
TUMULT_OTEL_CONSOLE=true tumult run experiment.toon

# Custom service name in telemetry
OTEL_SERVICE_NAME=chaos-pipeline tumult run experiment.toon
```

Note: when `TUMULT_OTEL_ENABLED=false`, telemetry is still collected internally for the journal — it is only the OTLP export that is disabled. The journal always contains full timing and result data regardless of the OTel configuration.

---

## The Evidence Chain

Observability in Tumult is not just about debugging. It is about evidence.

For every experiment run, there is a verifiable chain from the experiment definition to the journal to the distributed trace:

```
experiment.toon (definition)
    │
    ▼
journal.toon (results, all 5 phases)
    │
    ├──▶ trace_id → OTel backend (full distributed trace)
    ├──▶ experiment_id → DuckDB (SQL analytics)
    ├──▶ Parquet export (long-term archival)
    └──▶ HTML report (human-readable summary)
```

An auditor can start from the HTML report, drill into the journal for raw data, and follow the `trace_id` into the observability stack for the full distributed trace with nanosecond precision timing. This chain is the foundation for regulatory compliance evidence — covered in depth in Part 9 of this series.

---

*Next in the series: [Part 4 — The Plugin System: From Script to Binary →](./04-plugin-system.md)*
