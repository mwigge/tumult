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

FROM python:3.12-slim-bookworm

ENV PIP_NO_CACHE_DIR=1 \
    PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    FORECASTD_BIND=0.0.0.0:8700 \
    FORECASTD_MODEL=timestral \
    FORECASTD_MODEL_SIZE=5M \
    HF_HOME=/cache/hf

RUN apt-get update \
    && apt-get install -y --no-install-recommends git curl ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --user-group --uid 10002 forecastd

WORKDIR /app
COPY pyproject.toml README.md ./
COPY forecastd/ ./forecastd/

# CPU-only torch keeps the image a fraction of the CUDA build's size; the 5M
# model forecasts in tens of milliseconds on a few CPU threads.
RUN pip install --index-url https://download.pytorch.org/whl/cpu "torch>=2.4" \
    && pip install ".[serve]" \
    && pip install "git+https://github.com/mistralai/timestral.git"

RUN mkdir -p /cache/hf /weights && chown -R forecastd:forecastd /cache /weights
USER forecastd
EXPOSE 8700

HEALTHCHECK --interval=10s --timeout=3s --start-period=30s --retries=5 \
    CMD curl -sf http://127.0.0.1:8700/healthz || exit 1

CMD ["python", "-m", "forecastd"]
