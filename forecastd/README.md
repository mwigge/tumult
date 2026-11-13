# tumult-forecastd

The optional Timestral forecasting sidecar for the Tumult daemon. `tumultd`
always has a forecast band — the built-in Holt-Winters provider in
`tumult-forecast` needs no weights and no network — and this service is the
upgrade: Mistral's Timestral, a zero-shot probabilistic time-series model
that forecasts 21 quantiles per step and conditions on covariates (the
fault indicator, in Tumult's case). Point the daemon at it with
`TUMULTD_FORECAST_URL=http://forecastd:8700` and every forecast response
names the model that produced it.

## Endpoints

| Route | Body | Returns |
|---|---|---|
| `POST /forecast` | `{"context": [..] or [[..], ..], "horizon": N, "quantile_levels": [0.1, 0.5, 0.9]?, "past_covariates": [[..]]?, "known_covariates": [[..]]?}` | `{"levels": [..], "quantiles": [[[q per level] per step] per variate], "context_points": n, "model": {"model", "model_id", "size", "version", "revision", "weights_hash"}}` |
| `GET /healthz` | — | `{"status": "ok", "model", "model_id", "size", "version", "revision", "weights_hash", "levels", "max_context", "max_horizon", "max_variates", "max_covariates", "max_body_bytes"}` |

`context` is the series contract: regular steps, `null` for gaps, at most
32,768 steps, 8 series, a horizon of 4,096. Values are JSON numbers or
`null`; a string, a boolean, `NaN` or an infinity is a `400`, and so is a
body nested too deeply to parse and an input the model itself refuses (the
answer says that the model refused; what it said is in the log). A body
over 8 MiB is a `413`, one that takes more than 30 seconds in all to arrive
a `408` (stalled or dripped, on either server), and a request that finds
every slot taken (`FORECASTD_MAX_CONCURRENT`)
a `503` with `retry-after: 1`. Covariate
rows (8 at most of each kind) are as long as the context (`past_covariates`)
or the context plus the horizon (`known_covariates`). Requested quantile
levels snap to the nearest of Timestral's 21 (0.01, 0.05, 0.10 … 0.90, 0.95,
0.99); omit `quantile_levels` to get all of them.

## Configuration

| Variable | Default | Meaning |
|---|---|---|
| `FORECASTD_BIND` | `0.0.0.0:8700` | Listen address |
| `FORECASTD_MODEL` | `timestral` | `timestral` is the real model. `stub` is the offline mode: a deterministic linear stand-in with no torch and no weights, for unit tests and wiring checks. It reports `model: stub`, so its forecasts cannot pass for Timestral's |
| `FORECASTD_MODEL_SIZE` | `5M` | `5M`, `28M`, `123M`, `209M`. Start with 5M (CPU, tens of ms per forecast); evaluate a larger one on your own series before switching |
| `FORECASTD_MODEL_ID` | `mistralai/Timestral-<size>` | Hub repository id when not loading from a path |
| `FORECASTD_MODEL_REVISION` | `bdd20cfc15a3fea0e4cb5ccef37f8d3ca7aa335f` for the default model | Hub commit to load. Required for any other size or repository, unless `FORECASTD_WEIGHTS_PATH` is set |
| `FORECASTD_WEIGHTS_SHA256` | `577236a804ccb3c2b02638c94b6b433920acf8edd248c9f7a6a32eb50104d63e` for the default model | Digest the weights must have (SHA-256 over `config.json` and `model.safetensors`, framed as described below). Required for any other size or repository |
| `FORECASTD_WEIGHTS_PATH` | unset | Local snapshot directory. Set it for offline and air-gapped hosts |
| `FORECASTD_THREADS` | `4` (fewer on a smaller host) | PyTorch CPU threads. The 5M model gains nothing past four, and on a busy host more threads make each forecast slower |
| `FORECASTD_MAX_CONCURRENT` | `4` | Forecast requests in hand at once, body upload included. One more gets `503` at once rather than a place in a queue |
| `FORECASTD_LOG_FORMAT` | `json` | `json` (one object per line) or `text` (one readable line per event). Anything else stops the start |
| `FORECASTD_LOG_LEVEL` | `INFO` | Python log level |

The model is loaded once, before the socket opens. On a digest mismatch, a
missing pin or any other load failure the service logs `startup.failed` and
exits with status 1. `/healthz` and every forecast carry `revision` (the
hub commit of the loaded snapshot, `local` for a plain directory, `none`
for the stub) and `weights_hash` (the SHA-256 of the loaded files).
Forecasts are computed one at a time, off the event loop.

### The weights digest

`weights_hash` is one SHA-256 over `config.json` and `model.safetensors`,
in that order. Each file is framed: the length of its name, the name, the
length of its content, the content (lengths as 8 bytes, big-endian), so the
digest also fixes where one file ends and the next begins. To compute it
for a directory:

```sh
python -c 'import sys; from forecastd.model import CHECKPOINT_FILES, hash_weights; print(hash_weights(sys.argv[1], CHECKPOINT_FILES))' ./weights/Timestral-5M
```

The model does not open the directory that was checked. The two files are
copied into a new private directory under the temporary directory (mode
0500, files 0400), the digest is taken over the bytes as they are written,
the model loads from that copy and the copy is removed. Swapping the files
at the original path after the check changes nothing. `TMPDIR` (default
`/tmp`) must be writable with room for one copy of the weights, 21 MB for
the 5M model; `--read-only` therefore needs `--tmpfs /tmp`, as below.

### Logs

One JSON object per line on stderr, and nothing else there: the service's
events, uvicorn's, Python warnings and uncaught exceptions use one
formatter. Keys: `timestamp` (RFC 3339, UTC), `level`, `logger`, `event` (a
stable name such as `model.loaded`, `http.access`, `forecast.busy`,
`forecast.failed`, `model.refused`, `startup.failed`), `message` (a
constant sentence, nothing interpolated) and `fields` (everything that
varies; an exception is `error.type`, `error.message`, `error.stack`).
Strings are cut at 1 KiB and control characters are escaped by the JSON
encoder, so a value from a client or from the model cannot forge a line.
URLs in an error lose their query string and credentials (a failed
download quotes its signed link). A value whose `__str__` raises is
described instead, a record that cannot be formatted becomes a
`log.unformattable` line, and a closed stderr drops the line, not the
request.
`FORECASTD_LOG_FORMAT=text` is for reading at a terminal.

## Weights and the licence

The Timestral code is Apache-2.0; the weights are released under Mistral's
modified MIT licence. Read the `LICENSE` in the weights repository before
redistributing them. The container image therefore never bundles weights.
Fetch them yourself:

```sh
pip install huggingface_hub
huggingface-cli download mistralai/Timestral-5M --local-dir ./weights/Timestral-5M
```

For an air-gapped host, copy that directory across and mount it read-only:

```sh
docker run -p 8700:8700 --read-only --tmpfs /tmp --cap-drop ALL \
  -v "$PWD/weights/Timestral-5M:/weights:ro" \
  -e FORECASTD_WEIGHTS_PATH=/weights -e HF_HUB_OFFLINE=1 \
  tumult-forecastd:0.1.0
```

Only `config.json` and `model.safetensors` are hashed, so the README and
the bookkeeping a download leaves next to them do not change the digest.

Without `FORECASTD_WEIGHTS_PATH` the container downloads the weights through
the Hugging Face hub into `HF_HOME` on first start, which needs egress.

## Running without Docker

```sh
cd forecastd
python -m venv .venv && . .venv/bin/activate
pip install --index-url https://download.pytorch.org/whl/cpu 'torch>=2.4'   # CPU build
pip install 'timestral @ git+https://github.com/mistralai/timestral.git@a534bdbd244d64f3d9d743f46c7bf6fc330ed4ff'
pip install ".[serve]"                                   # fastapi + uvicorn
FORECASTD_WEIGHTS_PATH=./weights/Timestral-5M python -m forecastd
```

Timestral is not on PyPI; the commit above is release 1.0.0, the one the
tests run against. Without `FORECASTD_WEIGHTS_PATH` the weights are fetched
into `HF_HOME` on first start.

Without `fastapi`/`uvicorn` installed, `python -m forecastd` serves the same
two routes with the standard-library HTTP server.

## The image and its lock file

`Dockerfile` is a two-stage build on a base image pinned by digest (Python
3.12). `requirements.in` names the direct dependencies at exact versions;
`requirements.lock` is generated from it with hashes and installed with
`--require-hashes`. Timestral is built from its pinned commit against those
packages, without its own dependency resolution. The final stage carries
no compiler, `git` or `curl`.

Regenerate the lock inside the base image, so the hashes match its Python:

```sh
pip install pip-tools
pip-compile --generate-hashes --allow-unsafe --strip-extras \
  --extra-index-url https://download.pytorch.org/whl/cpu \
  --output-file requirements.lock requirements.in
```

## Tests

The service tests use the stub model and the standard library only (numpy
is the one dependency). `tests/test_timestral.py` loads the real model and
skips when `torch` or `timestral` is not installed:

```sh
cd forecastd && python -m unittest discover -s tests -t . -v
```

Run them from the virtualenv above to exercise the real model (the pinned
revision and digest, refusal on a mismatch, shape, quantile ordering,
determinism, limits, CPU latency). CI runs on Python 3.12 with the packages
of `requirements.lock`, installed by hash: the stub suites, the ASGI app,
the log format and `tests/test_loading.py`, which checks with a fake loader
that weights are copied, hashed and compared before anything loads them.
The real-model suite skips there (no Timestral, no weights); run it from
the virtualenv or against the image before a release. Measured latency and
memory are in `docs/guides/forecasting.md`.

## Deployment

* Compose: `TUMULTD_FORECAST_URL=http://forecastd:8700 docker compose -f docker/docker-compose.kronika.yml --profile forecast up`
  starts the sidecar next to `tumultd` and points the daemon at it.
* Kubernetes: `deploy/k8s/tumult-forecastd.yaml` (Deployment + Service; add
  `TUMULTD_FORECAST_URL=http://tumult-forecastd:8700` to the `tumultd`
  Deployment).

When the sidecar is unset or unreachable the daemon falls back to the
built-in provider and says so in the response (`provenance.model`,
`provenance.fallback_reason`, a code such as `unreachable` or
`circuit_open`). A failing sidecar is skipped for
`TUMULTD_FORECAST_COOLDOWN_S` seconds (default 30) before one request tries
it again.
