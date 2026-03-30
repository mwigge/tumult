#!/bin/sh
# Exhaust PostgreSQL connection pool by opening idle connections
#
# This simulates connection pool exhaustion — a common failure mode
# where the application runs out of available database connections.
#
# Environment variables:
#   TUMULT_PG_HOST         - PostgreSQL host (default: localhost)
#   TUMULT_PG_PORT         - PostgreSQL port (default: 5432)
#   TUMULT_PG_USER         - PostgreSQL user (default: postgres)
#   TUMULT_PG_DATABASE     - Target database (required)
#   TUMULT_PG_PASSWORD     - Password (optional)
#   TUMULT_CONNECTION_COUNT - Number of connections to open (default: 50)
#   TUMULT_DURATION        - Hold connections for this many seconds (default: 30)
set -e

HOST="${TUMULT_PG_HOST:-localhost}"
PORT="${TUMULT_PG_PORT:-5432}"
USER="${TUMULT_PG_USER:-postgres}"
DATABASE="${TUMULT_PG_DATABASE:?TUMULT_PG_DATABASE is required}"
COUNT="${TUMULT_CONNECTION_COUNT:-50}"
DURATION="${TUMULT_DURATION:-30}"

if ! command -v psql >/dev/null 2>&1; then
    echo "error: psql not found" >&2
    exit 1
fi

export PGPASSWORD="${TUMULT_PG_PASSWORD:-}"

echo "opening ${COUNT} idle connections to ${DATABASE} for ${DURATION}s"

PIDS=""
i=0
while [ "$i" -lt "${COUNT}" ]; do
    psql -h "${HOST}" -p "${PORT}" -U "${USER}" -d "${DATABASE}" -c \
        "SELECT pg_sleep(${DURATION});" >/dev/null 2>&1 &
    PIDS="${PIDS} $!"
    i=$((i + 1))
done

echo "${COUNT} connections opened, holding for ${DURATION}s"

# Wait for all background psql processes
for PID in ${PIDS}; do
    wait "${PID}" 2>/dev/null || true
done

echo "all connections released"
