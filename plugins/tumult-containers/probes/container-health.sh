#!/bin/sh
# Probe: check container health check status
# Outputs: "healthy", "unhealthy", "starting", "none", or "not_found"
#
# Environment variables:
#   TUMULT_CONTAINER_ID   - Container ID or name (required)
#   TUMULT_RUNTIME        - Container runtime: docker or podman (default: docker)
set -eu

CONTAINER="${TUMULT_CONTAINER_ID:?TUMULT_CONTAINER_ID is required}"
RUNTIME="${TUMULT_RUNTIME:-docker}"

case "${RUNTIME}" in docker|podman) ;; *) echo "error: TUMULT_RUNTIME must be docker or podman, got: ${RUNTIME}" >&2; exit 1;; esac

if ! command -v "${RUNTIME}" >/dev/null 2>&1; then
    echo "error: ${RUNTIME} not found" >&2
    exit 1
fi

HEALTH=$("${RUNTIME}" inspect --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}none{{end}}' "${CONTAINER}" 2>/dev/null) || {
    echo "not_found"
    exit 0
}

echo "${HEALTH}"
