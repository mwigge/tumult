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
   when it started: the version of the target's most recent span at or
   before the run's start. A rollback is therefore handled like any other
   deploy — after 1.1.0 is rolled back to 1.0.0, a new run belongs to 1.0.0
   again, although 1.1.0 was first seen later. Runs older than every known
   version are ignored.
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

## The endpoints

```
GET  /api/perf/{target}?probe=&quantile=&confidence=&min_samples=&margin_pct=&env=
POST /api/perf/{target}/verdicts?probe=&quantile=&confidence=&min_samples=&margin_pct=&env=
```

| Parameter | Default | Meaning |
|---|---|---|
| `probe` | `load.latency_p95_ms` | The `probe_samples` series compared |
| `quantile` | `p95` | `p50`, `p95` or `p99` |
| `confidence` | `0.95` | Bootstrap interval level, in (0, 1) |
| `min_samples` | `20` | Fewer on either side is inconclusive |
| `margin_pct` | `5` | Noise margin as a percentage of the baseline quantile |
| `env` | all in scope | Restrict the versions, runs and history to one environment (403 outside a scoped token's environments, 400 when empty) |

The answer carries both sides (`version`, `runs`, `samples`, `p50`/`p95`/
`p99`), the `comparison` (`delta`, `delta_pct`, `ci_low`, `ci_high`,
`margin`, `iterations`, `seed`), `verdict`, `reasons`, the `versions_seen`
with their first/last timestamps, `stored`, and `history` — the last ten
stored verdicts for the target. An unknown target (no `service.version`
seen in the environments asked for) is a 404 that names those environments;
a target with one version is a 200 `inconclusive`.

`GET` is a pure read (viewer role): dashboards can ask as often as they
like without growing the lake, and no query parameter makes it write.
`POST …/verdicts` (operator role, in the route table like every other
mutation) computes the same answer and appends it to the lake's
`perf_verdicts` table (schema v18): target, probe, both versions, both
sides' percentiles, the interval, the margin, the verdict and its reasons,
when it was computed, and the environments it was computed under (`envs`,
null when the caller was unscoped and passed no `env`). It answers 201 with
`stored: true`, 409 when there is only one version and so nothing to store,
and 503 when the daemon has no writable lake. Rows are never updated, so a
recomputation after more runs adds a newer row and the older one stays.

### Environments

Environment scoping applies to everything the answer is built from: the
versions, the runs, and the stored verdicts (a scoped token sees only the
rows whose `envs` lie inside its scopes; a verdict stored across every
environment is not shown to it).

A span's environment is the first of these that is present, compared
exactly (no case folding, no aliases — `prod` and `production` are two
environments):

1. the span attribute `resilience.target.environment`
2. the resource attribute `resilience.target.environment`
3. the resource attribute `deployment.environment.name`
4. the resource attribute `deployment.environment`

A service whose telemetry carries none of them is in no environment: an
unscoped request without `env` still sees it, but `?env=staging` or a
scoped token gets a 404 saying that no `service.version` was seen for the
target *in environment staging* and listing the attributes above. That is
deliberately fail-closed — untagged telemetry is never assumed to belong to
the environment someone asked for.

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
Without `--store` the command only reads (`GET`); with it the command
`POST`s to `/api/perf/{target}/verdicts` and the daemon appends the verdict
to `perf_verdicts`, which is what the pipeline step should do. `--store`
exits non-zero whenever the daemon did not store — a viewer token (403), a
single version (409), or an answer without `stored: true` — whatever
`--fail-on` says. It slots into a pipeline next to `tumult gate` (see
`deploy/ci/`).

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
`statusCodeDistribution`; a request that got no answer within the request
timeout is a transport error. k6's is its own `http_req_failed` rate, which
by default counts answers outside 200–399 too but follows whatever
`http.setResponseCallback` the script installs — a script that declares 404
expected does not count it. Compare error rates between runs of the same
tool, not across tools.

## Slow tails

The oha adapter waits for the requests in flight at the end of each window
and measures them, so a build whose median is unchanged but whose every
fifth request is slow shows up in the per-window p95 and the verdict is
`regressed`. Requests that never return are counted as timeout errors; a
window in which nothing was answered has no latency sample at all, and a
run made only of such windows misses its thresholds and ends `deviated`
(see the [Load Testing Guide](load-testing-guide.md)).

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
