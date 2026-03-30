#!/bin/sh
# Inject memory resource limit on a running container
#
# Environment variables:
#   TUMULT_CONTAINER_ID   - Container ID or name (required)
#   TUMULT_RUNTIME        - Container runtime: docker or podman (default: docker)
#   TUMULT_MEMORY_LIMIT   - Memory limit (default: 128m)
set -eu

CONTAINER="${TUMULT_CONTAINER_ID:?TUMULT_CONTAINER_ID is required}"
RUNTIME="${TUMULT_RUNTIME:-docker}"

case "${RUNTIME}" in docker|podman) ;; *) echo "error: TUMULT_RUNTIME must be docker or podman, got: ${RUNTIME}" >&2; exit 1;; esac
MEMORY_LIMIT="${TUMULT_MEMORY_LIMIT:-128m}"

if ! command -v "${RUNTIME}" >/dev/null 2>&1; then
    echo "error: ${RUNTIME} not found" >&2
    exit 1
fi

echo "limiting memory for ${CONTAINER}: limit=${MEMORY_LIMIT} via ${RUNTIME}"
"${RUNTIME}" update --memory "${MEMORY_LIMIT}" "${CONTAINER}"
echo "memory limit applied to ${CONTAINER}"
