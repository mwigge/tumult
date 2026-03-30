#!/bin/sh
# Inject query latency via pg_sleep
set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "${SCRIPT_DIR}/../../lib/validate.sh"

HOST="${TUMULT_PG_HOST:-localhost}"
PORT="${TUMULT_PG_PORT:-5432}"
USER="${TUMULT_PG_USER:-postgres}"
DATABASE="${TUMULT_PG_DATABASE:?TUMULT_PG_DATABASE is required}"
LATENCY_MS="${TUMULT_LATENCY_MS:-100}"

validate_identifier "TUMULT_PG_DATABASE" "${DATABASE}"
validate_number "TUMULT_LATENCY_MS" "${LATENCY_MS}"

if ! command -v psql >/dev/null 2>&1; then
    echo "error: psql not found" >&2
    exit 1
fi

# Use .pgpass file to avoid /proc/environ credential exposure (DB-04)
PGPASS_FILE=$(mktemp)
trap "rm -f ${PGPASS_FILE}" EXIT INT TERM
echo "*:*:*:*:${TUMULT_PG_PASSWORD:-}" > "${PGPASS_FILE}"
chmod 600 "${PGPASS_FILE}"
export PGPASSFILE="${PGPASS_FILE}"

LATENCY_S=$(awk -v ms="${LATENCY_MS}" 'BEGIN { printf "%.3f", ms / 1000 }')

echo "injecting ${LATENCY_MS}ms latency via pg_sleep in ${DATABASE}"
psql -h "${HOST}" -p "${PORT}" -U "${USER}" -d "${DATABASE}" -c \
    "SELECT pg_sleep(${LATENCY_S});"
echo "latency injection completed"
