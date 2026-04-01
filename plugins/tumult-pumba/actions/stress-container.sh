#!/bin/sh
# tumult-pumba: CPU/memory/IO stress injection inside container.
#
# Uses Pumba's stress command (backed by stress-ng inside the container).
#
# Environment variables:
#   TUMULT_CONTAINER    — target container name or ID (required)
#   TUMULT_DURATION     — stress duration (default: 30s)
#   TUMULT_STRESS_TYPE  — one of: cpu, memory, io, all (default: cpu)
#   TUMULT_CPU_WORKERS  — number of CPU stress workers (default: 1)
#   TUMULT_MEM_WORKERS  — number of memory stress workers (default: 1)
#   TUMULT_IO_WORKERS   — number of IO stress workers (default: 1)
#   TUMULT_PUMBA_IMAGE  — Pumba image (default: ghcr.io/alexei-led/pumba:latest)

set -eu

. "$(dirname "$0")/../../lib/validate.sh"

CONTAINER="${TUMULT_CONTAINER:?error: TUMULT_CONTAINER is required}"
DURATION="${TUMULT_DURATION:-30s}"
STRESS_TYPE="${TUMULT_STRESS_TYPE:-cpu}"
CPU_WORKERS="${TUMULT_CPU_WORKERS:-1}"
MEM_WORKERS="${TUMULT_MEM_WORKERS:-1}"
IO_WORKERS="${TUMULT_IO_WORKERS:-1}"
PUMBA_IMAGE="${TUMULT_PUMBA_IMAGE:-ghcr.io/alexei-led/pumba:latest}"

validate_enum "TUMULT_STRESS_TYPE" "$STRESS_TYPE" "cpu memory io all"
validate_integer "TUMULT_CPU_WORKERS" "$CPU_WORKERS"

STRESS_ARGS=""
case "$STRESS_TYPE" in
    cpu)  STRESS_ARGS="--stress-cpu ${CPU_WORKERS}" ;;
    memory) STRESS_ARGS="--stress-vm ${MEM_WORKERS}" ;;
    io) STRESS_ARGS="--stress-io ${IO_WORKERS}" ;;
    all) STRESS_ARGS="--stress-cpu ${CPU_WORKERS} --stress-vm ${MEM_WORKERS} --stress-io ${IO_WORKERS}" ;;
esac

PUMBA_ARGS="stress -d ${DURATION} ${STRESS_ARGS}"

if command -v pumba >/dev/null 2>&1; then
    pumba ${PUMBA_ARGS} "${CONTAINER}" >/dev/null 2>&1
else
    DOCKER_SOCK="${DOCKER_HOST:-unix:///var/run/docker.sock}"
    SOCK_PATH="$(echo "$DOCKER_SOCK" | sed 's|unix://||')"
    docker run --rm -v "${SOCK_PATH}:${SOCK_PATH}" -e "DOCKER_HOST=${DOCKER_SOCK}" \
        "${PUMBA_IMAGE}" ${PUMBA_ARGS} "${CONTAINER}" >/dev/null 2>&1
fi

cat <<EOF
{"chaos.tool":"pumba","chaos.type":"stress","chaos.action":"${STRESS_TYPE}","chaos.container":"${CONTAINER}","chaos.duration":"${DURATION}","stress.cpu_workers":${CPU_WORKERS},"stress.mem_workers":${MEM_WORKERS},"stress.io_workers":${IO_WORKERS},"traceparent":"${TRACEPARENT:-}"}
EOF
