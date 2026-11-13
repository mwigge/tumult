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
| `POST /forecast` | `{"context": [..] or [[..], ..], "horizon": N, "quantile_levels": [0.1, 0.5, 0.9]?, "past_covariates": [[..]]?, "known_covariates": [[..]]?}` | `{"levels": [..], "quantiles": [[[q per level] per step] per variate], "context_points": n, "model": {"model", "model_id", "size", "version", "weights_hash"}}` |
| `GET /healthz` | — | `{"status": "ok", "model", "model_id", "size", "version", "weights_hash", "levels", "max_context", "max_horizon"}` |

`context` is the series contract: regular steps, `null` for gaps, at most
32,768 steps. Covariate rows are as long as the context (`past_covariates`)
or the context plus the horizon (`known_covariates`). Requested quantile
levels snap to the nearest of Timestral's 21 (0.01, 0.05, 0.10 … 0.90, 0.95,
0.99); omit `quantile_levels` to get all of them.

## Configuration

| Variable | Default | Meaning |
|---|---|---|
| `FORECASTD_BIND` | `0.0.0.0:8700` | Listen address |
| `FORECASTD_MODEL` | `timestral` | `timestral`, or `stub` (deterministic linear model, no torch — tests and smoke runs) |
| `FORECASTD_MODEL_SIZE` | `5M` | `5M`, `28M`, `123M`, `209M`. Start with 5M (CPU, tens of ms per forecast); evaluate a larger one on your own series before switching |
| `FORECASTD_MODEL_ID` | `mistralai/Timestral-<size>` | Hub repository id when not loading from a path |
| `FORECASTD_WEIGHTS_PATH` | unset | Local snapshot directory. Set it for offline and air-gapped hosts; `/healthz` then reports the SHA-256 of the files, otherwise `unverified` |
| `FORECASTD_LOG_LEVEL` | `INFO` | Python log level |

The model is loaded once, before the socket opens.

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
docker run -p 8700:8700 \
  -v "$PWD/weights/Timestral-5M:/weights:ro" \
  -e FORECASTD_WEIGHTS_PATH=/weights \
  tumult-forecastd
```

Without `FORECASTD_WEIGHTS_PATH` the container downloads the weights through
the Hugging Face hub into `HF_HOME` on first start, which needs egress.

## Running without Docker

```sh
cd forecastd
python -m venv .venv && . .venv/bin/activate
pip install ".[serve]"                                   # fastapi + uvicorn
pip install 'git+https://github.com/mistralai/timestral.git'   # pulls torch >= 2.4
FORECASTD_WEIGHTS_PATH=./weights/Timestral-5M python -m forecastd
```

Without `fastapi`/`uvicorn` installed, `python -m forecastd` serves the same
two routes with the standard-library HTTP server.

## Tests

The tests use the stub model and the standard library only (numpy is the
one dependency):

```sh
cd forecastd && python -m unittest discover -s tests -v
```

## Deployment

* Compose: `TUMULTD_FORECAST_URL=http://forecastd:8700 docker compose -f docker/docker-compose.kronika.yml --profile forecast up`
  starts the sidecar next to `tumultd` and points the daemon at it.
* Kubernetes: `deploy/k8s/tumult-forecastd.yaml` (Deployment + Service; add
  `TUMULTD_FORECAST_URL=http://tumult-forecastd:8700` to the `tumultd`
  Deployment).

When the sidecar is unset or unreachable the daemon falls back to the
built-in provider and says so in the response (`provenance.model`).
