#!/bin/sh
# tumult-pumba: Add packet loss to container egress traffic.
#
# Outputs structured JSON for OTel span enrichment and DuckDB analytics.
#
# Environment variables:
#   TUMULT_CONTAINER    — target container name or ID (required)
#   TUMULT_LOSS_PCT     — packet loss percentage 0-100 (default: 10)
#   TUMULT_CORRELATION  — correlation percentage 0-100 (default: 25)
#   TUMULT_DURATION     — chaos duration e.g. "30s", "5m" (default: 30s)
#   TUMULT_INTERFACE    — network interface inside container (default: eth0)
#   TUMULT_TARGET_IP    — optional: only affect traffic to this IP
#   TUMULT_PUMBA_IMAGE  — Pumba Docker image (default: ghcr.io/alexei-led/pumba:latest)

set -eu

. "$(dirname "$0")/../../lib/validate.sh"

CONTAINER="${TUMULT_CONTAINER:?error: TUMULT_CONTAINER is required}"
LOSS_PCT="${TUMULT_LOSS_PCT:-10}"
CORRELATION="${TUMULT_CORRELATION:-25}"
DURATION="${TUMULT_DURATION:-30s}"
INTERFACE="${TUMULT_INTERFACE:-eth0}"
TARGET_IP="${TUMULT_TARGET_IP:-}"
PUMBA_IMAGE="${TUMULT_PUMBA_IMAGE:-ghcr.io/alexei-led/pumba:latest}"

validate_number "TUMULT_LOSS_PCT" "$LOSS_PCT"
validate_number "TUMULT_CORRELATION" "$CORRELATION"

PUMBA_ARGS="-d ${DURATION} netem --interface ${INTERFACE}"

if [ -n "$TARGET_IP" ]; then
    PUMBA_ARGS="${PUMBA_ARGS} --target ${TARGET_IP}"
fi

PUMBA_ARGS="${PUMBA_ARGS} loss -p ${LOSS_PCT} -c ${CORRELATION}"

if command -v pumba >/dev/null 2>&1; then
    pumba ${PUMBA_ARGS} "${CONTAINER}" >/dev/null 2>&1
else
    DOCKER_SOCK="${DOCKER_HOST:-unix:///var/run/docker.sock}"
    SOCK_PATH="$(echo "$DOCKER_SOCK" | sed 's|unix://||')"
    docker run --rm -v "${SOCK_PATH}:${SOCK_PATH}" -e "DOCKER_HOST=${DOCKER_SOCK}" \
        "${PUMBA_IMAGE}" ${PUMBA_ARGS} "${CONTAINER}" >/dev/null 2>&1
fi

TARGET_IP_JSON="null"
[ -n "$TARGET_IP" ] && TARGET_IP_JSON="\"${TARGET_IP}\""

cat <<EOF
{"chaos.tool":"pumba","chaos.type":"netem","chaos.action":"loss","chaos.container":"${CONTAINER}","chaos.interface":"${INTERFACE}","chaos.duration":"${DURATION}","chaos.target_ip":${TARGET_IP_JSON},"netem.loss_pct":${LOSS_PCT},"netem.correlation_pct":${CORRELATION},"traceparent":"${TRACEPARENT:-}"}
EOF
