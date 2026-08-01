#!/bin/sh
# Gracefully stop a container
#
# Environment variables:
#   TUMULT_CONTAINER_ID   - Container ID or name (required)
#   TUMULT_RUNTIME        - Container runtime: docker or podman (default: docker)
#   TUMULT_TIMEOUT        - Grace period in seconds (default: 10)
set -eu

. "$(dirname "$0")/../../lib/validate.sh"

CONTAINER="${TUMULT_CONTAINER_ID:?TUMULT_CONTAINER_ID is required}"
RUNTIME="${TUMULT_RUNTIME:-docker}"
TIMEOUT="${TUMULT_TIMEOUT:-10}"

validate_enum "TUMULT_RUNTIME" "${RUNTIME}" "docker podman"
validate_integer "TUMULT_TIMEOUT" "${TIMEOUT}"

# Container names/IDs: letters, digits, '_', '.', '-' (docker naming rules).
case "${CONTAINER}" in
    ''|*[!a-zA-Z0-9_.-]*)
        echo "error: TUMULT_CONTAINER_ID contains invalid characters: '${CONTAINER}'" >&2
        echo "  allowed: letters, digits, '_', '.', '-'" >&2
        exit 1
        ;;
esac

if ! command -v "${RUNTIME}" >/dev/null 2>&1; then
    echo "error: ${RUNTIME} not found" >&2
    exit 1
fi

echo "stopping container ${CONTAINER} with timeout ${TIMEOUT}s via ${RUNTIME}"
"${RUNTIME}" stop --time "${TIMEOUT}" "${CONTAINER}"
echo "container ${CONTAINER} stopped"
