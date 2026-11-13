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
   `resilience.target.system`) is attributed to the most recent version that
   had been seen before the run started. Runs older than every known version
   are ignored.
3. **Samples.** For each side, the `during`-phase `probe_samples` rows of its
   runs for one series — by default `load.latency_p95_ms`, the per-window p95
   a streaming load adapter writes (see the
   [Load Testing Guide](load-testing-guide.md)). Any series works: a
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
   | `unchanged` | the interval is inside the margin, or includes zero without reaching past the margin on both sides |
   | `inconclusive` | fewer than `min_samples` (20) on either side, only one version seen, or an interval so wide it reaches past the margin in both directions |

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
| `env` | all in scope | Restrict the attributed runs to one environment (403 outside a scoped token's environments) |
| `store` | `true` | Append the verdict to `perf_verdicts` |

The answer carries both sides (`version`, `runs`, `samples`, `p50`/`p95`/
`p99`), the `comparison` (`delta`, `delta_pct`, `ci_low`, `ci_high`,
`margin`, `iterations`, `seed`), `verdict`, `reasons`, the `versions_seen`
with their first/last timestamps, `stored`, and `history` — the last ten
stored verdicts for the target. An unknown target (no `service.version`
ever seen) is a 404; a target with one version is a 200 `inconclusive`.

Every computed verdict is appended to the lake's `perf_verdicts` table
(schema v17): target, probe, both versions, both sides' percentiles, the
interval, the margin, the verdict and its reasons, and when it was computed.
Rows are never updated, so a recomputation after more runs adds a newer row
and the older one stays.

## The CLI

```bash
export TUMULT_DAEMON_URL=http://tumultd.internal:4318
export TUMULT_DAEMON_TOKEN=kro_...   # viewer role is enough
tumult perf checkout --quantile p95 --margin-pct 5 --env staging
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
say; `--fail-on never` only prints. `--json` prints the daemon's answer. It
slots into a pipeline next to `tumult gate` (see `deploy/ci/`).

## A pipeline shape that works

1. Deploy build *N* of `checkout` to staging; its spans arrive with
   `service.version = N`.
2. Run a `kind: performance` (or `load`) experiment against `checkout` with a
   load profile — a ramp or a flat run long enough for 20+ windows.
3. `tumult perf checkout --env staging` compares *N* against *N−1* and fails
   the job on `regressed`.

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
