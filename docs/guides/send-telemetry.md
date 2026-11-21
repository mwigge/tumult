---
title: Send telemetry
parent: Guides
nav_order: 2
has_children: true
summary: "Bring traces, metrics, logs and entity events from your services into the lake"
---

# Send telemetry

Bring traces, metrics, logs and entity events from your services into
the lake, and look at them in the backend you already use.

```mermaid
flowchart TD
    accTitle: Where telemetry goes
    accDescr: Your services, with an OpenTelemetry agent, send to a Collector. The Collector sends to tumultd, which writes the lake, and to the backend you already use, such as Grafana or SigNoz.
    S[Your services and agents] --> C[OTel Collector]
    C --> D[tumultd]
    D --> L[(Lake)]
    C --> B[Grafana, SigNoz or another backend]
```

| You want to | Page |
|---|---|
| Wire Tumult and the daemon to a Collector and a backend | [Observability setup](observability-setup.md) |
| Name your services so Tumult can match them | [Auto-instrumentation](otel-autoinstrumentation.md) |
| Track hosts and pods as they come and go | [Entity events](entity-events.md) |
| Run Tempo, Mimir, Loki and Grafana on a laptop | [Grafana stack](grafana-stack.md) |
| Copy spans the lake holds into SigNoz | [SigNoz bulk import](signoz-bulk-import.md) |
| Look up a span, attribute, metric or ingest limit | [Telemetry reference](telemetry-reference.md) |
