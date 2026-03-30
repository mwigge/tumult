#!/bin/sh
# Unpause a paused container
#
# Environment variables:
#   TUMULT_CONTAINER_ID   - Container ID or name (required)
#   TUMULT_RUNTIME        - Container runtime: docker or podman (default: docker)
set -e

CONTAINER="${TUMULT_CONTAINER_ID:?TUMULT_CONTAINER_ID is required}"
RUNTIME="${TUMULT_RUNTIME:-docker}"

if ! command -v "${RUNTIME}" >/dev/null 2>&1; then
    echo "error: ${RUNTIME} not found" >&2
    exit 1
fi

echo "unpausing container ${CONTAINER} via ${RUNTIME}"
"${RUNTIME}" unpause "${CONTAINER}"
echo "container ${CONTAINER} unpaused"
