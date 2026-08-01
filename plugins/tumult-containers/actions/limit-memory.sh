#!/bin/sh
# Inject memory resource limit on a running container
#
# Environment variables:
#   TUMULT_CONTAINER_ID   - Container ID or name (required)
#   TUMULT_RUNTIME        - Container runtime: docker or podman (default: docker)
#   TUMULT_MEMORY_LIMIT   - Memory limit (default: 128m)
set -eu

. "$(dirname "$0")/../../lib/validate.sh"

CONTAINER="${TUMULT_CONTAINER_ID:?TUMULT_CONTAINER_ID is required}"
RUNTIME="${TUMULT_RUNTIME:-docker}"
MEMORY_LIMIT="${TUMULT_MEMORY_LIMIT:-128m}"

validate_enum "TUMULT_RUNTIME" "${RUNTIME}" "docker podman"

# Container names/IDs: letters, digits, '_', '.', '-' (docker naming rules).
case "${CONTAINER}" in
    ''|*[!a-zA-Z0-9_.-]*)
        echo "error: TUMULT_CONTAINER_ID contains invalid characters: '${CONTAINER}'" >&2
        echo "  allowed: letters, digits, '_', '.', '-'" >&2
        exit 1
        ;;
esac

# Memory limit: digits with an optional unit suffix (e.g. 128m, 1g).
case "${MEMORY_LIMIT}" in
    ''|*[!0-9bkmgBKMG]*)
        echo "error: TUMULT_MEMORY_LIMIT contains invalid characters: '${MEMORY_LIMIT}'" >&2
        echo "  allowed: digits, 'b', 'k', 'm', 'g'" >&2
        exit 1
        ;;
esac

if ! command -v "${RUNTIME}" >/dev/null 2>&1; then
    echo "error: ${RUNTIME} not found" >&2
    exit 1
fi

echo "limiting memory for ${CONTAINER}: limit=${MEMORY_LIMIT} via ${RUNTIME}"
"${RUNTIME}" update --memory "${MEMORY_LIMIT}" "${CONTAINER}"
echo "memory limit applied to ${CONTAINER}"
