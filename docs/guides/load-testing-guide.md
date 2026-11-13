---
title: Load Testing Guide
parent: Guides
nav_order: 7
---

# Load Testing Guide

Resilience testing under realistic traffic — combining chaos with load simulation.

## Why Test Under Load?

A system that handles faults at idle may fail under production traffic. Load testing during chaos experiments reveals:
- **Degradation curves** — how performance degrades under fault
- **Recovery behavior** — how quickly throughput recovers after fault removal
- **Cascading failures** — faults that only manifest under load (connection pool exhaustion, thread starvation)

## Supported Tools

| Tool | Best For | Metrics | Time series |
|------|----------|---------|-------------|
| **k6** | HTTP/gRPC APIs, scripted scenarios | req duration, error rate, throughput, VUs | end-of-run summary only |
| **oha** | One HTTP endpoint, no script | req duration percentiles, error rate, throughput | one sample per window (soak: every `sample_interval_s`) |

Both adapters implement the same `LoadExecutor` boundary and consume the same
traffic plan, so a load profile (ramp, spike, soak) means the same thing on
either tool. `load_executor_for(tool)` picks the adapter; JMeter is accepted
by the format for older files but has no adapter and fails at start.

## Load Profiles

```toon
load:
  tool: k6
  script: load/api-test.js
  vus: 50
  duration_s: 180
  profile:
    type: ramp        # or spike / soak
    start_vus: 5
    ramp_up_s: 60
```

| Profile | Fields | What runs |
|---|---|---|
| `ramp` | `start_vus`, `ramp_up_s` | k6: `--vus 5 --stage 60s:50 --stage 120s:50`; oha: one window per stage, the ramp window at its midpoint concurrency (or interpolated per sample interval) |
| `spike` | `peak_vus`, `after_s`, `hold_s` | k6: a one-second jump to the peak and back (`--stage 20s:10 --stage 1s:200 --stage 9s:200 --stage 1s:10 --stage 29s:10`); oha: three windows |
| `soak` | `sample_interval_s` | k6: one long stage; oha: one process per interval, each a sample |

A profile requires `duration_s`; `tumult validate` rejects a ramp longer than
the run, a spike that does not fit or does not rise above `vus`, and a sample
interval longer than the run. Full field reference in the
[Experiment Format](experiment-format.md#load).

Samples stream into the lake as `probe_samples` rows named `load.throughput_rps`,
`load.latency_p50_ms`, `load.latency_p95_ms`, `load.latency_p99_ms` and
`load.error_rate` (phase `during`), and the per-window p95 is what the
[performance regression verdict](performance-regression.md) compares between
builds.

## Integration Pattern

Load tools run as **background activities** alongside fault injection:

```
1. Start load tool (background)
2. Wait for traffic to stabilize (~30s)
3. Inject fault (chaos action)
4. Observe system under load + fault
5. Stop load tool
6. Collect load metrics
```

## k6 Quick Start

```bash
# 1. Write a k6 script
cat > load/api-test.js << 'EOF'
import http from 'k6/http';
import { check } from 'k6';

export default function () {
    const res = http.get('http://localhost:8080/api/health');
    check(res, { 'status 200': (r) => r.status === 200 });
}
EOF

# 2. Reference it in your experiment
tumult run experiment-with-load.toon
```

## Key Environment Variables

### k6

| Variable | Default | Description |
|----------|---------|-------------|
| `TUMULT_K6_SCRIPT` | (required) | Path to k6 test script |
| `TUMULT_K6_VUS` | 10 | Virtual users |
| `TUMULT_K6_DURATION` | 30s | Test duration |
| `TUMULT_K6_BINARY` | k6 | k6 binary path |
| `TUMULT_OTEL_ENDPOINT` | (none) | OTLP endpoint for trace correlation |

### oha

| Variable | Default | Description |
|----------|---------|-------------|
| `TUMULT_OHA_BINARY` | oha | oha binary path |

oha needs no script: `load.script` is the URL. The adapter runs
`oha --no-tui -j -c <vus> -z <window> <url>` per window and reads the JSON
summary (`summary.successRate`, `summary.requestsPerSec`,
`latencyPercentiles.p50/p95/p99`, `statusCodeDistribution`,
`errorDistribution`). Percentiles of the whole run are request-weighted means
of the window percentiles, since oha does not expose raw latencies; the
windows themselves are kept exactly. `thresholds` are checked by the adapter
against the aggregate (see the experiment format), because oha has no
threshold language of its own. On `stop`, windows not yet started are
skipped and the in-flight one gets ten seconds to finish before it is killed.

```bash
tumult run examples/checkout-spike-oha.toon
tumult run experiment.toon --load oha --load-script http://127.0.0.1:8080/health --load-vus 20 --load-duration 60s
```
