#!/bin/sh
# tumult-pumba: Kill container with configurable signal and timing.
#
# Environment variables:
#   TUMULT_CONTAINER   — target container name or regex (required)
#   TUMULT_SIGNAL      — kill signal (default: SIGKILL)
#   TUMULT_INTERVAL    — repeat interval e.g. "10s" (default: empty = once)
#   TUMULT_RANDOM      — if "true", pick random matching container (default: false)
#   TUMULT_PUMBA_IMAGE — Pumba image (default: ghcr.io/alexei-led/pumba:latest)

set -eu

CONTAINER="${TUMULT_CONTAINER:?error: TUMULT_CONTAINER is required}"
SIGNAL="${TUMULT_SIGNAL:-SIGKILL}"
INTERVAL="${TUMULT_INTERVAL:-}"
RANDOM_FLAG="${TUMULT_RANDOM:-false}"
PUMBA_IMAGE="${TUMULT_PUMBA_IMAGE:-ghcr.io/alexei-led/pumba:latest}"

PUMBA_ARGS=""
if [ -n "$INTERVAL" ]; then
    PUMBA_ARGS="${PUMBA_ARGS} --interval ${INTERVAL}"
fi
if [ "$RANDOM_FLAG" = "true" ]; then
    PUMBA_ARGS="${PUMBA_ARGS} --random"
fi
PUMBA_ARGS="${PUMBA_ARGS} kill -s ${SIGNAL}"

if command -v pumba >/dev/null 2>&1; then
    pumba ${PUMBA_ARGS} "${CONTAINER}" >/dev/null 2>&1
else
    DOCKER_SOCK="${DOCKER_HOST:-unix:///var/run/docker.sock}"
    SOCK_PATH="$(echo "$DOCKER_SOCK" | sed 's|unix://||')"
    docker run --rm -v "${SOCK_PATH}:${SOCK_PATH}" -e "DOCKER_HOST=${DOCKER_SOCK}" \
        "${PUMBA_IMAGE}" ${PUMBA_ARGS} "${CONTAINER}" >/dev/null 2>&1
fi

cat <<EOF
{"chaos.tool":"pumba","chaos.type":"container","chaos.action":"kill","chaos.container":"${CONTAINER}","chaos.signal":"${SIGNAL}","chaos.random":${RANDOM_FLAG},"traceparent":"${TRACEPARENT:-}"}
EOF
