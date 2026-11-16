---
title: Experiment Format
parent: Guides
nav_order: 1
---

# Experiment Format

Tumult experiments are defined in TOON (Token-Oriented Object Notation) — the only supported format. TOON is human-readable, token-efficient, and serde-compatible.

## Structure

Every experiment has these sections:

| Section | Required | Description |
|---------|----------|-------------|
| `version` | No | Schema version (defaults to `v1` when omitted) |
| `title` | Yes | Human-readable experiment name |
| `description` | No | What this experiment validates |
| `tags` | No | Classification tags for filtering and analytics |
| `target` | No | System under test: `system` (required inside the block), `technology`, `environment`, `criticality`, `version` (the build this run measures; `tumult run --target-version` overrides it, and without either the runner records the version the target's own spans carried during the run). Emitted as `resilience.target.*` on the root span; the scorecard, the gate and environment scopes key on it, and performance verdicts compare by the recorded version |
| `kind` | No | What the run is: `chaos` (default), `performance` (load and performance; `load` is accepted as another name), `robustness`, `dr`, `security`. Coverage and scores count four disciplines: chaos, load / performance, robustness and security. A `dr` run is part of robustness. Non-functional objectives are not a discipline: the `nfr` block below is verified by a run of any kind. Sets the `resilience.test.kind` span attribute |
| `nfr` | No | Non-functional objectives verified against the run's measurements (see [Objectives](#objectives)) |
| `rto_target_s` | No | Recovery time objective in seconds; required when `kind: dr`. Measured against post-phase MTTR |
| `rpo_target_s` | No | Recovery point objective in seconds of tolerated data loss; measured only by an `nfr` entry of `kind: rpo` |
| `configuration` | No | Non-sensitive key-value pairs (inline or from environment variables) — usable in templates and injected into subprocess environments |
| `secrets` | No | Sensitive values (from environment variables or files) — env-injected, never journaled |
| `controls` | No | Lifecycle hooks executed at every lifecycle event (before/after experiment, method, activity, hypothesis, rollback) |
| `steady_state_hypothesis` | No | Probes that define "healthy" — checked before and after fault |
| `guards` | No | Auto-halt guardrails — probes evaluated continuously during the fault window that stop the experiment when their tolerance is breached |
| `blast_radius` | No | Free-form note describing the intended blast radius (journaled for audit; documentation only, not enforced) |
| `max_concurrent_faults` | No | Cap on concurrently-active background faults during the method (unlimited when omitted) |
| `method` | Yes | Ordered sequence of actions and probes to execute |
| `rollbacks` | No | Actions to restore system state after the experiment |
| `estimate` | No | Phase 0 — prediction of expected outcome |
| `baseline` | No | Phase 1 — statistical baseline acquisition config |
| `load` | No | Load tool integration (k6 or oha) with an optional traffic profile |
| `regulatory` | No | Regulatory framework mapping (DORA, NIS2, PCI-DSS) |

## Activity Types

Every step in `method`, `rollbacks`, and `steady_state_hypothesis.probes` is an **Activity**:

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Unique name for this step |
| `activity_type` | `action` or `probe` | Actions change state, probes observe |
| `provider` | Provider | How the activity executes (see below) |
| `tolerance` | Tolerance | Expected result (for probes in hypothesis) |
| `pause_before_s` | float | Wait before executing (seconds) |
| `pause_after_s` | float | Wait after executing (seconds) |
| `background` | bool | Run concurrently with next step |

## Provider Types

| Type | Description | Key Fields |
|------|-------------|------------|
| `native` | Call a compiled Rust plugin | `plugin`, `function`, `arguments` |
| `script` | Dispatch a discovered script plugin action | `plugin`, `function`, `arguments`, `timeout_s` |
| `process` | Run a script or binary | `path`, `arguments`, `env`, `timeout_s` |

The `native` provider dispatches through a registry of compiled-in executors (`tumult-ssh`, `tumult-net`, `tumult-kubernetes`). Referencing an unknown plugin or function fails with an error listing the available names. To probe an HTTP endpoint, use a `process` provider with `curl` — the former experimental `http` provider type has been removed and now fails validation.

### Script Provider

The `script` provider dispatches an action (or probe) declared by a **script plugin** — a directory holding a `plugin.toon` manifest and shell scripts:

```toon
provider:
  type: script
  plugin: tumult-network
  function: redirect-dns
  arguments:
    dns_domain: "test-redirect.tumult.local"
    dns_redirect: "127.0.0.1"
  timeout_s: 10.0
```

- `plugin` names the plugin manifest; `function` names an entry in its `actions` (falling back to `probes`). Both must be non-empty and must not contain path separators, `..`, null bytes, or whitespace (rejected at validation time).
- Plugins are resolved at run time through the discovery search paths, in order: `./plugins/` (local to the experiment), `~/.tumult/plugins/` (user-global), then every colon-separated entry in `TUMULT_PLUGIN_PATH`. First match wins; a shadowed copy of the same plugin name is reported as a discovery warning. `tumult discover` prints the authoritative list.
- **Arguments contract:** each `arguments` entry reaches the script as an environment variable named `TUMULT_<KEY>` where `<KEY>` is the argument name uppercased (`dns_domain` → `TUMULT_DNS_DOMAIN`). Keys must form valid shell identifiers after uppercasing, and two keys that uppercase to the same name are rejected. Strings pass through unquoted; numbers, booleans, and composite values use their JSON representation.
- Unknown plugins or actions fail at dispatch time with an error listing the available names.
- Script plugins run through `/bin/sh` and are unsupported on Windows.
- **Rollbacks remain experiment-declared:** a script action never auto-undoes itself. If the fault needs cleanup, declare a matching rollback step in `rollbacks:` (another script action, or a small `process` step) — see Rollback Strategy below.

## Template Variables

Every string field in the experiment may contain `${name}` placeholders. Substitution runs when any placeholder source exists — a `--var KEY=VALUE` flag, a `configuration:` section, or a `secrets:` section — and resolves against, in precedence order:

1. `--var` entries (`tumult run --var env=staging …`) — an exact-name match always wins, even over the namespaces below,
2. `${config.<name>}` — resolved `configuration:` values,
3. `${secrets.<group>.<key>}` — resolved `secrets:` values.

Substitution is all-or-nothing and strict: if any `${name}` cannot be resolved, the run fails before executing anything and the error lists **every** missing variable. Values are escaped for the document context, so a value cannot inject structure into the experiment.

**Escape hatch:** `$${name}` renders as a literal `${name}` with no substitution and no missing-variable error. Use it for shell-style text that must reach the subprocess untouched — e.g. `sh -c 'echo $${HOME}'`, or shell parameter syntax like `$${#VAR}` — otherwise every `${...}` in the file becomes a required variable once substitution is active.

## Configuration and Secrets

`configuration:` holds **non-sensitive** settings (endpoints, sizes, thresholds) as defaults that each environment can override; `secrets:` holds **sensitive** values (tokens, passwords). Each configuration entry is `type: env` (read from an environment variable) or `type: inline` (a literal value); each secret entry is `type: env` or `type: file` (read verbatim from a file). Resolution happens at the start of the run; a missing env var or file fails the run before anything executes.

```toon
configuration:
  db_host:
    type: env
    key: CHAOS_DB_HOST
  retries:
    type: inline
    value: "3"
secrets:
  db:
    password:
      type: env
      key: CHAOS_DB_PASSWORD
```

Both sections feed two consumption paths:

- **Templates** — `${config.retries}`, `${secrets.db.password}` (see Template Variables above).
- **Environment injection** — `process` and `script` provider subprocesses receive `TUMULT_CONFIG_<NAME>` and `TUMULT_SECRET_<GROUP>_<KEY>` (uppercased; the secret's `group.key` becomes `GROUP_KEY`). Names that do not form valid shell identifiers after uppercasing (the same rule script-plugin arguments enforce) are skipped with a warning naming the key — they remain usable in templates. Entries declared directly on the activity (`env:` for process, `arguments:` for script) win over injected ones. `native` providers receive no injection.

**Secrets are never journaled.** Journals record each activity's name, type, status, timing, captured stdout/stderr, and trace ids — never provider arguments or environments — and resolved values are never printed by the CLI. A secret therefore stays out of journals, logs, and analytics as long as the subprocess itself does not echo it and you do not template it into a *recorded* field: prefer env injection for secrets, and keep `${secrets.*}` substitutions inside provider arguments (a secret substituted into an activity **name** would be journaled, and into a process `path` or plugin name would reach traces — don't).

## Controls

Each entry in `controls:` declares a lifecycle hook with a `name` and a `provider`. Declared controls **execute**: the control's provider is invoked once at every lifecycle event — before/after the experiment, the hypothesis evaluations, the method, each activity, and the rollbacks — with the event identity injected so the hook can decide whether to act:

- `process` providers receive `TUMULT_CONTROL_EVENT` (e.g. `before_experiment`, `after_activity`) as an environment variable, plus `TUMULT_CONTROL_ACTIVITY` for the per-activity events,
- `script` and `native` providers receive the same values as `control_event` / `control_activity` arguments (script providers export them as the same `TUMULT_*` env vars).

Entries the declared provider already sets win over the injected ones. Controls are observability/safeguard hooks, not gates: a failing (or panicking) control is logged and the run continues. Controls execute on every event by design — the schema has no event scoping, so a hook that only cares about some events should check `TUMULT_CONTROL_EVENT` and exit early.

## Rollback Strategy

Steps in `rollbacks:` run according to the run's rollback strategy (`tumult run --rollback-strategy`):

| Strategy | When rollbacks execute |
|----------|------------------------|
| `always` | After every run, regardless of outcome |
| `on-deviation` (default) | When the experiment deviates (hypothesis unmet after the fault), **or** when it fails or is interrupted after a fault-injecting activity started — an injected fault needs cleanup even when nothing "deviated" |
| `never` | Never |

## Tolerance Types

Used in steady-state hypothesis probes to define expected values:

| Type | Description | Fields |
|------|-------------|--------|
| `exact` | Exact value match | `value` (any JSON value) |
| `range` | Numeric range | `from`, `to` |
| `regex` | Pattern match on string output | `pattern` |

## Estimate (Phase 0)

Predictions made before any measurement. Compared against actual results in Phase 4.

| Field | Values | Description |
|-------|--------|-------------|
| `expected_outcome` | `recovered`, `deviated`, `unaffected` | What you expect |
| `expected_recovery_s` | float | Predicted recovery time |
| `expected_degradation` | `none`, `minor`, `moderate`, `severe` | Expected impact level |
| `expected_data_loss` | bool | Whether data loss is expected |
| `confidence` | `low`, `medium`, `high` | Confidence in prediction |
| `rationale` | string | Why this prediction |
| `prior_runs` | int | How many times this has been run before |

## Baseline Config (Phase 1)

Configuration for statistical baseline acquisition:

| Field | Description |
|-------|-------------|
| `duration_s` | How long to sample (seconds) |
| `warmup_s` | Settling time to discard |
| `interval_s` | Sample frequency |
| `method` | `static`, `percentile`, `mean_stddev`, `iqr`, `learned` |
| `sigma` | Standard deviations for mean_stddev method |
| `confidence` | Confidence level (0.0-1.0) |

## Objectives

`nfr` declares the non-functional objectives a run is verified against. Every objective gets a verdict in the journal (`objectives.verdicts`) and in the lake (`nfr_verdicts`): `met`, `breached`, or `inconclusive` when the run did not produce the metric. A breached objective with `criticality: blocking` turns a completed run into a deviation.

| Field | Values | Description |
|-------|--------|-------------|
| `id` | string | Unique within the experiment |
| `kind` | `latency`, `availability`, `throughput`, `error_rate`, `rto`, `rpo`, `capacity`, `recovery`, `data_integrity` | What the objective constrains |
| `metric` | string | Measurement to evaluate (table below) |
| `operator` | `lte`, `gte`, `eq` | Comparison against `threshold` |
| `threshold` | float | |
| `window_s` | float | Evaluation window (documentation; the runner evaluates over the phase that produced the metric) |
| `criticality` | `blocking`, `major` (default), `minor` | How much a breach matters |
| `description` | string | |

Metrics the runner produces:

| Metric | Source |
|--------|--------|
| `load.throughput_rps`, `load.latency_p50_ms`, `load.latency_p95_ms`, `load.latency_p99_ms`, `load.error_rate`, `load.total_requests` | the load result |
| `recovery.mttr_s`, `recovery.time_s` | the post phase |
| `during.error_rate`, `during.availability`, `post.error_rate`, `post.availability` | raw probe samples of that phase |
| `probe.<name>.mean_ms`, `probe.<name>.max_ms`, `probe.<name>.p95_ms`, `probe.<name>.error_rate` | raw during-phase samples of one probe |
| `fault.duration_s` | the during phase |

A metric that is not in this table is an error: no run can measure it. A `probe.<name>` metric must name a probe of the steady-state hypothesis, because those are the probes a run samples.

`rto_target_s` adds an implicit blocking `rto` objective on `recovery.mttr_s`. `rpo_target_s` is recorded with the run; without an `nfr` entry of `kind: rpo` naming a probe-derived metric it is reported `inconclusive`, never silently met.

```yaml
kind: dr
rto_target_s: 900
rpo_target_s: 60
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

Raw probe samples from the during and post phases are kept in the journal (`probe_samples`) and in the lake table `probe_samples` (`experiment_id`, `probe_name`, `phase`, `ts_ns`, `value` in ms, `ok`), so later analysis works on the time series rather than only on the per-phase aggregates.

## Load

The optional `load` section drives load generation concurrent with the method. Two adapters share one boundary (`runner::LoadExecutor` plus the tool-agnostic plan in `runner::load_profile`): **k6** (`tool: k6`, `script` is the k6 script) and **oha** (`tool: oha`, `script` is the URL to hit). The `--load` CLI override accepts `k6`, `oha` or `none`. `tool: jmeter` still parses for older files but has no adapter and fails at start rather than silently running k6.

| Field | Required | Description |
|---|---|---|
| `tool` | Yes | `k6` or `oha` |
| `script` | Yes | k6 script path, or the URL for oha |
| `vus` | No | Virtual users / concurrency (default 10) |
| `duration_s` | No | Seconds of load (default 30); required when a `profile` is set |
| `thresholds` | No | Metric → bound. k6 scripts enforce their own; for oha the adapter checks `latency_p50_ms`, `latency_p95_ms`, `latency_p99_ms`, `error_rate` (upper bounds) and `throughput_rps` (lower bound) against the aggregate, and an unknown name counts as unmet |
| `profile` | No | Traffic shape, see below |

### Load profiles

A profile shapes the traffic over `duration_s`; without one the load is flat at `vus`. Every profile is turned into the same plan of stages, which k6 renders as `--stage` arguments and oha as one process per window, so a profile means the same thing whichever tool runs it. The validator rejects a shape that does not fit the duration.

| `type` | Fields | Shape |
|---|---|---|
| `ramp` | `start_vus`, `ramp_up_s` | Linear ramp from `start_vus` to `vus` over `ramp_up_s`, then hold `vus` |
| `spike` | `peak_vus`, `after_s`, `hold_s` | `vus` for `after_s`, a jump to `peak_vus` held for `hold_s`, back to `vus` for the rest |
| `soak` | `sample_interval_s` | Constant `vus` for the whole (long) duration, with a summary sampled every `sample_interval_s` |

```toon
load:
  tool: oha
  script: http://127.0.0.1:8080/checkout/1
  vus: 20
  duration_s: 7200
  profile:
    type: soak
    sample_interval_s: 30
```

The oha adapter runs the plan window by window (`sample_interval_s` wide for a soak and at least one second, one window per stage otherwise) and records one sample per window in `load_result.samples` (`ts_ns`, `duration_s`, `vus`, `throughput_rps`, `latency_p50_ms`, `latency_p95_ms`, `latency_p99_ms`, `error_rate`, `requests`). The lake stores each field as a `probe_samples` series named `load.<field>` in the `during` phase, next to the probe samples of the same run, so a two-hour soak is a time series you can chart and compare, not one number. The k6 adapter keeps its single end-of-run summary (its per-request stream is too heavy to retain) but honours every profile through stages. Examples: `examples/checkout-ramp-k6.toon`, `examples/checkout-spike-oha.toon`, `examples/checkout-soak-oha.toon`.

### Robustness runs

`kind: robustness` covers the runs where the question is "does it fail safely" rather than "does it recover from a fault": malformed input, saturation and soak. They use the same sections as every other experiment — the method carries the hostile inputs as probes with tolerances, the load section drives the saturation or soak, and `nfr` entries say what must hold. `examples/api-malformed-input.toon` posts oversized, truncated, wrongly typed and deeply nested bodies and expects 4xx while the health probe stays green; `examples/api-saturation.toon` ramps past capacity under cpu stress and expects shed load (429/503) rather than a dead process; `examples/checkout-soak-oha.toon` holds load for two hours and streams samples.

## Analysis (Phase 4)

Computed onto the journal after the run, comparing predictions (Phase 0) and measurements:

| Field | Description |
|-------|-------------|
| `estimate_accuracy` | 0.0–1.0 — how close the outcome was to the estimate |
| `resilience_score` | 0.0–1.0 composite score |
| `estimate_recovery_delta_s` | *Reserved — unpopulated in 2.20* |
| `trend` | *Reserved — unpopulated in 2.20* |

The degradation fields on during/post results (`degradation_onset_s`, `degradation_peak_s`, `degradation_magnitude`, `graceful_degradation`, `residual_degradation`) are likewise reserved and unpopulated in 2.20.

## Journal Export

`tumult export <journal> --format <fmt>` writes the journal's experiment record in four formats: `parquet` (Apache Parquet), `csv`, `json` (pretty-printed journal), and `arrow` (Apache Arrow IPC file format, suitable for zero-copy analytics interchange).

## Where a step runs

A step runs on the machine that runs Tumult. To reach another place, pick
the provider for it:

| Place | How |
|-------|-----|
| This machine | `process`, or a `script` plugin |
| A remote host | `native` plugin `tumult-ssh`, function `execute` ([tumult-ssh](../plugins/tumult-ssh.md)) |
| A container | `process` with `docker exec`, or the `tumult-containers` plugin |
| A Kubernetes pod | `native` plugin `tumult-kubernetes` ([tumult-kubernetes](../plugins/tumult-kubernetes.md)) |

An example that changes the machine it runs on has the tag `host-changing`, and
its description starts with what it changes.

A step has no `execution_target:` key. A key that Tumult does not know,
on a step, a provider or a tolerance, is an error when the file is
loaded. `tumult validate` shows it.

A key written twice in one object is an error too. Only the last one
would be used, so a block left behind by an edit would win.

## Example

Run `tumult init` to generate a sample experiment, or build one programmatically using the `tumult-core` types and encode with `toon_format::encode_default()`.
