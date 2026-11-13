---
title: Performance Regression Verdicts
parent: Guides
nav_order: 8
---

# Performance Regression Verdicts

`kind: performance` experiments answer one question per build: did this
`service.version` get slower than the last one? Tumult answers it from the
latency samples the lake already holds, with a statistical verdict rather
than a mean-and-sigma guess, and keeps one row per build so the answer is
auditable later.

## How a verdict is computed

1. **Versions.** A target's versions are the `service.version` resource
   values on the telemetry whose `service.name` is the target — the
   service's own spans arriving over OTLP. The two most recently seen
   versions are the *candidate* (newest) and the *baseline*.
2. **Runs.** Every experiment run against the target (root spans with
   `resilience.target.system`) is attributed to the version that was live
   when it started: the one whose first-seen to last-seen window contains the
   start (the later-deployed one when two overlap, as during a rollout or
   after a rollback). A run that falls in a telemetry gap goes to the most
   recent version first seen before it. Runs older than every known version
   are ignored.
3. **Samples.** For each side, the `during`-phase `probe_samples` rows of its
   runs for one series — by default `load.latency_p95_ms`, the per-window p95
   a streaming load adapter writes (see the
   [Load Testing Guide](load-testing-guide.md)). Only the oha adapter
   streams windows today, so the default series exists for `tool: oha` runs
   only; a k6 run keeps one end-of-run summary and needs `probe` pointed at a
   series it does have. Any series works: a
   hypothesis probe's duration (`probe_name` = the probe's name) or another
   `load.*` metric.
4. **Statistics** (`tumult-perf`). Nearest-rank p50/p95/p99 of each side, the
   candidate − baseline difference at the chosen quantile (p95 by default),
   and a percentile-bootstrap confidence interval on that difference: 2000
   resamples with replacement from a seeded generator, so the same inputs
   always give the same interval.
5. **Verdict** against a noise margin (5 % of the baseline quantile by
   default):

   | Verdict | Rule |
   |---|---|
   | `regressed` | the whole interval lies above `+margin` |
   | `improved` | the whole interval lies below `−margin` |
   | `unchanged` | the interval is inside the margin, or includes zero and reaches past the margin on at most one side |
   | `inconclusive` | either side has no samples or fewer than `min_samples` (20), only one version seen, an interval so wide it reaches past the margin in both directions, or an interval that excludes zero but straddles the margin (for example `[4, 12]` against a margin of 5: the shift is significant, its size against the margin is not resolved) |

   A tiny but statistically significant shift inside the margin is
   `unchanged` on purpose: the margin is where you say what you do not care
   about.

## The endpoint

```
GET /api/perf/{target}?probe=&quantile=&confidence=&min_samples=&margin_pct=&env=&store=
```

| Parameter | Default | Meaning |
|---|---|---|
| `probe` | `load.latency_p95_ms` | The `probe_samples` series compared |
| `quantile` | `p95` | `p50`, `p95` or `p99` |
| `confidence` | `0.95` | Bootstrap interval level, in (0, 1) |
| `min_samples` | `20` | Fewer on either side is inconclusive |
| `margin_pct` | `5` | Noise margin as a percentage of the baseline quantile |
| `env` | all in scope | Restrict the versions, runs and history to one environment (403 outside a scoped token's environments) |
| `store` | `false` | `true` appends the verdict to `perf_verdicts`; needs the operator role (403 for a viewer) |

The answer carries both sides (`version`, `runs`, `samples`, `p50`/`p95`/
`p99`), the `comparison` (`delta`, `delta_pct`, `ci_low`, `ci_high`,
`margin`, `iterations`, `seed`), `verdict`, `reasons`, the `versions_seen`
with their first/last timestamps, `stored`, and `history` — the last ten
stored verdicts for the target. An unknown target (no `service.version`
ever seen) is a 404; a target with one version is a 200 `inconclusive`.

The request is a pure read by default: dashboards and viewers can ask as
often as they like without growing the lake. `store=true` appends the
verdict to the lake's `perf_verdicts` table (schema v18): target, probe,
both versions, both sides' percentiles, the interval, the margin, the
verdict and its reasons, when it was computed, and the environments it was
computed under (`envs`, null when the caller was unscoped and passed no
`env`). Storing is kept on the `GET` so the stored row is exactly the answer
returned, but it is a mutation and is role-checked as one: operator and
above. Rows are never updated, so a recomputation after more runs adds a
newer row and the older one stays.

Environment scoping applies to everything the answer is built from. A
token scoped to `staging` sees only the versions whose telemetry carries
`resilience.target.environment = staging`, only the runs against that
environment, and only the stored verdicts whose `envs` lie inside its
scopes; a verdict stored across every environment is not shown to a scoped
token.

## The CLI

```bash
export TUMULT_DAEMON_URL=http://tumultd.internal:4318
export TUMULT_DAEMON_TOKEN=kro_...   # viewer reads; --store needs operator
tumult perf checkout --quantile p95 --margin-pct 5 --env staging --store
```

```
perf checkout: REGRESSED (p95 of load.latency_p95_ms)
  baseline  1.4.0: 100.0 ms over 40 sample(s)
  candidate 1.5.0: 150.0 ms over 40 sample(s)
  delta +50.0 ms (+50.0%), CI [31.0, 70.0], margin ±5.0
  - p95 difference interval [31.00, 70.00] against a noise margin of ±5.00 lies entirely above the margin
  verdict stored in perf_verdicts
```

`--fail-on regressed` (default) exits non-zero on a regression;
`--fail-on inconclusive` also fails when there is not enough evidence to
say; `--fail-on never` only prints. `--json` prints the daemon's answer.
Without `--store` the command only reads; with it the daemon appends the
verdict to `perf_verdicts`, which is what the pipeline step should do. It
slots into a pipeline next to `tumult gate` (see `deploy/ci/`).

## A pipeline shape that works

1. Deploy build *N* of `checkout` to staging; its spans arrive with
   `service.version = N`.
2. Run a `kind: performance` (or `load`) experiment against `checkout` with
   `tool: oha` and a soak profile long enough for 20+ windows. The default
   series (`load.latency_p95_ms`) exists only for oha runs; with k6, pass
   `--probe` a series the run does write, such as a hypothesis probe's name.
3. `tumult perf checkout --env staging --store` compares *N* against *N−1*,
   records the verdict and fails the job on `regressed`.

## Error rates: oha and k6 differ

The oha adapter's `error_rate` is (transport errors + answers outside
2xx/3xx) / requests, counted from oha's `errorDistribution` and
`statusCodeDistribution`. k6's is its own `http_req_failed` rate, which by
default counts answers outside 200–399 too but follows whatever
`http.setResponseCallback` the script installs — a script that declares 404
expected does not count it. Compare error rates between runs of the same
tool, not across tools.

## What is deferred

Two items of the Phase 4 plan depend on the forecast crate
(`tumult-forecast`, Phase 3 of the revised plan) that does not exist yet
and are **not** stubbed here:

- **Soak drift via forecast** — a soak profile now streams a time series
  (`load.*` samples every `sample_interval_s`), which is the input; the
  drift verdict (is the series trending away from its first hour faster
  than the forecast's band allows) waits for a `ForecastProvider`.
- **NFR headroom forecasts** — the objective verdicts carry a signed
  `margin` today; projecting when that margin reaches zero under the
  forecast trend is the same dependency.

Both become small consumers of the forecast crate once it lands; nothing in
the schema or the API shape here needs to change for them.
