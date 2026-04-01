#!/bin/sh
# tumult-pumba: Check if target container is running.
#
# Environment variables:
#   TUMULT_CONTAINER — target container name or ID (required)

set -eu

CONTAINER="${TUMULT_CONTAINER:?error: TUMULT_CONTAINER is required}"

STATE=$(docker inspect --format '{{.State.Running}}' "${CONTAINER}" 2>/dev/null) || {
    echo "false"
    exit 0
}

echo "${STATE}"
