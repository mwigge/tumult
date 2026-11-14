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

One principle orders everything below: **the verdict is never better than
the evidence**. A build that fails to answer is worse, not unknown; a
latency nobody waited for is at least the timeout, not missing; and
evidence too thin to tell is `inconclusive`, which fails the pipeline
gate by default.

## How a verdict is computed

1. **Runs and the version each measured.** A run is an experiment against
   the target (root spans with `resilience.target.system`) that started
   inside the lookback `range` (30 days by default). Its window is the
   experiment's root span: from the run's start to its end. The version a
   run measured is, in this order:

   1. **the version the run recorded** — `declared` when the experiment's
      `target.version` or `tumult run --target-version` named it,
      `observed` when the runner read it at the end of the run from the
      target's own telemetry inside the run's window in the run's
      environment and froze it in the journal (`target_version` with
      `version_source`). The lake keeps it per run. A version is
      `observed` only when it served at least 95 % of the target's
      requests inside the window; when two versions shared it (a rollout
      in the middle of the run) the journal records `version_source:
      mixed` with each version's share and the run has no version;
   2. **`inferred`**, for a run that recorded none (older journals, or a
      target that was silent when the run ended): the version that served
      at least 95 % of the target's requests (`service.name` is the
      target) inside the run's window; below that the run has no version
      (`mixed_versions`). With no request inside the window, the version
      of the target's last span before the run's start, when that span is
      at most one hour old;
   3. otherwise **none**. The run is not dropped quietly: it is counted in
      `unattributed_runs` with its reason
      (`no_target_telemetry_in_environment` when the target never reported
      a version in the run's environment, `no_target_spans_near_run` when
      it did but not near this run, `mixed_versions` when no version
      served 95 % of its window) and takes part in nothing.

   **Requests, not spans.** A version's share is counted in the requests
   the service received or began (server and root spans), so a build that
   emits ten spans a request weighs no more than one that emits two.

   **The window and clock skew.** The window is read two seconds wider at
   both ends, the tolerance for clocks that disagree between the runner
   and the service. The price: a build that first speaks within two
   seconds of the end of a run it did not serve counts into that run's
   window. A run that recorded its version is not affected. Mind that the
   root span ends when the runner has finished asking for the version,
   which can be up to five seconds after the load stopped.

   **Asking until it settles.** Telemetry arrives late. The runner (the
   CLI and the daemon alike) asks which version served the window, waits
   a second and asks again until the request count no longer grows, for
   at most five seconds; then it records what the last answer said.

   **`service.namespace`.** A target is identified by its service name
   and its environment, and by `service.namespace` when the run's target
   names one: two services with one name in two namespaces are then not
   read as one.

   **Bounds.** A version is at most 128 characters and carries no control
   characters. `--target-version`, `target.version` and the `baseline` /
   `candidate` parameters are refused beyond that (an error before the
   run starts; a 400); a version from a span is stored cut to the bound,
   flagged in the rollup (`sanitized`) and logged
   (`lake.span.version_sanitized`), never dropped.

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
   by the start of their latest deployment, latest first, then by the
   greater version number. A version a run recorded counts as seen from
   that run's start. The *candidate* is the version deployed latest and
   the *baseline* the one deployed just before it, unless `candidate` and
   `baseline` name them; `selection` in the answer says which
   (`latest_deployment` or `explicit`) and `selection_rule` says it in
   words.

   **The order does not depend on `range`.** The versions and their order
   come from everything the lake has ever seen of the target in that
   environment; `range` bounds the runs and samples and nothing else. A
   request for 30 days and one for 60 days compare the same pair.

   **Rollback and redeploy.** A version's latest deployment is its first
   sight, or, when it came back after another version had taken over, the
   start of its latest return: the rollup keeps sightings by the minute,
   and a version that reports again after at least ten minutes of silence
   during which a later version reported was deployed again. Such a
   version is `redeployed: true` in `versions_seen`, with its
   `first_seen_ns` and its `last_deployed_ns`, and becomes the candidate
   again. Silence alone, or two versions reporting side by side without a
   gap, is no redeploy. When the candidate came back and the version
   before it kept reporting beside it, a redeploy cannot be told from two
   builds running side by side: the answer is a 400 that asks for both
   `baseline` and `candidate`. Versions known only from what runs
   recorded (a target that exports no telemetry) have no sightings to
   find a redeploy in; name the versions there.

   **A return is not always a rollback.** One request that still reaches
   an old pod after the new build took over looks, in telemetry, like the
   old build coming back. A return is therefore not taken as the latest
   deployment while it is younger than ten minutes, nor while it rests on
   fewer than 10 requests, or on fewer than 5 % of what the other version
   served from ten minutes before it. Until then the daemon does not
   order the two: the answer is a 400 with `reason: unorderable` and a
   message naming both versions, and `tumult perf` reads it as
   `inconclusive` (exit 4). With `candidate` named the question does not
   arise: the candidate is compared against the version it replaced,
   whichever reported last. **A CI gate knows which build it tests and
   should always pass `--candidate`.**

   A span that arrives late moves a version's first sight back by at most
   ten minutes; a span stamped earlier than that (a clock gone wrong, a
   replay) is stored as a span and left out of the version history, with
   a `lake.sightings.early_span_ignored` warning in the daemon's log.

   **Version numbers.** Where two versions tie, the greater number wins,
   in one total order: versions that parse as numbers (`1.10.0`, `v2.1`,
   `1.4.0-rc.1+build.7`) sort among themselves by semver precedence — a
   pre-release below its release, build metadata ignored and then
   compared as text so the order stays total, numbers of any length
   compared digit by digit — and everything else (git hashes, dates,
   names) sorts after them as text.

   **Old lakes.** A lake written before the rollup existed has its version
   history read back from its spans once, a day of spans at a step. A
   small lake is done when the daemon has opened it; for a large one the
   daemon spends two seconds on it at startup and carries on in the
   background. Until it is done the answer says `backfill: running`, and
   a pair the request did not name in full is `inconclusive`
   (`history_incomplete`): the order is not final yet.

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
   and its latency samples are left out; `include=partial` keeps them.
   Halted and deviated runs are counted too and stay in. `run_details`
   lists every run of a side with how its version is known, its status,
   whether it was truncated or aborted and whether its samples are in.
4. **What became of the requests** is judged before any latency, because
   latency is measured on the requests that were answered and the
   survivors of a build that hangs are fast.

   **Unanswered windows.** A load window in which nothing answered leaves
   no latency sample. Each side reports its `windows` (`planned`,
   `completed`, `unanswered`, `lost` — skipped behind an unanswered one —
   `unanswered_share` and `censored`). The rule is asymmetric, with a
   limit of a tenth of a side's windows:

   | Baseline | Candidate | Verdict | `reason` |
   |---|---|---|---|
   | within the limit | over it | `regressed` | `candidate_unanswered` |
   | over it | within the limit | `inconclusive` | `baseline_unanswered` |
   | over it | over it | `inconclusive` | `both_unanswered` |

   A candidate that stopped answering is a regression however few samples
   it left; a baseline that did is no yardstick.

   **Failure share.** Each side reports its `requests`: `sent`, `failed`
   (sent and not answered well: timed out, refused, or answered outside
   2xx/3xx), the `failure_share` and `runs_without_counts`. The requests
   of every run of the side are counted, those of the runs left out as
   partial too — requests that hang are exactly what makes a plan fall
   short, and a request that failed did fail. A candidate whose failure
   share is above the baseline's by more than **one percentage point**
   (absolute; the margin) and by more than chance (a one-sided
   two-proportion test at z ≥ 1.645, the 5 % level) is `regressed` with
   reason `failure_share`, whatever its latencies. Equal shares, or a
   difference inside the margin, decide nothing. `comparison.failure_share`
   carries both shares, the delta and the margin. A run recorded before
   request counts existed, or by a tool whose summary has none, is
   counted in `runs_without_counts` and the answer adds
   `failure_data_missing`: its silence is not read as zero failures. k6
   contributes `http_reqs` and its `http_req_failed` rate; it keeps one
   end-of-run summary, so it has no per-window counts and no censoring.

   **Censored samples.** With oha a request given up on at its timeout
   counts into the window's percentiles at the timeout: a window whose
   p95 falls among the timed-out requests reports the timeout — the true
   value is that or more — and is marked censored. A verdict that would
   need a censored value to be no more than it reads (a candidate called
   improved or unchanged, a baseline the candidate is slower than or
   level with) is `inconclusive` with reason `censored`. A candidate that
   is slower even when its censored windows are read at the timeout is
   still `regressed`.
5. **Statistics** (`tumult-perf`), on what is left. Windows of one run
   are not independent — a slow minute is followed by a slow minute — so
   the unit of evidence is the run:

   - **Percentiles** (p50/p95/p99 of each side) weigh a window by the
     seconds of load it really sampled: a window cut short has less say
     than a whole one.
   - **The interval** on the candidate − baseline difference at the
     chosen quantile (p95 by default) is a percentile bootstrap that
     resamples **whole runs** with replacement and keeps each run's
     windows together (a cluster bootstrap), 2000 times. Autocorrelated
     windows therefore give a wider interval than treating every window
     as an independent draw would, and one slow run among several is not
     called a regression.
   - **Minimum evidence.** Each side needs at least `min_runs` runs with
     samples (3) and `min_samples` windows (20); below either the verdict
     is `inconclusive` (`insufficient_evidence`) and says which is short.
     One or two runs give a bootstrap over runs nothing to draw from.
   - **Reproducible.** The resampling uses a seeded generator
     (`comparison.seed`; `seed=` overrides it), so the same runs and the
     same seed give the same interval, bit for bit.
6. **Verdict** on the latencies against a noise margin (5 % of the
   baseline quantile by default):

   | Verdict | Rule |
   |---|---|
   | `regressed` | the whole interval lies above `+margin` |
   | `improved` | the whole interval lies below `−margin` |
   | `unchanged` | the interval is inside the margin, or includes zero and reaches past the margin on at most one side |
   | `inconclusive` | either side has fewer than `min_runs` runs or `min_samples` samples, only one version seen, an interval so wide it reaches past the margin in both directions, or an interval that excludes zero but straddles the margin (for example `[4, 12]` against a margin of 5: the shift is significant, its size against the margin is not resolved) |

   A tiny but statistically significant shift inside the margin is
   `unchanged` on purpose: the margin is where you say what you do not care
   about.

   `reason_codes` lists what the verdict rests on, the deciding one first
   (also as `reason`): `candidate_unanswered`, `baseline_unanswered`,
   `both_unanswered`, `failure_share`, `censored`, `latency`,
   `insufficient_evidence`, `no_baseline`, `history_incomplete`, and
   `failure_data_missing` as a note beside any of them. `reasons` says the same in sentences.

## The endpoints

```
GET  /api/perf/{target}?probe=&quantile=&confidence=&min_samples=&min_runs=&seed=&margin_pct=&env=&baseline=&candidate=&range=&include=
POST /api/perf/{target}/verdicts   (same query string)
GET  /api/perf?range=
GET  /api/perf/{target}/version?env=&namespace=&from_ns=&to_ns=
```

| Parameter | Default | Meaning |
|---|---|---|
| `probe` | `load.latency_p95_ms` | The `probe_samples` series compared |
| `quantile` | `p95` | `p50`, `p95` or `p99` |
| `confidence` | `0.95` | Bootstrap interval level, in (0, 1) |
| `min_samples` | `20` | Fewer windows on either side is inconclusive |
| `min_runs` | `3` | Fewer runs with samples on either side is inconclusive (at least 1) |
| `seed` | fixed | Seed of the bootstrap generator; the answer carries the one used |
| `margin_pct` | `5` | Noise margin as a percentage of the baseline quantile |
| `env` | the only environment that can answer | The environment the answer is for (403 outside a scoped token's environments, 400 when empty) |
| `candidate` | the version deployed latest | The version judged; alone, its baseline is the version it replaced. At most 128 characters, no control characters |
| `baseline` | the version deployed before the candidate | The version compared against; alone, its candidate is the latest other version. Same bounds |
| `range` | `30d` | Lookback for runs and samples: a number followed by `s`, `m`, `h`, `d` or `w`. The versions and their order do not depend on it |
| `include` | — | `partial` keeps the latency samples of aborted and truncated runs |

The answer carries the `environment` it is for (and `environments_seen`),
the `range` and `since_ns`, `selection` (`explicit` or
`latest_deployment`) with its `selection_rule`, `backfill` (`done` or
`running`), both sides, the `comparison` (`delta`, `delta_pct`, `ci_low`,
`ci_high`, `margin`, `iterations`, `seed`, `min_samples`, `min_runs` and
`failure_share` with `baseline`, `candidate`, `delta` and `margin`, null
when a side counted no requests), `verdict`, `reason`, `reason_codes`,
`reasons`, the environment's `versions_seen` in deployment order
(`first_seen_ns`, `last_seen_ns`, `last_deployed_ns`, `redeployed`),
`unattributed_runs` (`count`, `reasons`, `runs`), `stored`, `duplicate`,
and `history` — the last ten verdicts stored for the target in that
environment. Each side has `version`, `runs`, `samples`, `dropped`,
`p50`/`p95`/`p99`, `attribution` (how many of its runs were `declared`,
`observed`, `inferred`), `run_details` (per run: the attribution, the
status, whether it was truncated or aborted, whether its samples are
included), `partial` (`truncated`, `aborted`, `halted`, `deviated`,
`excluded`), `windows`, `requests` and the `distribution` of its samples.

An unknown target (no `service.version` seen in the environments asked
for) is a 404 that names those environments; a target with one version is
a 200 `inconclusive`; a named version the environment has not seen inside
the range is a 400 that lists the ones it has; a target with versions in
several environments and nothing to single one out is a 400 that lists
them.

`GET /api/perf` lists the targets experiments ran against inside the
range, with the environments of those runs, within the caller's scopes.
`GET /api/perf/{target}/version` answers what served a window: the
requests each version of the target served between two instants in
exactly one environment (`versions`, with `requests` and `share`), and
the `version` that served at least 95 % of them (`version_source:
observed`); `null` with `version_source: mixed` when none did, and both
`null` when the target reported nothing there then. `namespace` narrows
it to one `service.namespace` (at most 128 bytes). The window is at most
24 hours wide; a wider one is a 400. It is what a runner asks at the end
of a run, until the count has settled.

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
  versions: 1.5.0 is the version deployed latest in environment staging
  baseline  1.4.0: 100.0 ms over 72 sample(s) in 3 run(s)
  candidate 1.5.0: 150.0 ms over 72 sample(s) in 3 run(s)
      1 run(s) attributed from spans; 1 partial run(s) left out
  delta +50.0 ms (+50.0%), CI [31.0, 70.0], margin ±5.0
  - p95 difference interval [31.00, 70.00] against a noise margin of ±5.00 lies entirely above the margin
  verdict stored in perf_verdicts
```

`--baseline <version>` and `--candidate <version>` name the versions
instead of taking the two deployed latest; the `versions:` line is the
daemon's `selection_rule` and says which of the two it was. `--range 12h` shortens or lengthens the lookback for
runs and samples, and `--include partial` keeps the latency samples of
aborted and truncated runs. A side that lost load windows to silence adds
`N of M load window(s) went unanswered`, one with failed requests or
censored windows says so, and runs without a version add `N run(s) could
not be attributed to a version`.

**Exit codes.** By default the command fails on a regression *and* on a
verdict that could not be reached (`--fail-on regressed,inconclusive`): a
gate that passes because nothing could be measured is not a gate.

| Exit | Meaning |
|---|---|
| `0` | `improved` or `unchanged`, or a verdict `--fail-on` lets pass |
| `1` | the command failed: daemon unreachable, an HTTP error, `--store` that did not store, a verdict this build does not know |
| `2` | usage error (an unknown flag or value) |
| `3` | `regressed` |
| `4` | `inconclusive`, or the daemon could not tell which version was deployed latest (`reason: unorderable`) |

`--fail-on regressed` lets `inconclusive` pass (the behaviour before this
default changed); `--fail-on never` only prints. `--json` prints the
daemon's answer.
Without `--store` the command only reads (`GET`); with it the command
`POST`s to `/api/perf/{target}/verdicts` and the daemon appends the verdict
to `perf_verdicts`, which is what the pipeline step should do. `--store`
exits non-zero whenever the daemon did not store — a viewer token (403), a
single version (409), or an answer without `stored: true` — whatever
`--fail-on` says. It slots into a pipeline next to `tumult gate` (see
`deploy/ci/`).

### `tumult run`, `--no-ingest` and the observed version

A run that does not declare its version asks for it when it ends. Whom it
asks:

| Setup | Who is asked |
|---|---|
| `TUMULT_DAEMON_URL` set, with or without `--no-ingest` | the daemon: it holds the target's telemetry whether or not this run's journal is sent to it |
| no daemon URL, run ingested into the local store | the local store |
| no daemon URL and `--no-ingest`, or no local store | nobody |

In the last case the run prints a notice on stderr and records no
version:

```
notice: the version of checkout this run measures is not declared and nobody is asked for it (no TUMULT_DAEMON_URL, and no local store the run reports to); declare it with --target-version or the run is attributed from the target's spans later, if at all
```

Declaring the version (`--target-version`, or `target.version` in the
definition) is the reliable path in every setup.

### Backups

The lake's Parquet export (`export_tables` and `export_load_tables` of
`tumult-lake`) writes the tables a verdict is computed from —
`experiments` with `target_version` and `version_source`, `load_results`
with how the load was executed and what became of its requests, and
`probe_samples` with each window's counts and censoring — and `tumult
import <dir>` reads `load_results.parquet` and `probe_samples.parquet`
back when the directory holds them (both or neither). The same runs give
the same verdict after an export and an import. No CLI command writes
such a directory yet; the export is a library call. Spans are not part of
it: the deployment order of an imported lake comes from the versions the
runs recorded, so name the versions when the runs recorded none.

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
3. Repeat step 2 until the build has at least three runs; one run, or
   two, is `inconclusive` whatever it measured.
4. `tumult perf checkout --env staging --candidate N --store` compares
   *N* against the build it replaced, records the verdict and fails the
   job on `regressed` (exit 3) and on `inconclusive` (exit 4). Name the
   candidate: the pipeline knows which build it tests, and the deployment
   order the daemon reads from telemetry can be upset by a request that
   still reaches the old build.

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
windows are reported per side, too many of them on the candidate alone is
a regression, and on the baseline the comparison is inconclusive.

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
