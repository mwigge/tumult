#!/bin/sh
# Kill a running container (SIGKILL)
#
# Environment variables:
#   TUMULT_CONTAINER_ID   - Container ID or name (required)
#   TUMULT_RUNTIME        - Container runtime: docker or podman (default: docker)
#   TUMULT_SIGNAL         - Signal to send (default: KILL)
set -e

CONTAINER="${TUMULT_CONTAINER_ID:?TUMULT_CONTAINER_ID is required}"
RUNTIME="${TUMULT_RUNTIME:-docker}"
SIGNAL="${TUMULT_SIGNAL:-KILL}"

if ! command -v "${RUNTIME}" >/dev/null 2>&1; then
    echo "error: ${RUNTIME} not found" >&2
    exit 1
fi

echo "killing container ${CONTAINER} with signal ${SIGNAL} via ${RUNTIME}"
"${RUNTIME}" kill --signal "${SIGNAL}" "${CONTAINER}"
echo "container ${CONTAINER} killed"
