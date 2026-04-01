#!/bin/sh
# tumult-pumba: Duplicate packets on container egress.
#
# Environment variables:
#   TUMULT_CONTAINER     — target container name or ID (required)
#   TUMULT_DUPLICATE_PCT — duplication percentage 0-100 (default: 5)
#   TUMULT_CORRELATION   — correlation percentage 0-100 (default: 0)
#   TUMULT_DURATION      — chaos duration (default: 30s)
#   TUMULT_INTERFACE     — network interface inside container (default: eth0)
#   TUMULT_PUMBA_IMAGE   — Pumba image (default: ghcr.io/alexei-led/pumba:latest)

set -eu

. "$(dirname "$0")/../../lib/validate.sh"

CONTAINER="${TUMULT_CONTAINER:?error: TUMULT_CONTAINER is required}"
DUP_PCT="${TUMULT_DUPLICATE_PCT:-5}"
CORRELATION="${TUMULT_CORRELATION:-0}"
DURATION="${TUMULT_DURATION:-30s}"
INTERFACE="${TUMULT_INTERFACE:-eth0}"
PUMBA_IMAGE="${TUMULT_PUMBA_IMAGE:-ghcr.io/alexei-led/pumba:latest}"

validate_number "TUMULT_DUPLICATE_PCT" "$DUP_PCT"

PUMBA_ARGS="-d ${DURATION} netem --interface ${INTERFACE} duplicate -p ${DUP_PCT} -c ${CORRELATION}"

if command -v pumba >/dev/null 2>&1; then
    pumba ${PUMBA_ARGS} "${CONTAINER}" >/dev/null 2>&1
else
    DOCKER_SOCK="${DOCKER_HOST:-unix:///var/run/docker.sock}"
    SOCK_PATH="$(echo "$DOCKER_SOCK" | sed 's|unix://||')"
    docker run --rm -v "${SOCK_PATH}:${SOCK_PATH}" -e "DOCKER_HOST=${DOCKER_SOCK}" \
        "${PUMBA_IMAGE}" ${PUMBA_ARGS} "${CONTAINER}" >/dev/null 2>&1
fi

cat <<EOF
{"chaos.tool":"pumba","chaos.type":"netem","chaos.action":"duplicate","chaos.container":"${CONTAINER}","chaos.interface":"${INTERFACE}","chaos.duration":"${DURATION}","netem.duplicate_pct":${DUP_PCT},"netem.correlation_pct":${CORRELATION},"traceparent":"${TRACEPARENT:-}"}
EOF
