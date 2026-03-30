#!/bin/sh
# Gracefully stop a container
#
# Environment variables:
#   TUMULT_CONTAINER_ID   - Container ID or name (required)
#   TUMULT_RUNTIME        - Container runtime: docker or podman (default: docker)
#   TUMULT_TIMEOUT        - Grace period in seconds (default: 10)
set -eu

CONTAINER="${TUMULT_CONTAINER_ID:?TUMULT_CONTAINER_ID is required}"
RUNTIME="${TUMULT_RUNTIME:-docker}"

case "${RUNTIME}" in docker|podman) ;; *) echo "error: TUMULT_RUNTIME must be docker or podman, got: ${RUNTIME}" >&2; exit 1;; esac
TIMEOUT="${TUMULT_TIMEOUT:-10}"

if ! command -v "${RUNTIME}" >/dev/null 2>&1; then
    echo "error: ${RUNTIME} not found" >&2
    exit 1
fi

echo "stopping container ${CONTAINER} with timeout ${TIMEOUT}s via ${RUNTIME}"
"${RUNTIME}" stop --time "${TIMEOUT}" "${CONTAINER}"
echo "container ${CONTAINER} stopped"
