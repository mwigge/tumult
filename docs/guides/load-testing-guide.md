---
title: Load testing
parent: Run experiments
grand_parent: Guides
nav_order: 4
summary: "Ramp, spike and soak tests with k6 and oha"
---

# Load testing

A fault that does no harm at idle can break a service under traffic. Add a
`load` section to an experiment, and Tumult runs k6 or oha while the fault is
on and keeps the numbers with the run.

![A soak run: latency and throughput for each window, during the fault](../images/3.0/screenshots/experiment-evidence.png)

## Add load to an experiment

```toon
load:
  tool: oha
  script: http://127.0.0.1:8080/checkout/1
  vus: 20
  duration_s: 120
  thresholds:
    error_rate: 0.02
```

| Field | Meaning |
|---|---|
| `tool` | `k6` or `oha` |
| `script` | The k6 script, or the URL that oha calls |
| `vus` | Virtual users (default 10) |
| `duration_s` | Seconds of load (default 30). A `profile` needs it |
| `thresholds` | Bounds for oha: `latency_p50_ms`, `latency_p95_ms`, `latency_p99_ms`, `error_rate` (upper) and `throughput_rps` (lower). k6 uses the thresholds of its script and ignores these, with a warning |
| `profile` | The shape of the traffic, see below |

The same from the command line, which replaces the `load` of the file:

```bash
tumult run experiment.toon --load oha --load-script http://127.0.0.1:8080/health --load-vus 20 --load-duration 60s
tumult run experiment.toon --load none   # run without the load of the file
```

## When the load runs

```mermaid
flowchart TD
    accTitle: When the load runs
    accDescr: The load starts after the first hypothesis check, runs through the method and the recovery check, and stops before the second hypothesis check and the rollbacks.
    before[Hypothesis, before] --> start[Load starts]
    start --> method[Method: the fault, under load]
    method --> recovery[Recovery check, still under load]
    recovery --> stop[Load stops]
    stop --> after[Hypothesis, after, then rollbacks]
```

Make the method as long as `duration_s`. When the method ends first, the run
does not wait: k6 gets 10 seconds to end, and oha stops after the window it
is in. The windows that never ran make the run `deviated`.

## Traffic profiles

| `type` | Fields | Shape |
|---|---|---|
| `ramp` | `start_vus`, `ramp_up_s` | From `start_vus` up to `vus` in `ramp_up_s`, then `vus` |
| `spike` | `peak_vus`, `after_s`, `hold_s`, `sample_interval_s` (optional) | `vus`, then `peak_vus` for `hold_s` after `after_s`, then `vus` again. With `sample_interval_s`, a window every `sample_interval_s` |
| `soak` | `sample_interval_s` | `vus` for the whole (long) run, with a sample every `sample_interval_s` |

```mermaid
%%{init: {"themeVariables": {"xyChart": {"plotColorPalette": "#1f2a44"}}}}%%
xychart-beta
    accTitle: A ramp profile
    accDescr: Virtual users rise from 5 to 50 over the first 60 seconds, then hold at 50 until 180 seconds.
    title "ramp: 5 to 50 users in 60 s, then hold (checkout-ramp-k6.toon)"
    x-axis "seconds" 0 --> 180
    y-axis "users" 0 --> 60
    line [5, 20, 35, 50, 50, 50, 50, 50, 50, 50]
```

```mermaid
%%{init: {"themeVariables": {"xyChart": {"plotColorPalette": "#1f2a44"}}}}%%
xychart-beta
    accTitle: A spike profile
    accDescr: Virtual users stay at 10, jump to 200 at 20 seconds for 10 seconds, then fall back to 10 until 60 seconds.
    title "spike: 10 users, 200 for 10 s at 20 s (checkout-spike-oha.toon)"
    x-axis "seconds" 0 --> 60
    y-axis "users" 0 --> 220
    line [10, 10, 10, 10, 200, 200, 10, 10, 10, 10, 10, 10, 10]
```

```mermaid
%%{init: {"themeVariables": {"xyChart": {"plotColorPalette": "#1f2a44"}}}}%%
xychart-beta
    accTitle: A soak profile
    accDescr: Virtual users stay at 20 for two hours, and oha takes a sample every 30 seconds.
    title "soak: 20 users for 2 hours, a sample every 30 s (checkout-soak-oha.toon)"
    x-axis "minutes" 0 --> 120
    y-axis "users" 0 --> 30
    line [20, 20, 20, 20, 20, 20, 20, 20, 20]
```

`tumult validate` refuses a ramp longer than the run, a spike that does not
fit or does not rise above `vus`, and a sample interval under 1 second or
longer than the run. The three examples are in the `examples/` folder.

## k6 or oha

| | k6 | oha |
|---|---|---|
| Input | A script | One URL |
| Samples | One summary at the end | One per window (a soak, or a spike with `sample_interval_s`: one per interval) |
| Thresholds | In the script | The `thresholds` of the load section |
| Binary | `TUMULT_K6_BINARY`, default `k6` | `TUMULT_OHA_BINARY`, default `oha` (1.x) |

oha runs one process per window and waits for the requests still in flight,
so a slow answer is measured, not dropped. A request gives up after the length
of its window (at most 30 seconds); `TUMULT_OHA_REQUEST_TIMEOUT_S` sets
another limit. The error rate counts transport errors and answers outside
2xx and 3xx.

## What the load does to the run

| The load | The run |
|---|---|
| Did not start, for example because the binary is missing | `failed` before the method, with no journal |
| Left no result, or sent no request | `failed` |
| Missed a threshold, or did not run its whole plan | `deviated` |
| Met its thresholds | Keeps its status |

A window in which nothing answered is recorded, but is no deviation by
itself. Declare `thresholds.error_rate` (oha) or a k6 threshold when it should
be. A guard halt or Ctrl-C stops the load before the rollbacks; the result is
marked `execution.aborted: true` and never meets its thresholds.

## Judge a soak or a spike

Three [objectives](objectives.md) read the windows of an oha load. k6 keeps
no windows, so with k6 their verdict is `inconclusive`.

| Metric | Load | What it is |
|---|---|---|
| `load.p95_drift_pct` | `soak` | How much the p95 grew, in percent. The median p95 of the last quarter of the windows against that of the first quarter |
| `load.error_rate_drift` | `soak` | How much the error rate rose. The median of the last quarter less that of the first quarter: `0.02` is 2 points |
| `load.spike_recovery_s` | `spike` with `sample_interval_s` | Seconds from the end of the spike to the first window whose p95 is back in the band. The band is up to 1.2 times the median p95 of the windows before the spike |

```mermaid
%%{init: {"themeVariables": {"xyChart": {"plotColorPalette": "#1f2a44"}}}}%%
xychart-beta
    accTitle: A soak that drifts
    accDescr: The p95 of twelve soak windows grows from about 100 ms to about 155 ms. The median of the first quarter, windows 1 to 3, is 100 ms. The median of the last quarter, windows 10 to 12, is 150 ms. The drift is 50 percent.
    title "p95 per window: first quarter 100 ms, last quarter 150 ms, drift 50 %"
    x-axis ["1 first", "2 first", "3 first", "4", "5", "6", "7", "8", "9", "10 last", "11 last", "12 last"]
    y-axis "p95 (ms)" 0 --> 180
    bar [100, 104, 98, 110, 115, 120, 126, 131, 138, 146, 150, 155]
```

```toon
nfr[2]:
  - id: p95-drift
    kind: latency
    metric: load.p95_drift_pct
    operator: lte
    threshold: 20
  - id: back-after-spike
    kind: recovery
    metric: load.spike_recovery_s
    operator: lte
    threshold: 10
```

- A drift needs at least four windows, one in each quarter. A window in
  which nothing was answered is not kept, so a drift does not see it.
- The recovery needs a window before the spike and one after it. A window
  that started during the spike is neither.
- With too few windows the verdict is `inconclusive`, with the reason
  `too few load windows to judge`.
- When no window after the spike is back in the band, the objective is
  `breached`. The verdict says `recovered: false`, and its `observed` is
  the time from the end of the spike to the end of the load. The true time
  is longer.
- `tumult validate` refuses a drift on a load that is not a soak, and the
  recovery on a load that is not a spike with `sample_interval_s`.

## Where the numbers go

- The journal keeps the `load_result`, with planned and completed windows.
- The lake keeps each oha window as `probe_samples` series in the `during`
  phase: `load.throughput_rps`, `load.latency_p50_ms`, `load.latency_p95_ms`,
  `load.latency_p99_ms`, `load.error_rate` and `load.window_s`.
- [Objectives](objectives.md) read the `load.*` metrics, and the
  [performance verdict](performance-regression.md) compares the p95 of each
  window between builds.

To start k6 as a step of the method instead, see the
[tumult-loadtest](../plugins/tumult-loadtest.md) plugin.
