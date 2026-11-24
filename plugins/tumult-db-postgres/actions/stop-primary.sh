#!/bin/sh
# Stop the container of a PostgreSQL primary, once the server says it is a
# primary. Prints JSON: the container, the mode and the time it stopped in
# epoch seconds. Rollback: start-primary, but only when no standby was
# promoted: else two primaries would take writes.
#
# Environment variables:
#   TUMULT_PG_HOST      - Host of the primary (default: localhost)
#   TUMULT_PG_PORT      - Port of the primary (default: 5432)
#   TUMULT_PG_USER      - PostgreSQL user (default: postgres)
#   TUMULT_PG_PASSWORD  - Password (optional)
#   TUMULT_CONTAINER_ID - The container of the primary (required)
#   TUMULT_RUNTIME      - Container runtime: docker or podman (default: docker)
#   TUMULT_STOP_MODE    - fast: a fast shutdown, kill: a crash (default: fast)
#   TUMULT_TIMEOUT      - Seconds a fast shutdown may take (default: 10)
set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "${SCRIPT_DIR}/../../lib/validate.sh"
. "${SCRIPT_DIR}/../lib/postgres.sh"

HOST="${TUMULT_PG_HOST:-localhost}"
PORT="${TUMULT_PG_PORT:-5432}"
USER="${TUMULT_PG_USER:-postgres}"
CONTAINER="${TUMULT_CONTAINER_ID:?TUMULT_CONTAINER_ID is required}"
RUNTIME="${TUMULT_RUNTIME:-docker}"
MODE="${TUMULT_STOP_MODE:-fast}"
TIMEOUT="${TUMULT_TIMEOUT:-10}"

validate_enum "TUMULT_RUNTIME" "${RUNTIME}" "docker podman"
validate_enum "TUMULT_STOP_MODE" "${MODE}" "fast kill"
validate_integer "TUMULT_TIMEOUT" "${TIMEOUT}"

# Container names/IDs: letters, digits, '_', '.', '-' (docker naming rules).
case "${CONTAINER}" in
    ''|*[!a-zA-Z0-9_.-]*)
        echo "error: TUMULT_CONTAINER_ID contains invalid characters: '${CONTAINER}'" >&2
        echo "  allowed: letters, digits, '_', '.', '-'" >&2
        exit 1
        ;;
esac

need_tool psql
need_tool "${RUNTIME}"
use_password_file

# A failed query ends the action with the status and the message of psql.
IN_RECOVERY=$(psql -h "${HOST}" -p "${PORT}" -U "${USER}" -w -d postgres -t -A \
    -c "SELECT pg_is_in_recovery();")
case "${IN_RECOVERY}" in
    f) ;;
    t)
        echo "error: the server at ${HOST}:${PORT} is a standby, not a primary" >&2
        exit 1
        ;;
    *)
        echo "error: pg_is_in_recovery() answered '${IN_RECOVERY}', not t or f" >&2
        exit 1
        ;;
esac

# The postgres image stops on SIGINT, a fast shutdown that sends the last
# WAL to the standby. KILL is a crash: the newest commits may not reach it.
case "${MODE}" in
    kill) "${RUNTIME}" kill --signal KILL "${CONTAINER}" >/dev/null ;;
    fast) "${RUNTIME}" stop --time "${TIMEOUT}" "${CONTAINER}" >/dev/null ;;
esac

printf '{"container":"%s","mode":"%s","stopped_at":%s}\n' \
    "${CONTAINER}" "${MODE}" "$(epoch_now)"
