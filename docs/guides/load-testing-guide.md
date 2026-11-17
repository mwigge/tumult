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
either tool. `load_executor_for(tool)` picks the adapter; JMeter still
parses for older files, but it has no adapter, so validation refuses it.

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
| `TUMULT_OHA_REQUEST_TIMEOUT_S` | the window's length, at most 30; three quarters of it for a window under 2 s | Seconds oha waits for one answer before counting the request as a `timeout` error |

oha needs no script: `load.script` is the URL. The adapter runs
`oha --no-tui --output-format json -c <vus> -z <window> -w -t <timeout> <url>`
per window (oha 1.x; older releases that only know `-j` are not supported)
and reads the JSON summary (`summary.requestsPerSec`,
`latencyPercentiles.p50/p95/p99`, `statusCodeDistribution`,
`errorDistribution`).

**Slow requests are measured, not dropped.** `-w` makes oha wait for the
requests still in flight when a window's time is up, so a slow answer lands
in that window's percentiles, and `-t` turns one that never comes into a
`timeout` error. A window can therefore outlast its slot by up to the
request timeout. By default that timeout is the window's own planned
length, never above 30 seconds, so a normal stop never waits half a minute
with the fault still injected. A window shorter than two seconds gives up
after three quarters of its length instead: a one-second sampling window
against a hung target ends within about 1.75 seconds, inside the first
half of the next window's slot, so one hung request cannot by itself cost
the next window its place. Set `TUMULT_OHA_REQUEST_TIMEOUT_S` when one
answer may legitimately take
longer. A window is killed (and reported as failed) only after its length
plus the timeout plus a ten-second grace. Each sample's `duration_s` is the
time the window really ran, not the planned length.

**The plan clock.** Windows are scheduled against the plan, not against
each other: window *n* owns the slot from its planned offset to its planned
end, measured from the start of the load.

| When its slot comes, the window … | It … |
|---|---|
| is early | waits for its slot |
| is on time (within 50 ms) | runs its full length |
| is late because the one before ran over, with at least half its slot left | runs for what is left of the slot |
| is late with less than half its slot left | is skipped |

A stretched window therefore costs the next one time, never the rest of the
plan its place: a ramp reaches its peak when the plan says so, and a
24-second soak with a slow tail still ends after about 24 seconds.

**Planned against completed.** The load result carries an `execution`
block in the journal: `planned_windows`, `completed_windows`,
`unanswered_windows`, `lost_to_unanswered`, `aborted`, and `reasons`; the
lake keeps them per run, with the time the load really sampled. A slot
lost behind a window in which nothing answered is part of that blackout
(`lost_to_unanswered`), not a truncation: it fails the run only through a
declared threshold, like the unanswered window itself. A plan that fell
short —
windows skipped because their predecessors overran, a window that failed,
windows never started because the method ended first — never meets its
thresholds (`thresholds_met: false`, the reason says how many of how many
windows were lost), so the run ends `deviated` instead of reporting a ramp
that never reached its peak as passed. A soak's
`sample_interval_s` below one second is rejected at validation, and the
planner never slices a stage into windows shorter than one second (a short
remainder joins the window before it), because a window that short measures
mostly its own start-up.

The error rate is (transport errors + answers outside 2xx/3xx) / requests —
oha's own `successRate` only knows about transport errors, so a run of 503s
would otherwise look clean. Should oha still report requests as
`aborted due to deadline`, one per connection is how a window ends and is
not counted; any beyond that are errors. A count oha prints that is not a
whole number fails the window instead of reading as zero. Percentiles of the
whole run are request-weighted means of the window percentiles, since oha
does not expose raw latencies; the windows themselves are kept exactly.
`thresholds` are checked by the adapter against the aggregate (see the
experiment format), because oha has no threshold language of its own.

**How a load ends.**

| The run … | The load … |
|---|---|
| reaches the end of its method | finishes the window in flight (a flat load is one window, so it runs its whole `duration_s`) and starts no further one |
| is halted by a guard, or interrupted (Ctrl-C, e-stop) | is aborted before the rollbacks run: the window in flight is interrupted, gets up to one second to report what it had measured, and its process group is then killed. The result keeps the completed windows and the partial one, and is marked `execution.aborted: true` |

Keep the method at least as long as `load.duration_s`: a method that ends
first leaves windows unstarted, which truncates the plan. An aborted load
is evidence, not a measurement of the plan: it never meets its thresholds,
a flat load cut short keeps the requests of its single partial window, and
a window whose requests were all still unanswered when it was cut reports
nothing. The `resilience.load` span carries `resilience.load.aborted`,
`resilience.load.truncated`, `resilience.load.planned_windows` and
`resilience.load.completed_windows` with every collected result, and a load
that left none still marks its span with `resilience.load.error` and
`resilience.load.aborted`;
both an aborted and a truncated load are logged as warnings.
Cancellation is checked between activities and during
`pause_before_s` / `pause_after_s`; an activity's own process that is already
running is not interrupted, so hold a long experiment open with a pause
rather than a `sleep` command.

**What the load does to the run.** A declared load is evidence the run
depends on:

| The load … | Run status |
|---|---|
| could not be collected (the tool failed, no window completed) or sent no request at all | `failed` |
| ran but missed a declared `thresholds` bound, or did not run its whole plan | `deviated` |
| met its thresholds | unchanged |

`thresholds_met` is true only when at least one request was sent, the whole
plan ran, and every declared bound holds. A window nobody answered (a
blackout while the fault is injected) is recorded and is not a deviation by
itself, with oha as with k6: declare `thresholds.error_rate` (oha) or a
threshold in the k6 script when it should be one. An interrupted k6 is
killed at once and leaves a result marked aborted with no measurements.

**When the load ends.** A run does not wait for its load: when the method
and the recovery check are done, k6 has 10 seconds to end by itself and oha
is stopped after the window it is in. Give the load the length of the method.
A GameDay shares one load between its experiments. When every experiment has
run and the load has time left, the GameDay waits for the load to end, so
its journal holds the whole load. A GameDay that is cancelled, or that an
experiment error stops, does not wait: it aborts the load as in the table
above, and the load result is marked `execution.aborted: true`.

**Status precedence.** When several things went wrong, the first that
applies names the run: `interrupted` (cancelled), `aborted` (the hypothesis
did not hold before the method), `failed` (an action failed, or the load
left no evidence), `deviated` (hypothesis after, a probe of the method, a
blocking objective, or the load's thresholds or plan), `completed`. The status never hides the
cleanup a lower one calls for: rollbacks run whenever the facts amount to a
deviation, whatever the status reads — a run that is `failed` because its
load left no evidence still rolls back as a deviated run would, also when its
method held only probes — and after a failed or interrupted run in which a
fault was started.

```bash
tumult run examples/checkout-spike-oha.toon
tumult run experiment.toon --load oha --load-script http://127.0.0.1:8080/health --load-vus 20 --load-duration 60s
```
