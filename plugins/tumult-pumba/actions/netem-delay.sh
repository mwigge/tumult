#!/bin/sh
# tumult-pumba: Add network latency to container egress traffic.
#
# Uses Pumba's netem delay command to inject latency into a container's
# network namespace. Works on any platform where Docker runs — no root
# or Linux kernel access required on the host.
#
# Outputs structured JSON for OTel span enrichment and DuckDB analytics.
#
# Environment variables:
#   TUMULT_CONTAINER    — target container name or ID (required)
#   TUMULT_DELAY_MS     — latency to add in milliseconds (default: 100)
#   TUMULT_JITTER_MS    — jitter in milliseconds (default: 10)
#   TUMULT_CORRELATION  — correlation percentage 0-100 (default: 20)
#   TUMULT_DURATION     — chaos duration e.g. "30s", "5m" (default: 30s)
#   TUMULT_INTERFACE    — network interface inside container (default: eth0)
#   TUMULT_TARGET_IP    — optional: only affect traffic to this IP
#   TUMULT_PUMBA_IMAGE  — Pumba Docker image (default: ghcr.io/alexei-led/pumba:latest)

set -eu

. "$(dirname "$0")/../../lib/validate.sh"

CONTAINER="${TUMULT_CONTAINER:?error: TUMULT_CONTAINER is required}"
DELAY_MS="${TUMULT_DELAY_MS:-100}"
JITTER_MS="${TUMULT_JITTER_MS:-10}"
CORRELATION="${TUMULT_CORRELATION:-20}"
DURATION="${TUMULT_DURATION:-30s}"
INTERFACE="${TUMULT_INTERFACE:-eth0}"
TARGET_IP="${TUMULT_TARGET_IP:-}"
PUMBA_IMAGE="${TUMULT_PUMBA_IMAGE:-ghcr.io/alexei-led/pumba:latest}"

validate_number "TUMULT_DELAY_MS" "$DELAY_MS"
validate_number "TUMULT_JITTER_MS" "$JITTER_MS"
validate_number "TUMULT_CORRELATION" "$CORRELATION"

# Build Pumba command
PUMBA_ARGS="-d ${DURATION} netem --interface ${INTERFACE}"

if [ -n "$TARGET_IP" ]; then
    PUMBA_ARGS="${PUMBA_ARGS} --target ${TARGET_IP}"
fi

PUMBA_ARGS="${PUMBA_ARGS} delay -t ${DELAY_MS} -j ${JITTER_MS} -c ${CORRELATION}"

# Execute Pumba — local binary or Docker container
if command -v pumba >/dev/null 2>&1; then
    pumba ${PUMBA_ARGS} "${CONTAINER}" >/dev/null 2>&1
else
    DOCKER_SOCK="${DOCKER_HOST:-unix:///var/run/docker.sock}"
    SOCK_PATH="$(echo "$DOCKER_SOCK" | sed 's|unix://||')"

    docker run --rm \
        -v "${SOCK_PATH}:${SOCK_PATH}" \
        -e "DOCKER_HOST=${DOCKER_SOCK}" \
        "${PUMBA_IMAGE}" \
        ${PUMBA_ARGS} "${CONTAINER}" >/dev/null 2>&1
fi

# Structured JSON output for OTel enrichment + DuckDB analytics
# Fields follow the resilience.* namespace convention
TARGET_IP_JSON="null"
[ -n "$TARGET_IP" ] && TARGET_IP_JSON="\"${TARGET_IP}\""

cat <<EOF
{"chaos.tool":"pumba","chaos.type":"netem","chaos.action":"delay","chaos.container":"${CONTAINER}","chaos.interface":"${INTERFACE}","chaos.duration":"${DURATION}","chaos.target_ip":${TARGET_IP_JSON},"netem.delay_ms":${DELAY_MS},"netem.jitter_ms":${JITTER_MS},"netem.correlation_pct":${CORRELATION},"traceparent":"${TRACEPARENT:-}"}
EOF
