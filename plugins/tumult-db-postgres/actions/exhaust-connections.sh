#!/bin/sh
# Exhaust PostgreSQL connection pool by opening idle connections
set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "${SCRIPT_DIR}/../../lib/validate.sh"

HOST="${TUMULT_PG_HOST:-localhost}"
PORT="${TUMULT_PG_PORT:-5432}"
USER="${TUMULT_PG_USER:-postgres}"
DATABASE="${TUMULT_PG_DATABASE:?TUMULT_PG_DATABASE is required}"
COUNT="${TUMULT_CONNECTION_COUNT:-50}"
DURATION="${TUMULT_DURATION:-30}"

validate_identifier "TUMULT_PG_DATABASE" "${DATABASE}"
validate_integer "TUMULT_CONNECTION_COUNT" "${COUNT}"
validate_integer "TUMULT_DURATION" "${DURATION}"

# Safety cap: max 500 connections to prevent fork-bombing the host
if [ "${COUNT}" -gt 500 ]; then
    echo "error: TUMULT_CONNECTION_COUNT capped at 500, got: ${COUNT}" >&2
    exit 1
fi

if ! command -v psql >/dev/null 2>&1; then
    echo "error: psql not found" >&2
    exit 1
fi

# Use .pgpass file to avoid /proc/environ exposure
PGPASS_FILE=$(mktemp)
echo "*:*:*:*:${TUMULT_PG_PASSWORD:-}" > "${PGPASS_FILE}"
chmod 600 "${PGPASS_FILE}"
export PGPASSFILE="${PGPASS_FILE}"

echo "opening ${COUNT} idle connections to ${DATABASE} for ${DURATION}s"

PIDS=""
# Trap to clean up background processes and pgpass file on signal/exit
cleanup() {
    for PID in ${PIDS}; do
        kill "${PID}" 2>/dev/null || true
    done
    rm -f "${PGPASS_FILE}"
}
trap cleanup EXIT INT TERM

i=0
while [ "$i" -lt "${COUNT}" ]; do
    psql -h "${HOST}" -p "${PORT}" -U "${USER}" -d "${DATABASE}" -c \
        "SELECT pg_sleep(${DURATION});" >/dev/null 2>&1 &
    PIDS="${PIDS} $!"
    i=$((i + 1))
done

echo "${COUNT} connections opened, holding for ${DURATION}s"

for PID in ${PIDS}; do
    wait "${PID}" 2>/dev/null || true
done

echo "all connections released"
