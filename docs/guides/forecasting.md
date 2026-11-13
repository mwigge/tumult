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
| `post_backcast` | The run has no pre-fault samples (the runner samples the during and post phases). The post-phase samples, reversed, are the context; the forecast runs backwards across the fault window. Post samples still recovering right after the fault (the run before the first sample inside the 10–90 % range of the later half of the post phase, at most the earlier half) are left out of the context, so the backcast does not extend the recovery slope across the fault window. The post phase is compared with the 10–90 % range of the remaining, settled samples. |

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

Each provider has its own rows. While the sidecar is in use only a
sidecar row is a hit, so a Holt-Winters row written before the sidecar was
configured (or during an outage) never answers in its place. While the
sidecar is being skipped (see the circuit breaker below) the Holt-Winters
row is served, with the `fallback_reason` of the moment.

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
daemon logs a warning. The sidecar gets 3 seconds to accept a connection
and 15 seconds per request.

### The circuit breaker

A sidecar that is down would otherwise cost every request its timeout.
After a failure that is the sidecar's own fault — it cannot be reached, it
times out, it answers 5xx or a body the daemon cannot read — the daemon
skips it for a cool-down and answers from Holt-Winters at once. The
`fallback_reason` then reads
`sidecar circuit open, retry in N s: <the error that opened it>`. The first
request after the cool-down tries the sidecar again: success closes the
circuit, another failure starts a new cool-down.

A 4xx answer does not open the circuit. It means the sidecar turned down
that one request (a context it will not take), so only that request falls
back.

| Variable | Default | Meaning |
|---|---|---|
| `TUMULTD_FORECAST_COOLDOWN_S` | `30` | Seconds a failing sidecar is skipped. `0` disables the breaker: every request tries the sidecar. An unreadable value logs a warning and uses the default |

The state lives in the daemon's memory: a restart closes the circuit.

**Built-in Holt-Winters.** Additive level and trend, plus an additive
season when the autocorrelation shows one and the context holds at least
three full periods. Smoothing parameters come from a fixed grid search, so
the same input always gives the same output. The band is empirical: the
quantiles of the one-step fit residuals, widened by the square root of the
lead time. `provenance.weights_hash` is `none`.

**Timestral sidecar.** A zero-shot probabilistic model that forecasts 21
quantiles per step and takes covariates. For run impact the daemon passes
the fault indicator as a covariate, set to zero across the horizon, which
asks the model for the no-fault path. The provenance comes from the
sidecar's response: `provenance.version` reads
`<model id>@<weights revision> (timestral <package version>)`, for example
`mistralai/Timestral-5M@bdd20cfc… (timestral 1.0.0)`, and
`provenance.weights_hash` is the SHA-256 of the weight files it loaded. The
model does not sample: the same input gives the same forecast.

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
| `FORECASTD_MODEL` | `timestral` | `timestral` is the real model. `stub` is the **offline mode**: a linear stand-in with no torch and no weights, for unit tests and for checking the wiring on a host that cannot fetch the model. It names itself `stub` in every response, so a forecast can never pass for a Timestral one |
| `FORECASTD_MODEL_SIZE` | `5M` | `5M`, `28M`, `123M` or `209M` |
| `FORECASTD_MODEL_ID` | `mistralai/Timestral-<size>` | Hub repository when not loading from a path |
| `FORECASTD_MODEL_REVISION` | the repository's head | Hub commit to load. Pin it so every start loads the same weights |
| `FORECASTD_WEIGHTS_PATH` | unset | Local snapshot directory |
| `FORECASTD_THREADS` | `4` (fewer on a smaller host) | PyTorch CPU threads |

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

### Installing without Docker

The sidecar runs from a virtualenv. Timestral is not on PyPI; install the
release the service is tested against (`timestral` 1.0.0, Apache-2.0) from
its repository by commit. On a host without a GPU take PyTorch from the CPU
wheel index first, or pip pulls the CUDA build, several gigabytes larger.

```sh
python3 -m venv /srv/forecastd/venv
. /srv/forecastd/venv/bin/activate
pip install --index-url https://download.pytorch.org/whl/cpu 'torch>=2.4'
pip install 'timestral @ git+https://github.com/mistralai/timestral.git@a534bdbd244d64f3d9d743f46c7bf6fc330ed4ff'
pip install './forecastd[serve]'
HF_HOME=/srv/forecastd/hf FORECASTD_BIND=127.0.0.1:8700 python -m forecastd
```

Tested here with Python 3.14.8, `torch` 2.15.0+cpu, `timestral` 1.0.0 at
commit `a534bdbd244d`, `numpy` 2.5.4, `safetensors` 0.8.0 and
`huggingface_hub` 2.4.0. The virtualenv takes about 1.1 GB, nearly all of it
PyTorch.

### The weights cache

On first start the sidecar downloads the weights into the Hugging Face
cache under `HF_HOME` (default `~/.cache/huggingface`) and reuses them
afterwards. Put `HF_HOME` on a disk with room and keep it across restarts:
the compose profile uses a named volume. `mistralai/Timestral-5M` is two
files, `config.json` and a 20.8 MB `model.safetensors`. The revision tested
here is `bdd20cfc15a3fea0e4cb5ccef37f8d3ca7aa335f`; set
`FORECASTD_MODEL_REVISION` to it to load exactly those files. `/healthz`
reports the revision and the SHA-256 of what was loaded.

### CPU latency and memory

Measured on a 24-core x86-64 host without a GPU, `mistralai/Timestral-5M`,
four PyTorch threads, while the host was busy with unrelated builds (load
average near 30). Times are for one forecast inside the sidecar, median of
ten (three for the longest context); the HTTP hop and the daemon add a few
milliseconds.

| Context steps | Horizon | Median |
|---|---|---|
| 60 | 20 | 38 ms |
| 512 | 24 | 25 ms |
| 1,440 | 24 | 42 ms |
| 4,096 | 96 | 105 ms |
| 32,768 | 24 | 508 ms |

Loading the model takes about two seconds. The process holds about 300 MB
resident after loading and peaked at 335 MB across those runs, so a 1 GB
limit leaves room; the larger sizes need more and have not been measured.

`FORECASTD_THREADS` matters on a shared host. With PyTorch's default (half
the cores, twelve here) the same 512-step forecast took 1.2 s under that
load, fifty times slower than with four threads, because the threads fight
the other work for cores. The 5M model gains nothing past four. Forecasts
are served one at a time.

The larger sizes have not been evaluated on Tumult series; measure before
you switch.

### Verifying the model path

The sidecar's unit tests run without PyTorch on the stub. The tests in
`forecastd/tests/test_timestral.py` load the real weights and check the
output shape, the ordering of the quantiles, determinism, gaps, covariates,
the horizon and context limits and the CPU latency; they skip only when
`torch` or `timestral` is not installed.

```sh
cd forecastd
HF_HOME=/srv/forecastd/hf /srv/forecastd/venv/bin/python -m unittest discover -s tests -t . -v
```

`scripts/proof/rip-phase3-forecast/proof.sh` is the end-to-end check: it
starts the daemon on a fresh lake, launches a run, starts the sidecar with
the real model, asserts that forecast and impact provenance name the model
and its weights revision, kills the sidecar, asserts the circuit-breaker
fallback and brings it back. Run it after changing the model, its version
or the weights.

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

`/healthz` reports the SHA-256 over the snapshot files it loaded, from a
mounted path or from the hub cache alike, and the daemon repeats it as
`provenance.weights_hash`. Record that hash when you approve a snapshot; a
different hash later means different weights. A snapshot from the hub also
carries its commit as `revision`; a plain directory reads `local`.

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
