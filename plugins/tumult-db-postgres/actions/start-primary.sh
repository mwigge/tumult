#!/bin/sh
# Rollback of stop-primary: start the container of the PostgreSQL primary
# again. Prints JSON: the container and the time the runtime started it in
# epoch seconds. Run it only when no standby was promoted: else two
# primaries would take writes.
#
# Environment variables:
#   TUMULT_CONTAINER_ID - The container of the primary (required)
#   TUMULT_RUNTIME      - Container runtime: docker or podman (default: docker)
set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "${SCRIPT_DIR}/../../lib/validate.sh"
. "${SCRIPT_DIR}/../lib/postgres.sh"

CONTAINER="${TUMULT_CONTAINER_ID:?TUMULT_CONTAINER_ID is required}"
RUNTIME="${TUMULT_RUNTIME:-docker}"

validate_enum "TUMULT_RUNTIME" "${RUNTIME}" "docker podman"

# Container names/IDs: letters, digits, '_', '.', '-' (docker naming rules).
case "${CONTAINER}" in
    ''|*[!a-zA-Z0-9_.-]*)
        echo "error: TUMULT_CONTAINER_ID contains invalid characters: '${CONTAINER}'" >&2
        echo "  allowed: letters, digits, '_', '.', '-'" >&2
        exit 1
        ;;
esac

need_tool "${RUNTIME}"

"${RUNTIME}" start "${CONTAINER}" >/dev/null

printf '{"container":"%s","started_at":%s}\n' "${CONTAINER}" "$(epoch_now)"
