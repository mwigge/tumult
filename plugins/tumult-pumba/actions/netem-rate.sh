#!/bin/sh
# tumult-pumba: Limit bandwidth on container egress.
#
# Environment variables:
#   TUMULT_CONTAINER   — target container name or ID (required)
#   TUMULT_RATE        — bandwidth limit e.g. "100kbit", "1mbit" (default: 100kbit)
#   TUMULT_DURATION    — chaos duration (default: 30s)
#   TUMULT_INTERFACE   — network interface inside container (default: eth0)
#   TUMULT_PUMBA_IMAGE — Pumba image (default: ghcr.io/alexei-led/pumba:latest)

set -eu

CONTAINER="${TUMULT_CONTAINER:?error: TUMULT_CONTAINER is required}"
RATE="${TUMULT_RATE:-100kbit}"
DURATION="${TUMULT_DURATION:-30s}"
INTERFACE="${TUMULT_INTERFACE:-eth0}"
PUMBA_IMAGE="${TUMULT_PUMBA_IMAGE:-ghcr.io/alexei-led/pumba:latest}"

PUMBA_ARGS="-d ${DURATION} netem --interface ${INTERFACE} rate -r ${RATE}"

if command -v pumba >/dev/null 2>&1; then
    pumba ${PUMBA_ARGS} "${CONTAINER}" >/dev/null 2>&1
else
    DOCKER_SOCK="${DOCKER_HOST:-unix:///var/run/docker.sock}"
    SOCK_PATH="$(echo "$DOCKER_SOCK" | sed 's|unix://||')"
    docker run --rm -v "${SOCK_PATH}:${SOCK_PATH}" -e "DOCKER_HOST=${DOCKER_SOCK}" \
        "${PUMBA_IMAGE}" ${PUMBA_ARGS} "${CONTAINER}" >/dev/null 2>&1
fi

cat <<EOF
{"chaos.tool":"pumba","chaos.type":"netem","chaos.action":"rate","chaos.container":"${CONTAINER}","chaos.interface":"${INTERFACE}","chaos.duration":"${DURATION}","netem.rate":"${RATE}","traceparent":"${TRACEPARENT:-}"}
EOF
