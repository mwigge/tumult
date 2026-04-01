#!/bin/sh
# tumult-pumba: Pause container processes for a duration.
#
# Pumba pauses the container and automatically unpauses after duration.
#
# Environment variables:
#   TUMULT_CONTAINER   — target container name or ID (required)
#   TUMULT_DURATION    — pause duration e.g. "10s", "1m" (default: 10s)
#   TUMULT_PUMBA_IMAGE — Pumba image (default: ghcr.io/alexei-led/pumba:latest)

set -eu

CONTAINER="${TUMULT_CONTAINER:?error: TUMULT_CONTAINER is required}"
DURATION="${TUMULT_DURATION:-10s}"
PUMBA_IMAGE="${TUMULT_PUMBA_IMAGE:-ghcr.io/alexei-led/pumba:latest}"

PUMBA_ARGS="pause -d ${DURATION}"

if command -v pumba >/dev/null 2>&1; then
    pumba ${PUMBA_ARGS} "${CONTAINER}" >/dev/null 2>&1
else
    DOCKER_SOCK="${DOCKER_HOST:-unix:///var/run/docker.sock}"
    SOCK_PATH="$(echo "$DOCKER_SOCK" | sed 's|unix://||')"
    docker run --rm -v "${SOCK_PATH}:${SOCK_PATH}" -e "DOCKER_HOST=${DOCKER_SOCK}" \
        "${PUMBA_IMAGE}" ${PUMBA_ARGS} "${CONTAINER}" >/dev/null 2>&1
fi

cat <<EOF
{"chaos.tool":"pumba","chaos.type":"container","chaos.action":"pause","chaos.container":"${CONTAINER}","chaos.duration":"${DURATION}","traceparent":"${TRACEPARENT:-}"}
EOF
