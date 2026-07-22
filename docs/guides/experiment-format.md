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
| `title` | Yes | Human-readable experiment name |
| `description` | No | What this experiment validates |
| `tags` | Yes | Classification tags for filtering and analytics |
| `configuration` | No | Non-sensitive key-value pairs (inline or from environment variables) — usable in templates and injected into subprocess environments |
| `secrets` | No | Sensitive values (from environment variables or files) — env-injected, never journaled |
| `controls` | No | Lifecycle hooks executed at every lifecycle event (before/after experiment, method, activity, hypothesis, rollback) |
| `steady_state_hypothesis` | No | Probes that define "healthy" — checked before and after fault |
| `method` | Yes | Ordered sequence of actions and probes to execute |
| `rollbacks` | No | Actions to restore system state after the experiment |
| `estimate` | No | Phase 0 — prediction of expected outcome |
| `baseline` | No | Phase 1 — statistical baseline acquisition config |
| `load` | No | Load tool integration (k6) |
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

## Load

The optional `load` section drives load generation concurrent with the method. **k6 is the only supported driver** — `tool: k6`, and the `--load` CLI override accepts only `k6` or `none`. (The former `--load jmeter` CLI choice was removed: it never invoked JMeter — it silently ran k6.)

## Analysis (Phase 4)

Computed onto the journal after the run, comparing predictions (Phase 0) and measurements:

| Field | Description |
|-------|-------------|
| `estimate_accuracy` | 0.0–1.0 — how close the outcome was to the estimate |
| `resilience_score` | 0.0–1.0 composite score |
| `estimate_recovery_delta_s` | *Reserved — unpopulated in 2.17* |
| `trend` | *Reserved — unpopulated in 2.17* |

The degradation fields on during/post results (`degradation_onset_s`, `degradation_peak_s`, `degradation_magnitude`, `graceful_degradation`, `residual_degradation`) are likewise reserved and unpopulated in 2.17.

## Journal Export

`tumult export <journal> --format <fmt>` writes the journal's experiment record in four formats: `parquet` (Apache Parquet), `csv`, `json` (pretty-printed journal), and `arrow` (Apache Arrow IPC file format, suitable for zero-copy analytics interchange).

## Execution Target

Activities can specify WHERE they run:

| Target | Description |
|--------|-------------|
| `local` | Run on the machine running tumult (default) |
| `ssh` | Run via SSH on a remote host |
| `container` | Run inside a Docker/Podman container |
| `kube_exec` | Run via kubectl exec in a Kubernetes pod |

## Example

Run `tumult init` to generate a sample experiment, or build one programmatically using the `tumult-core` types and encode with `toon_format::encode_default()`.
