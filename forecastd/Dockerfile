# tumult-forecastd: the Timestral forecasting sidecar.
#
# The image ships the service, the ASGI stack, CPU-only PyTorch and the
# Timestral code. It does NOT ship the model weights: they are released under
# Mistral's modified MIT licence and must be fetched by the operator (see
# README.md). Mount them at /weights and set FORECASTD_WEIGHTS_PATH, or let the
# container download them through the Hugging Face hub on first start (needs
# egress; not for air-gapped hosts).
#
#   docker build -f forecastd/Dockerfile -t tumult-forecastd forecastd/
#   docker run -p 8700:8700 -v ./weights/Timestral-5M:/weights:ro \
#     -e FORECASTD_WEIGHTS_PATH=/weights tumult-forecastd
#
# The image loads only the weights it was tested with: the default 5M model
# is pinned to a hub revision and to the SHA-256 of its files (see
# forecastd/model.py), and the service refuses to start on any other bytes.
#
# Python packages come from requirements.lock (exact versions with hashes,
# installed with --require-hashes); the Timestral code is built from its
# pinned git commit in the build stage. The final stage has no git, no curl
# and no compiler.
#
# The default is the real model. FORECASTD_MODEL=stub is the offline mode: a
# linear stand-in that needs no weights and names itself `stub` in every
# response.

# python:3.12-slim-bookworm (Python 3.12.16), pinned by digest. The test
# suites run on this interpreter in CI and against the built image.
ARG PYTHON_IMAGE=python@sha256:935c3c5b12357a7e10caa9c1e6ca5692344d97cf229832ef584be2cec9fc03df

FROM ${PYTHON_IMAGE} AS build

ENV PIP_NO_CACHE_DIR=1 \
    PIP_DISABLE_PIP_VERSION_CHECK=1 \
    PYTHONDONTWRITEBYTECODE=1

# The Timestral release the service is tested against (1.0.0, Apache-2.0).
ARG TIMESTRAL_COMMIT=a534bdbd244d64f3d9d743f46c7bf6fc330ed4ff

RUN apt-get update \
    && apt-get install -y --no-install-recommends git ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN python -m venv /opt/venv
ENV PATH=/opt/venv/bin:$PATH

# CPU-only torch keeps the image a fraction of the CUDA build's size; the 5M
# model forecasts in tens of milliseconds on a few CPU threads.
COPY requirements.lock /tmp/requirements.lock
RUN pip install --require-hashes --no-deps -r /tmp/requirements.lock

# A git commit is its own content hash; --no-deps keeps the resolver away
# from anything that is not in the lock, and the build uses the locked
# setuptools rather than fetching a build backend.
RUN pip install --no-deps --no-build-isolation \
      "timestral @ git+https://github.com/mistralai/timestral.git@${TIMESTRAL_COMMIT}" \
    && pip check \
    && python -c "import timestral, torch; print('timestral', timestral.__version__, 'torch', torch.__version__)"

FROM ${PYTHON_IMAGE}

ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    PATH=/opt/venv/bin:$PATH \
    FORECASTD_BIND=0.0.0.0:8700 \
    FORECASTD_MODEL=timestral \
    FORECASTD_MODEL_SIZE=5M \
    FORECASTD_MODEL_REVISION=bdd20cfc15a3fea0e4cb5ccef37f8d3ca7aa335f \
    FORECASTD_WEIGHTS_SHA256=577236a804ccb3c2b02638c94b6b433920acf8edd248c9f7a6a32eb50104d63e \
    FORECASTD_THREADS=4 \
    HF_HOME=/cache/hf

RUN useradd --create-home --user-group --uid 10002 forecastd \
    && mkdir -p /cache/hf /weights \
    && chown -R forecastd:forecastd /cache /weights

COPY --from=build /opt/venv /opt/venv
WORKDIR /app
COPY forecastd/ ./forecastd/

USER forecastd
EXPOSE 8700

HEALTHCHECK --interval=10s --timeout=3s --start-period=30s --retries=5 \
    CMD ["python", "-c", "import sys, urllib.request; sys.exit(urllib.request.urlopen('http://127.0.0.1:8700/healthz', timeout=2).status != 200)"]

CMD ["python", "-m", "forecastd"]
