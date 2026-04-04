---
title: "Proving Disruption in Numbers: Load Testing During Chaos Injection"
parent: Blog
nav_order: 13
---

# <img src="/images/tumult.png" alt="Tumult Logo" width="100" valign="middle"> Proving Disruption in Numbers: Load Testing During Chaos Injection

![Tumult Banner](/images/tumult-banner.png)

*Part 13 of the Tumult series. [← Part 12: The Full Span Waterfall](./12-traces-in-production.md)*

---

Chaos engineering without load is a rehearsal without an audience. You can kill a database connection, pause a container, inject latency — but if nothing is using the system when the fault hits, you have no evidence of impact. The experiment passes, the journal says "completed," and you have learned nothing about how your system behaves under real conditions.

Tumult now runs load tests concurrently with chaos injection. The load generator hammers your system while faults are active. The disruption is measured in numbers — latency spikes, error rates, throughput drops — captured in the same journal, queryable in the same DuckDB store, visible in the same OTel trace.

---

## The Architecture

```
resilience.experiment (root trace)
├── resilience.hypothesis.before
├── resilience.load (background — k6 running continuously)
│   └── TRACEPARENT propagated to k6 for trace correlation
├── resilience.action: pause-postgres (foreground chaos)
├── resilience.hypothesis.after
└── load_result: {latency_p95: 157ms, error_rate: 0.003, requests: 300}
```

The load test runs as a background process. The chaos method runs in the foreground. Both share the same parent OTel trace. When you open SigNoz, the load span and the chaos span appear in parallel on the waterfall.

---

## A Real Example

Here is a PostgreSQL experiment. k6 hammers the database with real INSERT and SELECT queries using the xk6-sql driver. While k6 is running, Pumba pauses the PG container for 5 seconds.

```toon
title: PostgreSQL under k6 load — container pause disruption

load:
  tool: k6
  script: examples/k6/pg-load.js
  vus: 5
  duration_s: 20.0

method[3]:
  - name: connection-count-before
    activity_type: probe
    ...
  - name: pause-postgres-5s
    activity_type: action
    ...
  - name: connection-count-after
    activity_type: probe
    ...
```

Run it:

```bash
tumult run examples/pg-load-chaos.toon
```

Or use CLI flags to add load to any existing experiment:

```bash
tumult run experiment.toon --load k6 --load-script load.js --load-vus 50 --load-duration 30s
```

---

## The Evidence

The journal captures the load result alongside the method results:

```toon
load_result:
  tool: k6
  duration_s: 10.5
  vus: 5
  latency_p50_ms: 101.0
  latency_p95_ms: 157.0
  error_rate: 0.003
  total_requests: 300
  thresholds_met: true
```

Compare this to a baseline run without chaos:

| Metric | Baseline (no chaos) | Under chaos (5s pause) | Impact |
|--------|-------------------|----------------------|--------|
| p95 latency | 97ms | 157ms | +62% |
| Max latency | 151ms | 5,130ms | 34x |
| Avg query time | 18ms | 47ms | 2.6x |
| Error rate | 0% | 0.3% | Disrupted |
| Recovery | — | 100% | Full |

The max latency of 5,130ms is the direct fingerprint of the 5-second container pause. That number exists because k6 was running real queries against PostgreSQL when the container froze. Without load, the experiment would have reported "completed" with no evidence of impact.

---

## SQL Analytics

Load results flow into DuckDB alongside experiment and activity data:

```bash
tumult analyze --query "
  SELECT e.title, l.tool, l.vus, l.latency_p95_ms, l.error_rate
  FROM experiments e
  JOIN load_results l ON e.experiment_id = l.experiment_id
  ORDER BY l.latency_p95_ms DESC
"
```

Or use the default summary:

```bash
tumult analyze
```

```
Experiment: PostgreSQL under k6 load — container pause disruption
Status:     PASS (10687ms)

Timeline:
  ├─ pg-responds (probe) (hypothesis before)  115ms
  ├─ connection-count-before (probe)  3065ms  → 6
  ├─ pause-postgres-5s (action)  5354ms
  ├─ connection-count-after (probe)  92ms  → 6
  └─ pg-responds (probe) (hypothesis after)  55ms

Load Test (k6):
  VUs: 5  Duration: 10.5s  Requests: 300
  Latency: p50=101ms  p95=157ms
  Throughput: 29 req/s  Error rate: 0.003
  Thresholds: PASS
```

---

## OTel Span Attributes

The `resilience.load` span carries the full result as attributes:

```
resilience.load.tool:            k6
resilience.load.vus:             5
resilience.load.throughput_rps:  29.0
resilience.load.latency_p50_ms:  101.0
resilience.load.latency_p95_ms:  157.0
resilience.load.error_rate:      0.003
resilience.load.total_requests:  300
resilience.load.thresholds_met:  true
resilience.load.duration_s:      10.5
```

These flow to SigNoz, Jaeger, or any OTLP backend — queryable alongside the experiment trace.

---

## When Load Matters

Some experiments need load to produce meaningful results:

- **Connection pool exhaustion** — killing idle connections has no effect without active traffic
- **Network latency injection** — p95 impact is only measurable with concurrent requests
- **CPU/memory stress** — degradation manifests as increased response times under load
- **Failover testing** — client-side impact (retries, timeouts) only visible with active sessions

Other experiments are meaningful without load:

- **Pod deletion** — Kubernetes scheduler behavior is independent of traffic
- **Node drain** — pod rescheduling happens regardless of load
- **Data integrity checks** — corruption detection doesn't need concurrent writes

Tumult makes load optional. Add it when the experiment question is "how does this affect users?" Leave it off when the question is "does the infrastructure mechanism work?"

---

## Try It

```bash
git clone https://github.com/mwigge/tumult.git && cd tumult && ./install.sh
make up-targets
tumult run examples/pg-under-load.toon
tumult analyze
```

The numbers tell the story.

*Try Tumult at [tumult.rs](https://tumult.rs)*

*Next in the series: [Part 14 — GameDay Is Here →](./14-gameday-is-here.md)*

---
