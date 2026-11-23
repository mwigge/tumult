---
title: Objectives
parent: Run experiments
grand_parent: Guides
nav_order: 3
summary: "Say which numbers a run must meet"
---

# Objectives

An objective is a number a run must meet, such as a p95 latency under
250 ms. Each run checks its objectives and gives each one a verdict.

![A run page with one objective: its metric, target, observed value and verdict](../images/3.0/screenshots/experiment-evidence.png)

## Write an objective

Objectives go in the `nfr` list of the [experiment file](experiment-format.md).

```toon
nfr[2]:
  - id: p95
    kind: latency
    metric: load.latency_p95_ms
    operator: lte
    threshold: 250
    criticality: blocking
  - id: avail
    kind: availability
    metric: during.availability
    operator: gte
    threshold: 0.995
```

| Field | Values |
|---|---|
| `id` | A name, unique in the file |
| `kind` | `latency`, `availability`, `throughput`, `error_rate`, `rto`, `rpo`, `capacity`, `recovery`, `data_integrity` |
| `metric` | What the run measured, see below |
| `operator` | `lte`, `gte` or `eq`, against `threshold` |
| `threshold` | A number |
| `criticality` | `blocking`, `major` (default) or `minor` |
| `window_s`, `description` | Notes. The run checks the whole phase that made the metric |

## Verdicts

| Verdict | When |
|---|---|
| `met` | The measured value meets the threshold |
| `breached` | It does not |
| `inconclusive` | The run did not measure the metric. It is never counted as met |

A breached `blocking` objective turns a completed run into `deviated`. A
`major` or `minor` breach is recorded and leaves the status alone. The
verdicts are in the journal (`objectives.verdicts`) and in the lake table
`nfr_verdicts`.

In a JUnit report (`tumult report --format junit`) each objective is a test
case. A breached `blocking` objective fails it, as it fails the run. An
`inconclusive` objective is skipped.

## Metrics

| Metric | Comes from |
|---|---|
| `load.throughput_rps`, `load.latency_p50_ms`, `load.latency_p95_ms`, `load.latency_p99_ms`, `load.error_rate`, `load.total_requests` | The [load](load-testing-guide.md) of the run |
| `recovery.mttr_s`, `recovery.time_s` | The recovery check right after the method |
| `during.error_rate`, `during.availability` | The hypothesis probes, sampled during the method |
| `post.error_rate`, `post.availability` | The hypothesis probes, sampled after the method |
| `probe.<name>.mean_ms`, `.max_ms`, `.p95_ms`, `.error_rate` | One hypothesis probe, sampled during the method |
| `fault.duration_s` | The length of the method |

Validation refuses any other metric, and a `probe.<name>` that is not a probe
of the steady-state hypothesis: no run could measure it.

## Kinds

The `kind` of an objective must be one that its metric measures.

| Kind | Metrics |
|---|---|
| `latency` | `load.latency_p50_ms`, `load.latency_p95_ms`, `load.latency_p99_ms`, `probe.<name>.mean_ms`, `.max_ms`, `.p95_ms` |
| `error_rate`, `availability` | `load.error_rate`, `during.error_rate`, `during.availability`, `post.error_rate`, `post.availability`, `probe.<name>.error_rate` |
| `throughput`, `capacity` | `load.throughput_rps`, `load.total_requests` |
| `rto` | `recovery.mttr_s`, `recovery.time_s` |
| `recovery` | `recovery.mttr_s`, `recovery.time_s`, `post.error_rate`, `post.availability`, `fault.duration_s` |
| `data_integrity` | `probe.<name>.error_rate`, for a probe that checks the data |
| `rpo` | None yet: no metric counts lost data |

Validation refuses any other pair, such as an `rpo` objective on
`fault.duration_s`.

## Recovery targets

`rto_target_s` adds a blocking objective: `recovery.mttr_s` at most that many
seconds. A run of `kind: dr` must set it.

A run that does not recover while the recovery check watches breaches its
RTO. The verdict says `recovered: false`, its `observed` is the time the check
watched, and it has no margin. The true recovery time is longer. A run whose
method failed, or that was stopped, measures no recovery: its RTO is
`inconclusive`.

`rpo_target_s` is recorded with the run. No metric counts lost data yet, so
no objective can measure it, and its verdict is `inconclusive`.

## Next

- [Load testing](load-testing-guide.md): the source of the `load.*` metrics.
- [Performance verdicts](performance-regression.md): compare runs between
  builds.
