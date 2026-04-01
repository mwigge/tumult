#!/bin/sh
# tumult-pumba: Drop incoming packets to container via iptables.
#
# Unlike netem (which affects egress), iptables loss affects ingress.
#
# Environment variables:
#   TUMULT_CONTAINER   — target container name or ID (required)
#   TUMULT_LOSS_PCT    — drop probability percentage 0-100 (default: 10)
#   TUMULT_DURATION    — chaos duration (default: 30s)
#   TUMULT_PUMBA_IMAGE — Pumba image (default: ghcr.io/alexei-led/pumba:latest)

set -eu

. "$(dirname "$0")/../../lib/validate.sh"

CONTAINER="${TUMULT_CONTAINER:?error: TUMULT_CONTAINER is required}"
LOSS_PCT="${TUMULT_LOSS_PCT:-10}"
DURATION="${TUMULT_DURATION:-30s}"
PUMBA_IMAGE="${TUMULT_PUMBA_IMAGE:-ghcr.io/alexei-led/pumba:latest}"

validate_number "TUMULT_LOSS_PCT" "$LOSS_PCT"

PUMBA_ARGS="iptables -d ${DURATION} loss --probability ${LOSS_PCT}"

if command -v pumba >/dev/null 2>&1; then
    pumba ${PUMBA_ARGS} "${CONTAINER}" >/dev/null 2>&1
else
    DOCKER_SOCK="${DOCKER_HOST:-unix:///var/run/docker.sock}"
    SOCK_PATH="$(echo "$DOCKER_SOCK" | sed 's|unix://||')"
    docker run --rm -v "${SOCK_PATH}:${SOCK_PATH}" -e "DOCKER_HOST=${DOCKER_SOCK}" \
        "${PUMBA_IMAGE}" ${PUMBA_ARGS} "${CONTAINER}" >/dev/null 2>&1
fi

cat <<EOF
{"chaos.tool":"pumba","chaos.type":"iptables","chaos.action":"loss","chaos.container":"${CONTAINER}","chaos.duration":"${DURATION}","iptables.loss_pct":${LOSS_PCT},"traceparent":"${TRACEPARENT:-}"}
EOF
