# Experiment Format

Tumult experiments are defined in TOON (Token-Oriented Object Notation) — the only supported format. TOON is human-readable, token-efficient, and serde-compatible.

## Structure

Every experiment has these sections:

| Section | Required | Description |
|---------|----------|-------------|
| `title` | Yes | Human-readable experiment name |
| `description` | No | What this experiment validates |
| `tags` | Yes | Classification tags for filtering and analytics |
| `configuration` | No | Key-value pairs resolved from environment variables |
| `secrets` | No | Sensitive configuration (env vars, file paths) |
| `controls` | No | Lifecycle hooks (before/after experiment, method, activity) |
| `steady_state_hypothesis` | No | Probes that define "healthy" — checked before and after fault |
| `method` | Yes | Ordered sequence of actions and probes to execute |
| `rollbacks` | No | Actions to restore system state after the experiment |
| `estimate` | No | Phase 0 — prediction of expected outcome |
| `baseline` | No | Phase 1 — statistical baseline acquisition config |
| `load` | No | Load tool integration (k6, JMeter) |
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
| `process` | Run a script or binary | `path`, `arguments`, `env`, `timeout_s` |
| `http` | Make an HTTP request | `method`, `url`, `headers`, `body`, `timeout_s` |

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
