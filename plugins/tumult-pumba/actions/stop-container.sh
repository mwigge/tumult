#!/bin/sh
# tumult-pumba: Stop container with grace period and optional restart.
#
# Environment variables:
#   TUMULT_CONTAINER   — target container name or ID (required)
#   TUMULT_DURATION    — stop duration before restart e.g. "30s" (default: 30s)
#   TUMULT_RESTART     — restart after stop: "true"/"false" (default: false)
#   TUMULT_PUMBA_IMAGE — Pumba image (default: ghcr.io/alexei-led/pumba:1.6.0, pinned by digest)

set -eu

CONTAINER="${TUMULT_CONTAINER:?error: TUMULT_CONTAINER is required}"
DURATION="${TUMULT_DURATION:-30s}"
RESTART="${TUMULT_RESTART:-false}"
PUMBA_IMAGE="${TUMULT_PUMBA_IMAGE:-ghcr.io/alexei-led/pumba:1.6.0@sha256:11ed15538c7ec688628e2f53917d949e8c16418c146742731c3030ef276b6057}"

if [ "$RESTART" = "true" ]; then
    PUMBA_ARGS="-d ${DURATION} restart"
else
    PUMBA_ARGS="stop -d ${DURATION}"
fi

if command -v pumba >/dev/null 2>&1; then
    pumba ${PUMBA_ARGS} "${CONTAINER}" >/dev/null 2>&1
else
    DOCKER_SOCK="${DOCKER_HOST:-unix:///var/run/docker.sock}"
    SOCK_PATH="$(echo "$DOCKER_SOCK" | sed 's|unix://||')"
    docker run --rm -v "${SOCK_PATH}:${SOCK_PATH}" -e "DOCKER_HOST=${DOCKER_SOCK}" \
        "${PUMBA_IMAGE}" ${PUMBA_ARGS} "${CONTAINER}" >/dev/null 2>&1
fi

cat <<EOF
{"chaos.tool":"pumba","chaos.type":"container","chaos.action":"stop","chaos.container":"${CONTAINER}","chaos.duration":"${DURATION}","chaos.restart":${RESTART},"traceparent":"${TRACEPARENT:-}"}
EOF
