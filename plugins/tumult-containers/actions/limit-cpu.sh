#!/bin/sh
# Inject CPU resource limit on a running container
#
# Environment variables:
#   TUMULT_CONTAINER_ID   - Container ID or name (required)
#   TUMULT_RUNTIME        - Container runtime: docker or podman (default: docker)
#   TUMULT_CPU_QUOTA      - CPU quota in microseconds per period (default: 50000 = 50%)
#   TUMULT_CPU_PERIOD     - CPU period in microseconds (default: 100000)
set -e

CONTAINER="${TUMULT_CONTAINER_ID:?TUMULT_CONTAINER_ID is required}"
RUNTIME="${TUMULT_RUNTIME:-docker}"
CPU_QUOTA="${TUMULT_CPU_QUOTA:-50000}"
CPU_PERIOD="${TUMULT_CPU_PERIOD:-100000}"

if ! command -v "${RUNTIME}" >/dev/null 2>&1; then
    echo "error: ${RUNTIME} not found" >&2
    exit 1
fi

echo "limiting CPU for ${CONTAINER}: quota=${CPU_QUOTA}/${CPU_PERIOD} via ${RUNTIME}"
"${RUNTIME}" update --cpu-quota "${CPU_QUOTA}" --cpu-period "${CPU_PERIOD}" "${CONTAINER}"
echo "cpu limit applied to ${CONTAINER}"
