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

1. **Runs and the version each measured.** A run is an experiment against
   the target (root spans with `resilience.target.system`) that started
   inside the lookback `range` (30 days by default). Its window is the
   experiment's root span: from the run's start to its end. The version a
   run measured is, in this order:

   1. **the version the run recorded** — `declared` when the experiment's
      `target.version` or `tumult run --target-version` named it,
      `observed` when the runner read it at the end of the run from the
      target's own spans inside the run's window in the run's environment
      and froze it in the journal (`target_version` with
      `version_source`). The lake keeps it per run;
   2. **`inferred`**, for a run that recorded none (older journals, or a
      target that was silent when the run ended): the version carried by
      most of the target's spans (`service.name` is the target) inside the
      run's window; on a tie the version first seen latest in that
      environment, then the greater version number (`1.10.0` is above
      `1.9.0`; plain string order only when a version is not a number).
      With no span inside the window, the version of the target's last
      span before the run's start, when that span is at most one hour old;
   3. otherwise **none**. The run is not dropped quietly: it is counted in
      `unattributed_runs` with its reason
      (`no_target_telemetry_in_environment` when the target never reported
      a version in the run's environment, `no_target_spans_near_run` when
      it did but not near this run) and takes part in nothing.

   Only spans of exactly the run's environment are read for a run. A run
   that names no environment is read from the spans that name none, never
   from another environment's. With 1.0.0 busy in prod and 1.1.0 soaked in
   demo, the demo run belongs to 1.1.0 however the two streams interleave.
   Recording the version is the reliable path: declare it in the pipeline
   that knows what it deployed, and inference never has to guess. A run
   against a canary taking a minority of one environment's traffic is
   otherwise observed as the majority version; declare the version, or
   give the canary its own environment name.
2. **One environment, two versions.** An answer is for one environment:
   the one `env` names, else the only one in which the target has versions,
   else the only one that can answer (it has two versions, or every
   version the request names). When several could, the answer is a 400
   that asks for `env` or for explicit versions; one environment's
   baseline is never compared with another's candidate.

   Inside that environment the versions are put in **deployment order**:
   by first sight, latest first, then by the greater version number. A
   version a run recorded counts as seen from that run's start. The
   *candidate* is the version deployed last and the *baseline* the one
   deployed just before it, unless `candidate` and `baseline` name them;
   `selection` in the answer says which (`deployment_order` or
   `explicit`). The order does not depend on which version reported last:
   an old build that keeps emitting spans, or resumes after a rollback,
   changes nothing, and two identical requests with no new run in between
   return the same verdict. To judge a rollback, name the versions.

   **What a read costs.** Versions come from a rollup the writer keeps
   while it inserts spans (`service_version_sightings`: one row per
   service, version and environment with its first and last sighting), so
   listing them never scans the spans. Runs are the experiment root spans
   inside the lookback. The target's own spans are read only for the runs
   that recorded no version, only inside the lookback plus the one-hour
   staleness limit, and aggregated in SQL to at most runs × versions rows.
   The cost is therefore bounded by `range` and by the number of runs in
   it, but it is not constant: a read that has to infer many old runs
   against a busy target scans that target's spans for the lookback.
   Shorten `range`, or record versions on the runs, to keep it small.
3. **Samples.** For each side, the `during`-phase `probe_samples` rows of its
   runs for one series — by default `load.latency_p95_ms`, the per-window p95
   a streaming load adapter writes (see the
   [Load Testing Guide](load-testing-guide.md)). Only the oha adapter
   streams windows today, so the default series exists for `tool: oha` runs
   only; a k6 run keeps one end-of-run summary and needs `probe` pointed at a
   series it does have. Any series works: a
   hypothesis probe's duration (`probe_name` = the probe's name) or another
   `load.*` metric.

   **Partial runs.** A run whose load was aborted or truncated (windows
   skipped because earlier ones overran) is counted per side in `partial`
   and its samples are left out; `include=partial` keeps them. Halted and
   deviated runs are counted too and stay in. A load window in which
   nothing answered leaves no latency sample, so it could flatter a build
   that stopped answering: each side reports its `windows` (`planned`,
   `completed`, `unanswered`, `unanswered_share`), and a side that lost
   more than a tenth of its windows that way is not comparable — the
   verdict is `inconclusive` with that reason first, never `unchanged`.
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
GET  /api/perf/{target}?probe=&quantile=&confidence=&min_samples=&margin_pct=&env=&baseline=&candidate=&range=&include=
POST /api/perf/{target}/verdicts   (same query string)
GET  /api/perf?range=
GET  /api/perf/{target}/version?env=&from_ns=&to_ns=
```

| Parameter | Default | Meaning |
|---|---|---|
| `probe` | `load.latency_p95_ms` | The `probe_samples` series compared |
| `quantile` | `p95` | `p50`, `p95` or `p99` |
| `confidence` | `0.95` | Bootstrap interval level, in (0, 1) |
| `min_samples` | `20` | Fewer on either side is inconclusive |
| `margin_pct` | `5` | Noise margin as a percentage of the baseline quantile |
| `env` | the only environment that can answer | The environment the answer is for (403 outside a scoped token's environments, 400 when empty) |
| `candidate` | the version deployed last | The version judged; alone, its baseline is the version deployed just before it |
| `baseline` | the version deployed before the candidate | The version compared against; alone, its candidate is the latest other version |
| `range` | `30d` | Lookback for runs and versions: a number followed by `s`, `m`, `h`, `d` or `w` |
| `include` | — | `partial` keeps the samples of aborted and truncated runs |

The answer carries the `environment` it is for (and `environments_seen`),
the `range` and `since_ns`, `selection` (`explicit` or
`deployment_order`), both sides, the `comparison` (`delta`, `delta_pct`,
`ci_low`, `ci_high`, `margin`, `iterations`, `seed`), `verdict`,
`reasons`, the environment's `versions_seen` in deployment order with
their first and last timestamps, `unattributed_runs` (`count`, `reasons`,
`runs`), `stored`, `duplicate`, and `history` — the last ten verdicts
stored for the target in that environment. Each side has `version`,
`runs`, `samples`, `p50`/`p95`/`p99`, `attribution` (how many of its runs
were `declared`, `observed`, `inferred`), `run_details` (per run: the
attribution, the status, whether it was truncated or aborted, whether its
samples are included), `partial` (`truncated`, `aborted`, `halted`,
`deviated`, `excluded`), `windows` and the `distribution` of its samples.

An unknown target (no `service.version` seen in the environments asked
for) is a 404 that names those environments; a target with one version is
a 200 `inconclusive`; a named version the environment has not seen inside
the range is a 400 that lists the ones it has; a target with versions in
several environments and nothing to single one out is a 400 that lists
them.

`GET /api/perf` lists the targets experiments ran against inside the
range, with the environments of those runs, within the caller's scopes.
`GET /api/perf/{target}/version` answers which version most of the
target's spans carried between two instants in exactly one environment
(`null` when it reported none); it is what `tumult run` asks once at the
end of a run to record an `observed` version.

`GET` is a pure read (viewer role): dashboards can ask as often as they
like without growing the lake. A `store` parameter, with any value, is refused with 400 on both routes:
on `GET` with the name of the `POST` route rather than an answer that
looks as if it had stored, on `POST` because the route always stores.
`POST …/verdicts` (operator role, in the route table like every other
mutation) computes the same answer and appends it to the lake's
`perf_verdicts` table (schema v18): target, probe, both versions, both
sides' percentiles, the interval, the margin, the verdict and its reasons,
when it was computed, and the environment it was computed for (`envs`,
null for telemetry that names no environment). It answers 201 with
`stored: true`, 409 when there is only one version and so nothing to store,
and 503 when the daemon has no writable lake. Rows are never updated, so a
recomputation after more runs adds a newer row and the older one stays.

Storing is idempotent. When the latest stored row for the same target,
probe, pair of versions, quantile and environments already holds the same
evidence (sample counts, percentiles, interval, margin and verdict), the
`POST` appends nothing and answers 200 with `stored: true` and
`duplicate: true`. A pipeline step retried after a client timeout
therefore leaves one row, and `tumult perf --store` treats both answers as
stored.

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

A service whose telemetry carries none of them forms an environment of its
own (`environment: null` in the answer). An unscoped request without `env`
is answered for it when it is the only one that can answer; `?env=staging`
or a scoped token gets a 404 saying that no `service.version` was seen for
the target *in environment staging* and listing the attributes above. That
is deliberately fail-closed — untagged telemetry is never assumed to
belong to the environment someone asked for, and it cannot be selected by
name.

## The CLI

```bash
export TUMULT_DAEMON_URL=http://tumultd.internal:4318
export TUMULT_DAEMON_TOKEN=kro_...   # viewer reads; --store needs operator
tumult perf checkout --quantile p95 --margin-pct 5 --env staging --store
```

```
perf checkout: REGRESSED (p95 of load.latency_p95_ms)
  versions: the two deployed last, in environment staging
  baseline  1.4.0: 100.0 ms over 40 sample(s) in 2 run(s)
  candidate 1.5.0: 150.0 ms over 40 sample(s) in 2 run(s)
      1 run(s) attributed from spans; 1 partial run(s) left out
  delta +50.0 ms (+50.0%), CI [31.0, 70.0], margin ±5.0
  - p95 difference interval [31.00, 70.00] against a noise margin of ±5.00 lies entirely above the margin
  verdict stored in perf_verdicts
```

`--baseline <version>` and `--candidate <version>` name the versions
instead of taking the two deployed last (the `versions:` line then reads
`named on the request`). A side that lost load windows to silence adds
`N of M load window(s) went unanswered`, and runs without a version add
`N run(s) could not be attributed to a version`.

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

1. Deploy build *N* of `checkout` to staging.
2. Run a `kind: performance` (or `load`) experiment against `checkout` with
   `tool: oha` and a soak profile long enough for 20+ windows, and say
   which build it measures: `tumult run soak.toon --target-version N` (or
   `target.version` in the definition). Without it the runner records the
   version the target's spans carried during the run, which needs the
   service to export traces with `service.version = N` to the daemon. The
   default series (`load.latency_p95_ms`) exists only for oha runs; with
   k6, pass `--probe` a series the run does write, such as a hypothesis
   probe's name.
3. `tumult perf checkout --env staging --store` compares *N* against the
   build deployed before it, records the verdict and fails the job on
   `regressed`.

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
`regressed`. The time a window spends waiting is taken from the next
window, so the plan stays on its clock; a tail so slow that windows have to
be skipped truncates the plan and the run ends `deviated`. Requests that
never return are counted as timeout errors; a window in which nothing was
answered has no latency sample at all, is counted in the journal
(`execution.unanswered_windows`), and fails the run only through a declared
threshold such as `error_rate` (see the
[Load Testing Guide](load-testing-guide.md)). A window skipped because the
one before it went unanswered is counted with the unanswered ones
(`execution.lost_to_unanswered`), not as truncation, so a blackout alone
does not make the run deviate. The verdict still sees it: unanswered
windows are reported per side and too many of them make the comparison
inconclusive.

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
