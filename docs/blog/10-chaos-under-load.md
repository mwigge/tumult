---
title: "Chaos Under Load: Network Faults and Load Testing"
parent: Blog
nav_order: 10
---

# <img src="/images/tumult.png" alt="Tumult Logo" width="100" valign="middle"> Chaos Under Load: Network Faults and Load Testing with tumult-network and tumult-loadtest

![Tumult Banner](/images/tumult-banner.png)

*Part 10 of the Tumult series. [← Part 9: Compliance as Code](./09-regulatory-compliance.md)*

---

Most chaos engineering tooling treats faults and load as separate concerns. You inject a fault, observe what breaks, roll back, and write up the results. Load tests are a different discipline — they live in a different tool, run on a different schedule, and produce data that never meets your fault injection data.

This separation creates a blind spot. Production failures rarely happen to idle systems. The database that falls over under a failover was serving 3,000 requests per second when it went. The network partition that cascaded into an outage hit during peak traffic. Testing faults in isolation tells you what breaks; testing faults under realistic load tells you whether your system actually survives.

`tumult-network` and `tumult-loadtest` close that gap. Together, they let you inject realistic network faults while real traffic is running — and capture both streams of data in the same TOON journal, correlated through the same OTel trace.

---

## tumult-network: Fault Injection at the Packet Level

`tumult-network` is a script-based plugin that operates at the OS networking layer. It uses two standard Linux tools:

- **`tc netem`** — the kernel's built-in network emulation subsystem, available via iproute2. Adds latency, packet loss, and corruption directly on the interface.
- **`iptables`** — kernel-level packet filtering. Used for DNS blocking and host partitioning.

Because the faults are applied at the OS level, they affect all traffic on the interface regardless of protocol or application. The plugin does not require application modifications, sidecar proxies, or service mesh configuration.

### Prerequisites

- Linux host with `iproute2` and `iptables` installed
- Root or sudo access (required for `tc` and `iptables` rules)
- Probes (`ping-latency`, `dns-resolve`) work on Linux and macOS

### Available Actions

| Action | Mechanism | What It Simulates |
|--------|-----------|-------------------|
| `add-latency` | `tc netem delay` | Cross-region links, degraded WAN, satellite, throttled CDN |
| `add-packet-loss` | `tc netem loss` | Intermittent connectivity, wireless interference, congested uplinks |
| `add-corruption` | `tc netem corrupt` | Faulty cables, bad NICs, hardware-level bit errors |
| `reset-tc` | `tc qdisc del` | Rollback — removes all netem rules from the interface |
| `block-dns` | `iptables DROP` on UDP/TCP 53 | DNS outage, split-horizon failure, resolver unavailability |
| `partition-host` | `iptables DROP` by destination IP | Network split, firewall misconfiguration, VPC route failure |

### Available Probes

| Probe | What It Measures | Output |
|-------|-----------------|--------|
| `ping-latency` | Round-trip latency to a target host | Float (ms) |
| `dns-resolve` | Whether a hostname resolves, and to what IP | IP string or `"failed"` |

### Adding Latency

A minimal latency injection that simulates a 200ms cross-region link with 20ms of jitter:

```toon
method[1]:
  - name: inject-latency
    activity_type: action
    provider:
      type: process
      path: plugins/tumult-network/actions/add-latency.sh
      env:
        TUMULT_INTERFACE: eth0
        TUMULT_DELAY_MS: 200
        TUMULT_JITTER_MS: 20
        TUMULT_TARGET_IP: 10.0.1.50

rollbacks[1]:
  - name: cleanup-latency
    activity_type: action
    provider:
      type: process
      path: plugins/tumult-network/actions/reset-tc.sh
      env:
        TUMULT_INTERFACE: eth0
```

`TUMULT_TARGET_IP` is optional — omit it to apply latency to all traffic on the interface. Include it to target a specific peer (a database, a downstream service, a specific pod IP).

The rollback calls `reset-tc.sh`, which removes all netem rules from the interface. Always register this as a rollback; `tc netem` rules are persistent across the experiment unless explicitly cleared.

### Packet Loss

```toon
method[1]:
  - name: inject-packet-loss
    activity_type: action
    provider:
      type: process
      path: plugins/tumult-network/actions/add-packet-loss.sh
      env:
        TUMULT_INTERFACE: eth0
        TUMULT_LOSS_PCT: 5
        TUMULT_CORRELATION: 25
```

`TUMULT_CORRELATION` (0–100) controls temporal correlation between dropped packets — a value of 25 means each drop is 25% correlated with the previous one, simulating burst loss rather than uniform random loss. This matters: TCP behaves differently under burst loss than under uniform loss, and burst loss is what you see in practice on congested links.

### Partitioning a Host

A partition drops all packets to or from a target IP, simulating a hard network split:

```toon
method[1]:
  - name: partition-db
    activity_type: action
    provider:
      type: process
      path: plugins/tumult-network/actions/partition-host.sh
      env:
        TUMULT_TARGET_IP: 10.0.1.50
        TUMULT_DIRECTION: both
```

`TUMULT_DIRECTION` accepts `inbound`, `outbound`, or `both`. A one-directional partition (outbound drops, inbound still arrives) produces an asymmetric failure mode that some applications handle very differently than a full blackout.

Rollback for iptables rules requires removing rules by their comment tag:

```sh
iptables -D OUTPUT -m comment --comment "tumult-partition" -j DROP
```

### Probing Network Conditions

The two probes let you measure network state during BASELINE and DURING phases, giving you before/after comparison data in the journal:

```toon
steady-state[2]:
  - name: baseline-latency
    activity_type: probe
    provider:
      type: process
      path: plugins/tumult-network/probes/ping-latency.sh
      env:
        TUMULT_TARGET_HOST: 10.0.1.50

  - name: baseline-dns
    activity_type: probe
    provider:
      type: process
      path: plugins/tumult-network/probes/dns-resolve.sh
      env:
        TUMULT_DNS_HOST: payments.internal
```

Both probes run in `steady-state` (BASELINE phase) before the fault is injected, and again in `after` (POST phase) after rollback. The journal records both values, and the analytics pipeline surfaces the delta automatically.

---

## tumult-loadtest: Sustained Traffic During Fault Injection

`tumult-loadtest` integrates k6 and JMeter with the Tumult experiment lifecycle. The key design detail is `background: true` — load generators run as background processes while the rest of the experiment proceeds through fault injection, probing, and rollback. When the experiment completes, the rollback phase stops the load generator and collects its output metrics.

### k6 Integration

k6 is the primary driver. A k6 action starts the load generator in the background:

```toon
method[1]:
  - name: start-load
    activity_type: action
    provider:
      type: process
      path: plugins/tumult-loadtest/drivers/k6-start.sh
      env:
        TUMULT_K6_SCRIPT: load/payment-api.js
        TUMULT_K6_VUS: 50
        TUMULT_K6_DURATION: 5m
    background: true
```

`TUMULT_K6_VUS` sets virtual users (concurrent connections). `TUMULT_K6_DURATION` sets how long k6 runs — this should be longer than the full experiment duration so load continues through the fault window.

Stopping and collecting metrics in rollbacks:

```toon
rollbacks[2]:
  - name: stop-load
    activity_type: action
    provider:
      type: process
      path: plugins/tumult-loadtest/drivers/k6-stop.sh

  - name: collect-metrics
    activity_type: probe
    provider:
      type: process
      path: plugins/tumult-loadtest/drivers/k6-metrics.sh
```

`k6-stop.sh` sends a signal to terminate the k6 process. `k6-metrics.sh` reads k6's output and returns the summary — p95 latency, error rate, throughput — as probe output recorded in the journal.

### JMeter Integration

JMeter follows the same pattern with different driver scripts and variables:

```toon
method[1]:
  - name: start-load
    activity_type: action
    provider:
      type: process
      path: plugins/tumult-loadtest/drivers/jmeter-start.sh
      env:
        TUMULT_JMETER_PLAN: load/test-plan.jmx
        TUMULT_JMETER_THREADS: 20
        TUMULT_JMETER_DURATION: 300
    background: true
```

`TUMULT_JMETER_THREADS` sets concurrent threads. `TUMULT_JMETER_DURATION` is in seconds. The stop and metrics collection pattern mirrors k6.

### OTLP Correlation

When `TUMULT_OTEL_ENDPOINT` is set, k6 exports its metrics through the same OTel Collector pipeline as Tumult's experiment spans:

```bash
TUMULT_OTEL_ENDPOINT=http://localhost:4317 tumult run experiment.toon
```

This means load test metrics (latency percentiles, error rates, throughput) land in the same trace backend as fault injection events. In Jaeger or Grafana Tempo, you can see exactly when the fault was injected on the same timeline as the degradation in p95 latency. This is the correlation that makes post-experiment analysis meaningful.

---

## Full Example: API Resilience Under Database Failover and Network Degradation

The scenario: a payment API must continue handling traffic when its primary database becomes unreachable and the network is experiencing 100ms of added latency. This combines both plugins with a postgres connection kill from `tumult-db-postgres`.

```toon
title: Payment API survives DB failover and network degradation under load
description: Validates that the payment service continues processing requests when the primary database fails over under a degraded network connection.
contributions:
  - DORA Article 25.1 ICT continuity

steady-state[4]:
  - name: baseline-api-health
    activity_type: probe
    provider:
      type: process
      path: plugins/tumult-process/probes/http-status.sh
      env:
        TUMULT_HTTP_URL: http://localhost:8080/health
        TUMULT_HTTP_EXPECTED_STATUS: 200

  - name: baseline-db-connections
    activity_type: probe
    provider:
      type: process
      path: plugins/tumult-db-postgres/probes/connection-count.sh
      env:
        TUMULT_PG_DATABASE: payments

  - name: baseline-network-latency
    activity_type: probe
    provider:
      type: process
      path: plugins/tumult-network/probes/ping-latency.sh
      env:
        TUMULT_TARGET_HOST: 10.0.1.50

  - name: baseline-dns
    activity_type: probe
    provider:
      type: process
      path: plugins/tumult-network/probes/dns-resolve.sh
      env:
        TUMULT_DNS_HOST: payments-db.internal

method[5]:
  - name: start-load
    activity_type: action
    provider:
      type: process
      path: plugins/tumult-loadtest/drivers/k6-start.sh
      env:
        TUMULT_K6_SCRIPT: load/payment-api.js
        TUMULT_K6_VUS: 100
        TUMULT_K6_DURATION: 8m
    background: true

  - name: warm-up
    activity_type: action
    provider:
      type: process
      path: sleep
      arguments[1]: "30"

  - name: degrade-network
    activity_type: action
    provider:
      type: process
      path: plugins/tumult-network/actions/add-latency.sh
      env:
        TUMULT_INTERFACE: eth0
        TUMULT_DELAY_MS: 100
        TUMULT_JITTER_MS: 10
        TUMULT_TARGET_IP: 10.0.1.50

  - name: wait-for-degradation
    activity_type: action
    provider:
      type: process
      path: sleep
      arguments[1]: "15"

  - name: kill-db-connections
    activity_type: action
    provider:
      type: process
      path: plugins/tumult-db-postgres/actions/kill-connections.sh
      env:
        TUMULT_PG_DATABASE: payments

rollbacks[4]:
  - name: restore-network
    activity_type: action
    provider:
      type: process
      path: plugins/tumult-network/actions/reset-tc.sh
      env:
        TUMULT_INTERFACE: eth0

  - name: stop-load
    activity_type: action
    provider:
      type: process
      path: plugins/tumult-loadtest/drivers/k6-stop.sh

  - name: collect-metrics
    activity_type: probe
    provider:
      type: process
      path: plugins/tumult-loadtest/drivers/k6-metrics.sh

  - name: post-api-health
    activity_type: probe
    provider:
      type: process
      path: plugins/tumult-process/probes/http-status.sh
      env:
        TUMULT_HTTP_URL: http://localhost:8080/health
        TUMULT_HTTP_EXPECTED_STATUS: 200

after[3]:
  - name: post-network-latency
    activity_type: probe
    provider:
      type: process
      path: plugins/tumult-network/probes/ping-latency.sh
      env:
        TUMULT_TARGET_HOST: 10.0.1.50

  - name: post-dns
    activity_type: probe
    provider:
      type: process
      path: plugins/tumult-network/probes/dns-resolve.sh
      env:
        TUMULT_DNS_HOST: payments-db.internal

  - name: post-db-connections
    activity_type: probe
    provider:
      type: process
      path: plugins/tumult-db-postgres/probes/connection-count.sh
      env:
        TUMULT_PG_DATABASE: payments
```

### What the journal captures

When this experiment runs, the TOON journal records:

- BASELINE probe values: API HTTP 200, DB connection count, network latency ~0ms, DNS resolves to correct IP
- DURING: load generator active (100 VUs), network degraded (+100ms), DB connections killed
- POST/rollback: k6 summary metrics (p50/p95/p99 latency, error rate, throughput), network restored, API health restored
- AFTER: network latency back to baseline, DNS still resolving, DB connection count recovering

The `collect-metrics` probe output — k6's summary JSON — is embedded in the journal and queryable through the analytics pipeline:

```sql
SELECT
  j.experiment_id,
  j.started_at,
  json_extract(p.output, '$.http_req_duration.p95') AS p95_ms,
  json_extract(p.output, '$.http_req_failed.rate') AS error_rate
FROM journals j
JOIN probes p ON p.journal_id = j.id AND p.name = 'collect-metrics'
ORDER BY j.started_at DESC;
```

---

## Sequencing Matters

The 30-second warm-up (`sleep 30`) before fault injection is deliberate. k6 needs time to ramp its virtual users to full load before the fault lands. Without it, you might be injecting faults during the ramp-up phase, before traffic is at steady state — and the results will not be representative.

The ordering in `method` is:

1. Start load generator (background)
2. Wait for steady-state traffic
3. Inject network degradation
4. Wait briefly for degradation to establish
5. Inject database fault

Layering faults this way — network first, then database — gives you data about each fault in isolation before you combine them. The phase markers in the OTel trace show exactly when each fault was applied, so you can see which degradation in the k6 metrics corresponds to which fault.

---

## What to Look For in the Data

After the experiment runs, the analytics pipeline (covered in [Part 6](./06-analytics-pipeline.md)) lets you query across the five data phases. The questions to answer:

**Did the API maintain availability?**
```sql
SELECT phase, output
FROM probes
WHERE experiment_id = 'payment-api-db-failover-under-load'
  AND name = 'baseline-api-health'
ORDER BY phase;
```

**How did p95 latency change under the combined fault?**
```sql
SELECT
  json_extract(output, '$.http_req_duration.p50') AS p50_ms,
  json_extract(output, '$.http_req_duration.p95') AS p95_ms,
  json_extract(output, '$.http_req_duration.p99') AS p99_ms
FROM probes
WHERE name = 'collect-metrics';
```

**Did network conditions fully recover post-rollback?**
```sql
SELECT name, phase, output
FROM probes
WHERE name IN ('baseline-network-latency', 'post-network-latency')
ORDER BY phase;
```

If `post-network-latency` matches `baseline-network-latency`, `reset-tc` cleaned up completely. If it does not, there is a residual netem rule that was not removed — a signal to investigate the rollback.

---

## Common Fault Combinations

`tumult-network` and `tumult-loadtest` compose with every other Tumult plugin. Some scenarios that come up frequently:

| Fault combination | Plugins | What you learn |
|-------------------|---------|---------------|
| Kafka broker kill + 50ms latency to replicas | tumult-kafka + tumult-network | Does your Kafka producer retry successfully when replication is slow? |
| Redis eviction + 10% packet loss to cache | tumult-db + tumult-network | Does your application degrade gracefully when cache reads are unreliable? |
| Pod deletion + 200ms network jitter + k6 at 500 VUS | tumult-kubernetes + tumult-network + tumult-loadtest | Does your K8s deployment recover within SLA while traffic continues? |
| DNS block + JMeter at steady load | tumult-network + tumult-loadtest | Does your service discovery fall back correctly when DNS is unavailable? |

The TOON format makes these compositions straightforward — each plugin contributes its action and probe steps independently, and `method`, `rollbacks`, and `after` sections compose cleanly across plugins within a single experiment file.

---

## Linux-Only Limitation

The network fault actions require Linux — `tc netem` and `iptables` are not available on macOS or Windows. The probes (`ping-latency`, `dns-resolve`) work cross-platform.

For teams developing on macOS, the typical pattern is to run experiments against a Linux target via SSH using `tumult-ssh`, rather than running the network fault actions locally. The experiment file is identical; only the execution host changes.

---

Chaos engineering at the level of individual fault injection is a starting point. The real question — the one production failures actually ask — is: does your system hold together under realistic conditions, with real traffic, on a degraded network, while multiple things are failing at the same time?

`tumult-network` and `tumult-loadtest` are the plugins that let you ask that question and get a structured, queryable, OTel-correlated answer.

---

*Next in the series: [Part 11 — The Road Ahead: Autonomous Chaos, MCP, and the Future of Resilience Engineering →](./11-road-ahead.md)*
