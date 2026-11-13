---
title: Forecasting and run impact
parent: Guides
nav_order: 31
---

# Forecasting and run impact

Tumult forecasts two things from the telemetry already in the lake:

* **Where a metric is heading** — `GET /api/forecast` returns a semantic
  metric's recent series with a forecast band for the next steps.
* **What a run did to the system** — `GET /api/runs/{id}/impact` compares
  each probe's samples with a counterfactual ("what the probe would have
  read without the fault") and reports the size of the deviation and the
  time it took to come back. The run page draws it as the **Impact** panel.

Every response names the model that produced the numbers. Nothing in this
feature needs a network connection or model weights: the built-in provider
is plain Rust. A sidecar running Mistral's Timestral model is optional.

## Reading the band

A forecast is a set of quantiles per future step. The API returns three by
default — 0.1, 0.5 and 0.9 — and the charts draw the 0.5 quantile as the
dashed median and the 0.1–0.9 range as the shaded **80 % band**.

Read the band as: *if the series keeps behaving the way it did in the
context window, about eight of ten future values fall inside it.* It is not
a guarantee, and it is not an SLO threshold. A value outside the band means
"unusual compared with recent history", which is what a fault is supposed
to cause.

The band widens with the horizon: uncertainty about step 20 is larger than
about step 1.

### Impact

Impact is the observed value's distance from the counterfactual median,
measured in half band widths:

```text
impact = (observed − median) / ((upper − lower) / 2)
```

`0` sits on the median, `±1` on the edge of the band, `4` is four half
widths above it. The summary carries the mean and the peak across the fault
window, the share of fault-window samples outside the band, and the mean
after the fault. A step with no observation, or a band with no width (a
perfectly constant context), has no impact value (`null`) rather than an
invented one.

### Recovery time

Recovery is the first step at or after the end of the fault from which the
probe stays inside the band for three consecutive samples. Gaps neither
count towards the three nor reset them. `recovery_time_s` is the distance
from the end of the fault to that step; `null` means the run ended before
the probe settled. The run-level figure is the slowest probe.

### Which samples the counterfactual comes from

Each probe's entry carries a `reference`:

| `reference` | Meaning |
|---|---|
| `baseline` | The run has samples before the fault. They are the context; the forecast runs forward across the fault and post phases. |
| `post_backcast` | The run has no pre-fault samples (the runner samples the during and post phases). The post-phase samples, reversed, are the context; the forecast runs backwards across the fault window. The post phase is compared with its own 10–90 % range. |

The backcast assumes the system ended the run in its normal state. If it
did not — the post phase is still degraded — the counterfactual is too
pessimistic and the impact is understated. Check the `outside_after_fault`
count and the chart before quoting the number.

## The endpoints

### `GET /api/forecast`

| Parameter | Default | Meaning |
|---|---|---|
| `metric` | required | A semantic metric name (see `/api/metrics`) |
| `target` | all | Confine the series to one target system |
| `env` | the caller's scopes | Confine to one environment; `403` outside the caller's scopes |
| `range` | `24h` | Context window: `1h`, `6h`, `24h`, `7d`, `14d`, `30d`, `90d` |
| `horizon` | `24` | Steps to forecast, 1–1024 |

The series is resampled onto a regular step chosen from the window (the
series contract below). The body carries `series` (chart-ready: `ts`,
`observed`, `median`, `bands`), `forecast` (`levels`, `quantiles[step][level]`,
`context_points`, `insufficient_history`), `provenance` and `cached`.
`404` means the metric is unknown or has no points in the window.

Results are cached in the lake's `forecasts` table (schema v19), keyed by
series, model, step and horizon. A cached row is reused while the context
window still ends at the same step, so repeated calls within one step do
not recompute. The cache row stores the provenance, so a cached answer
still says which model and which weights produced it.

### `GET /api/runs/{id}/impact`

Returns one entry per probe with the series, the fault window, the impact
summary, recovery and provenance, plus the run-level `recovery_time_s` and
`impact_peak`. A probe that cannot be compared (no samples during the
fault, or none after it) is listed with an `error` instead of numbers.
`404` means the run is unknown, outside the caller's environment scopes, or
has no probe samples. Both endpoints need the Viewer role.

## The series contract

Providers receive a regular series, never raw points:

* one value per step, `NaN` (JSON `null`) where a step has no observation;
* at most 32,768 steps — longer windows keep the most recent ones;
* the step is the finest of 1 s, 1 min and 1 h that fits the window in
  32,768 steps: 1 s up to about nine hours (a run), 1 min up to about three
  weeks (a day, a week), 1 h beyond that (a quarter).

Probe samples use their own sampling interval as the step.

## Choosing a provider

| `TUMULTD_FORECAST_URL` | Provider | `provenance.model` |
|---|---|---|
| unset or empty | Built-in Holt-Winters | `holt-winters` |
| set, sidecar answers | Timestral sidecar | `timestral` |
| set, sidecar unreachable or erroring | Built-in Holt-Winters | `holt-winters`, with `provenance.fallback_reason` saying why |

A fallback is never silent: `fallback_reason` carries the error, and the
daemon logs a warning. The sidecar gets 15 seconds per request.

**Built-in Holt-Winters.** Additive level and trend, plus an additive
season when the autocorrelation shows one and the context holds at least
three full periods. Smoothing parameters come from a fixed grid search, so
the same input always gives the same output. The band is empirical: the
quantiles of the one-step fit residuals, widened by the square root of the
lead time. `provenance.weights_hash` is `none`.

**Timestral sidecar.** A zero-shot probabilistic model that forecasts 21
quantiles per step and takes covariates. For run impact the daemon passes
the fault indicator as a covariate, set to zero across the horizon, which
asks the model for the no-fault path. `provenance.version` and
`provenance.weights_hash` come from the sidecar's response.

Start with the built-in provider. Move to the sidecar when you have series
with structure Holt-Winters cannot follow (several seasonalities, regime
changes), and compare both on your own data before trusting either: the
`provenance` in each response makes that comparison straightforward.

## Deploying the sidecar

The service lives in `forecastd/` (Python). It exposes `POST /forecast` and
`GET /healthz`, loads the model once before it opens the socket, and has no
authentication of its own: keep it on a private network next to the daemon.

| Variable | Default | Meaning |
|---|---|---|
| `FORECASTD_BIND` | `0.0.0.0:8700` | Listen address |
| `FORECASTD_MODEL` | `timestral` | `timestral`, or `stub` (a deterministic linear model with no weights, for smoke tests) |
| `FORECASTD_MODEL_SIZE` | `5M` | `5M`, `28M`, `123M` or `209M` |
| `FORECASTD_MODEL_ID` | `mistralai/Timestral-<size>` | Hub repository when not loading from a path |
| `FORECASTD_WEIGHTS_PATH` | unset | Local snapshot directory |

Docker Compose (the demo stack; the sidecar sits behind the `forecast`
profile and does not start with a plain `up`):

```sh
TUMULTD_FORECAST_URL=http://forecastd:8700 \
  docker compose -f docker/docker-compose.kronika.yml --profile forecast up
```

Kubernetes: apply `deploy/k8s/tumult-forecastd.yaml` (Deployment, Service,
weights volume claim and a NetworkPolicy that admits only the `tumultd`
pod), then set `TUMULTD_FORECAST_URL=http://tumult-forecastd:8700` on the
`tumultd` Deployment.

The 5M model answers in tens of milliseconds on a couple of CPU cores. The
larger sizes have not been evaluated on Tumult series; measure before you
switch.

## Weights and air-gapped hosts

The image contains the service and the Timestral code, **not the weights**.
The weights are published under Mistral's modified MIT licence; read the
licence in the weights repository before you redistribute them.

On a connected host the sidecar downloads the weights from the Hugging Face
hub on first start. For an air-gapped host:

1. On a connected machine:
   `huggingface-cli download mistralai/Timestral-5M --local-dir ./weights/Timestral-5M`
2. Copy the directory to the host (or into the `tumult-forecastd-weights`
   volume).
3. Mount it read-only and set `FORECASTD_WEIGHTS_PATH` to the mount point.
   Set `HF_HUB_OFFLINE=1` so nothing tries the network.

With a weights path, `/healthz` reports the SHA-256 over the snapshot
files, and the daemon repeats it as `provenance.weights_hash`. Record that
hash when you approve a snapshot; a different hash later means different
weights. Without a path the hash reads `unverified`.

Without the sidecar the daemon needs nothing: the built-in provider works
with no egress.

## Limits

* **Short history.** Under eight observed points the built-in provider
  returns a flat forecast at the mean with a band from the few points
  there are, and sets `insufficient_history: true`. Probe series from a
  short run are often in this state; the Impact panel says so. Treat those
  numbers as a rough indication.
* **The band describes the recent past.** A deploy, a traffic shift or an
  incident inside the context window is part of "normal" as far as the
  model can tell.
* **Backcast bias.** See above: a run that has not recovered by its last
  sample understates its own impact.
* **No causal claim.** Impact is "the probe deviated from its forecast
  during the fault window". Something else that happened in the same
  seconds is counted too.
* **Gaps.** Missing steps are carried as gaps, not filled with guesses in
  the output; the built-in fit bridges them internally by interpolation.
* **Recovery resolution** is one sampling step. A probe sampled every five
  seconds cannot show a two-second recovery.
* **ClickHouse backend.** The `forecasts` cache and probe samples are
  DuckDB-lake only.
