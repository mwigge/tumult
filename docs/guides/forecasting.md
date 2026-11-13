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
after the fault. A step with no observation has no impact value (`null`)
rather than an invented one.

### Flat series and the band floor

A probe that only ever reports `200`, `true` or a quantised gauge, and any
exactly periodic series, leaves no residuals: the empirical band has no
width, and an impact divided by it would be undefined or, with float noise
in the width, an absurd `1e15`. Every band therefore has a floor. It is
taken from the **reference samples only**: the pre-fault samples of a
`baseline`, the settled post-phase samples of a `post_backcast`. Fault
samples never count, so thirty-second timeouts cannot widen the band around
a fifty-millisecond baseline. The half width is never less than the larger
of

* 1 % of the reference's spread (half its 10–90 % range), or, when the
  reference does not vary at all, 0.1 % of its level (the median absolute
  value), and
* half the observed resolution (the smallest gap between two distinct
  values inside the 10–90 % range).

It is `1e-9` only when all of that is zero (a reference that is zero
throughout). The floor follows how the reference moves, not where it sits:
a gauge at 8e9 that moves by 1e6 is floored at a share of 1e6, and a series
of the order 1e-7 at a share of its own spread.

A band that had to be widened to the floor is flagged: `degenerate_band:
true` on the forecast, on the probe's impact entry and on the run-level
summary when any probe has it. The Impact panel prints a note next to the
numbers. Read such an impact as "the probe left its flat line by this many
floor widths": the direction and the timing are real, the multiple is in
units of a floor and not of an observed spread.

### Recovery time

Recovery is the first step at or after the end of the fault from which the
probe stays inside the band for three consecutive samples. Gaps neither
count towards the three nor reset them. `recovery_time_s` is the distance
from the end of the fault to that step; `null` means the run ended before
the probe settled.

The run-level figure is the slowest probe, and it is `null` (not
recovered) as soon as one evaluated probe never settled: a run is not back
because most of its probes are. `unrecovered_probes` says how many did not
settle, out of `evaluated_probes`; a probe that could not be evaluated
(listed with an `error`) counts in neither.

### Which samples the counterfactual comes from

Each probe's entry carries a `reference`:

| `reference` | Meaning |
|---|---|
| `baseline` | The run has samples before the fault. They are the context; the forecast runs forward across the fault and post phases. |
| `post_backcast` | The run has no pre-fault samples (the runner samples the during and post phases). The post-phase samples, reversed, are the context; the forecast runs backwards across the fault window. Post samples still recovering right after the fault (the run before the first sample inside the 10–90 % range of the later half of the post phase, at most the earlier half) are left out of the context, so the backcast does not extend the recovery slope across the fault window. A later sample far outside the range of the kept samples (more than three widths beyond their interquartile range; a width is that range, at least one step of a quantised metric and at least the band floor) is a gap in the context for the same reason: one stray spike next to the fault must not bend the backcast. Values that are not rare are not strays: when more than a tenth of the samples (or more than one, in a short phase) would be gapped, none is. A gapped sample is still drawn and still counted against the band. The post phase is compared with the 10–90 % range of the remaining, settled samples. |

The backcast assumes the system ended the run in its normal state. If it
did not — the post phase is still degraded — the counterfactual is too
pessimistic and the impact is understated. Check the `outside_after_fault`
count and the chart before quoting the number.

In a backcast the post phase is judged against the 10–90 % range of its own
settled samples. By construction about a fifth of perfectly settled samples
fall outside such a range, so `outside_after_fault` from a backcast is
biased upwards and is not residual impact. The recovery rule absorbs it
(three consecutive samples inside); a `baseline` reference has no such
bias.

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
`context_points`, `insufficient_history`, `degenerate_band`), `provenance`
and `cached`. Times come twice: `from_ns`/`to_ns` and `from_ms`/`to_ms`.
A browser should read the millisecond ones, because a JavaScript number
cannot hold epoch nanoseconds exactly.
`404` means the metric is unknown or has no points in the window.

Results are cached in the lake's `forecasts` table (schema v19), keyed by
series, model, step, horizon bucket and the context window. A cached
row is reused while the context window still starts and ends at the same
steps, so repeated calls within one step do not recompute, and a 14 d view
is never answered with a forecast fitted on 24 h. In the one-second tier
the window end moves once a minute, so a dashboard polling a short range
does not write a row per second. The cache row stores the provenance, so a
cached answer still says which model and which weights produced it. A
cache write that fails is logged and the forecast is returned anyway.

A forecast is fitted at one of four horizons (24, 96, 384 or 1,024 steps:
the smallest that holds the one asked for) and the response carries the
first `horizon` steps of it. Horizons 30 and 90 are therefore one fit and
one row; the `horizon` column of the table holds the fitted length, not
the asked one.

The table is bounded. Every write keeps only the newest 128 rows of the
series it wrote (the oldest go first) and deletes up to 256 rows older
than 30 days, oldest first, so a backlog shrinks with each write instead
of being paid for by one request.

Each provider has its own rows. While the sidecar is in use only a
sidecar row is a hit, so a Holt-Winters row written before the sidecar was
configured (or during an outage) never answers in its place. While the
sidecar is being skipped (see the circuit breaker below) the Holt-Winters
row is served, with the `fallback_reason` of the moment.

### `GET /api/runs/{id}/impact`

Returns one entry per probe with the series, the fault window, the impact
summary, recovery and provenance, plus the run-level `recovery_time_s`,
`unrecovered_probes`, `evaluated_probes`, `impact_peak` and
`degenerate_band`. Instants are given in nanoseconds and
in milliseconds (`fault.start_ms`, `recovery.recovered_at_ms`, `as_of_ms`).
A probe that cannot be compared (no samples during the
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

A fallback is never silent: `fallback_reason` carries a code, and the
daemon logs a warning with the detail (the address, the status, the parse
error), which stays out of the API response.

| `fallback_reason` | Meaning |
|---|---|
| `unreachable` | No connection to the sidecar |
| `timeout` | No answer in time |
| `bad_response` | A status that is not about this one request (5xx, and 401, 403, 404, 429 or any other 4xx not listed under `rejected`), or a body the daemon cannot use: not JSON, the wrong shape, other quantile levels than requested, values that are not finite |
| `rejected` | The sidecar answered 400, 413 or 422: it will not take this request. Also a request larger than the sidecar accepts, which is never sent |
| `circuit_open` | The sidecar is being skipped after a failure (below) |

The sidecar gets 3 seconds to accept a connection and 15 seconds per
request. The daemon follows no redirects, uses no proxy for this call and
reads at most 8 MiB of answer. `TUMULTD_FORECAST_URL` must be an `http://`
or `https://` URL.

### The circuit breaker

A sidecar that is down would otherwise cost every request its timeout.
After a failure that is the sidecar's own fault — it cannot be reached, it
times out, it answers 5xx or a body the daemon cannot read — the daemon
skips it for a cool-down and answers from Holt-Winters at once, with
`fallback_reason: circuit_open`. After the cool-down the circuit is half
open: one request tries the sidecar while the others keep getting the
fallback. Success closes the circuit, another failure starts a new
cool-down. An answer that arrives late, from a call made before the circuit
last opened, cannot close it.

`rejected` is the one code that does not open the circuit. It means the
sidecar turned down that one request with 400, 413 or 422 (a context it
will not take), so only that request falls back. A request too large to
send is turned down before the breaker is asked at all. If the rejected
request was the half-open probe, the circuit neither closes nor starts a
new cool-down: the next request is the probe.

Every other status opens the circuit, 4xx included. 404, 401 and 403 mean
the URL is wrong or something in front of the sidecar wants credentials;
429 and 503 mean it is throttling or full; 408 is reported as `timeout`.
Each of them would meet every request alike, so one request finds out and
the rest fall back at once instead of each paying for the round trip.

| Variable | Default | Meaning |
|---|---|---|
| `TUMULTD_FORECAST_COOLDOWN_S` | `30` | Seconds a failing sidecar is skipped. `0` disables the breaker: every request tries the sidecar. An unreadable value logs a warning and uses the default |

The state lives in the daemon's memory: a restart closes the circuit.

### Telemetry

Each forecast is a `forecast.produce` span with an `outcome` attribute
(`sidecar`, `built_in`, `fallback`, `error`); a call to the sidecar is a
child span `forecast.sidecar_call` whose `outcome` is `ok` or the failure
code. Three counters follow the same events:
`tumult.forecast.fallbacks` (by `reason`),
`tumult.forecast.breaker.transitions` (by `state`: `open`, `half_open`,
`closed`) and `tumult.forecast.cache.lookups` (by `result`: `hit`, `miss`).

**Built-in Holt-Winters.** Additive level and trend, plus an additive
season when the autocorrelation shows one and the context holds at least
three full periods. Smoothing parameters come from a fixed grid search, so
the same input always gives the same output. The band is empirical: the
quantiles of the one-step fit residuals, widened by the square root of the
lead time. `provenance.weights_hash` is `none`.

**Timestral sidecar.** A zero-shot probabilistic model that forecasts 21
quantiles per step and takes covariates. For run impact the daemon passes
the fault indicator as a covariate, set to zero across the horizon, which
asks the model for the no-fault path. In a backcast the context has no
fault samples either, so the indicator is zero throughout and carries no
information: the forecast is then the model's reading of the settled
samples alone. The provenance comes from the
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
| `FORECASTD_MODEL_REVISION` | `bdd20cfc15a3fea0e4cb5ccef37f8d3ca7aa335f` for the default model | Hub commit to load. Required for any other size or repository |
| `FORECASTD_WEIGHTS_SHA256` | `577236a8…04d63e` for the default model | Digest the weights must have. Checked before they are opened; the service does not start on a mismatch. Required for any other size or repository |
| `FORECASTD_WEIGHTS_PATH` | unset | Local snapshot directory |
| `FORECASTD_THREADS` | `4` (fewer on a smaller host) | PyTorch CPU threads |
| `FORECASTD_MAX_CONCURRENT` | `4` | Forecast requests in hand at once, from the first body byte to the answer. One more is answered `503` with `retry-after: 1` |
| `FORECASTD_LOG_FORMAT` | `json` | `json` or `text` (one line per event, for a terminal). Anything else stops the start |
| `FORECASTD_LOG_LEVEL` | `INFO` | Python log level |

The service takes at most 8 series and 8 covariates per request, 32,768
context steps, a horizon of 4,096 and a body of 8 MiB (`413` beyond it).
Values must be JSON numbers or `null` for a gap: a string, a boolean, `NaN`
or an infinity is a `400`, as is a body nested too deeply to parse and
anything else the model refuses (the client is told that the model refused,
the model's own words go to the log). Inference runs off the event loop, so
`/healthz` answers while a forecast is being computed.

Requests in hand are bounded. A request takes one of
`FORECASTD_MAX_CONCURRENT` slots before its body is read and gives it back
with its answer; when every slot is taken the next request gets `503` at
once instead of queueing behind the model. A body that takes more than 30
seconds to arrive is a `408`, so a stalled upload cannot hold a slot. The
daemon treats `503` as `bad_response` and `408` as `timeout`: both open the
circuit, and the requests of the cool-down are answered by Holt-Winters.

### Logs

The sidecar writes one JSON object per line on stderr and nothing else
there: its own events, uvicorn's, Python warnings and uncaught exceptions
all pass through the same formatter. The access log is the event
`http.access`.

```json
{"timestamp": "2026-11-13T21:02:15.503Z", "level": "INFO", "logger": "forecastd", "event": "http.access", "message": "request served", "fields": {"client": "127.0.0.1", "method": "POST", "path": "/forecast", "status": 200}}
```

| Key | Content |
|---|---|
| `timestamp` | RFC 3339, UTC, milliseconds |
| `level`, `logger` | Python's level name and logger name |
| `event` | A stable name: `model.loaded`, `startup.failed`, `http.access`, `forecast.failed`, `forecast.busy`, `model.refused`, … |
| `message` | A constant sentence. No value is ever interpolated into it |
| `fields` | Everything that varies. An exception is `error.type`, `error.message` and `error.stack` |

Strings are cut at 1 KiB (a stack at 8 KiB) and end in `...[truncated]` when
they were; a newline or any other control character in a value is escaped
by the encoder, so a value cannot start a second line. The daemon's own
forecast events follow the same naming (`error.type`, `error.message` as
fields, a constant message).

### The image

`forecastd/Dockerfile` builds in two stages from a base image pinned by
digest (Python 3.12). Every Python package comes from
`forecastd/requirements.lock` with hashes (`pip install --require-hashes`);
Timestral is built from its pinned commit against those packages. The final
image has no compiler, no `git` and no `curl`. The Compose service uses the
tag `tumult-forecastd:0.1.0`, a read-only root filesystem with a tmpfs
`/tmp`, no capabilities and `no-new-privileges`. To change a dependency
edit `forecastd/requirements.in` and regenerate the lock inside the base
image, as the header of that file describes.

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

Tested here with Python 3.14.8 in a virtualenv and with Python 3.12.16 in
the image, both with `torch` 2.15.0+cpu, `timestral` 1.0.0 at commit
`a534bdbd244d`, `numpy` 2.5.4, `safetensors` 0.8.0 and `huggingface_hub`
2.4.0. The virtualenv takes about 1.1 GB, nearly all of it PyTorch.

### The weights cache

On first start the sidecar downloads the weights into the Hugging Face
cache under `HF_HOME` (default `~/.cache/huggingface`) and reuses them
afterwards. Put `HF_HOME` on a disk with room and keep it across restarts:
the compose profile uses a named volume. `mistralai/Timestral-5M` is two
files, `config.json` and a 20.8 MB `model.safetensors`.

### The weights pin

The service loads one known set of weights or none. For the default model
it asks the hub for revision `bdd20cfc15a3fea0e4cb5ccef37f8d3ca7aa335f` and
expects the two files to hash to
`577236a804ccb3c2b02638c94b6b433920acf8edd248c9f7a6a32eb50104d63e`
(SHA-256 over `config.json` and `model.safetensors` in that order; other
files in the directory do not count). Each file enters the digest framed:
the length of its name, the name, the length of its content, the content,
lengths as 8 bytes big-endian. The lengths fix where one file ends and the
next begins, so no two sets of files share a digest by moving bytes between
them. `sha256sum` of the files does not give this number; compute it with
the service's own function:

```sh
cd forecastd && python -c 'import sys; from forecastd.model import CHECKPOINT_FILES, hash_weights; print(hash_weights(sys.argv[1], CHECKPOINT_FILES))' /path/to/Timestral-5M
```

The model never opens the path that was checked. The service copies the two
files into a new private directory under the temporary directory (mode
0500, files 0400), takes the digest over the bytes as it writes them, hands
the model that copy and removes it once the model is in memory. Files
swapped at the original path after the check are therefore not what gets
loaded. The temporary directory (`TMPDIR`, `/tmp` by default) must be
writable and hold one copy of the weights for the length of the start:
about 21 MB for the 5M model. The Compose service mounts a tmpfs there and
the Kubernetes manifest an `emptyDir`.

On a mismatch the copy is removed, the service logs `startup.failed` with
`error.type: WeightsMismatch` and both digests, and exits with status 1
without opening its socket.

The digest changed with the framing: a deployment that set
`FORECASTD_WEIGHTS_SHA256` to the earlier value for the same files
(`4178c86d…`) must set the new one.

Another size or repository has no built-in pin: set both
`FORECASTD_MODEL_REVISION` and `FORECASTD_WEIGHTS_SHA256` (the revision is
not needed with `FORECASTD_WEIGHTS_PATH`), or the service refuses to start.
The Compose service and the Kubernetes manifest set both values explicitly.
`/healthz` reports the revision and the SHA-256 of what was loaded.

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

CI (job `forecastd`) runs the suites on Python 3.12 with every package of
`requirements.lock` installed by hash. Timestral and the weights are not
there, so `test_timestral.py` skips; the stub suites, the ASGI app, the log
format and `tests/test_loading.py` run. That last one checks the order
resolve, copy, hash, compare, load with a fake loader, so "never load
unverified weights" is tested on every push and not only where the model is
installed.

`scripts/proof/rip-phase3-forecast/proof.sh` is the end-to-end check: it
starts the daemon on a fresh private lake, launches a run, starts the
sidecar with the real model, asserts that it serves exactly the pinned
revision and digest and that a second instance told to expect another
digest exits with status 1, asserts that forecast and impact provenance
name the model and its weights, kills the sidecar, asserts the
circuit-breaker fallback and brings it back. It also asserts that every
line the sidecar wrote on stderr, the refused start included, is a JSON log
entry. Run it after changing the model, its version or the weights.

It takes its paths from the environment: `FORECASTD_VENV` (default
`$XDG_DATA_HOME/tumult/forecastd-venv`), `HF_HOME`, `PROOF_WORK` and
`TUMULT_BIN_DIR`. It refuses to start when one of its ports is taken and
checks that the processes it started are the ones listening before it posts
anything. Exit status 0 is a pass, 1 a failed expectation, 2 a missing
model environment. `PROOF_OFFLINE_STUB=1` checks the wiring against the
stub and always exits 3 with `NOT A PROOF`.

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

The mounted directory is verified like a download: its two checkpoint
files must hash to the pinned digest, whatever else the download left next
to them. `/healthz` reports that SHA-256, from a mounted path or from the
hub cache alike, and the daemon repeats it as `provenance.weights_hash`. A
snapshot from the hub also carries its commit as `revision`; a plain
directory reads `local`.

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
