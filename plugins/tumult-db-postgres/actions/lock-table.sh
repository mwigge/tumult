#!/bin/sh
# Acquire exclusive lock on a PostgreSQL table for a duration
#
# Environment variables:
#   TUMULT_PG_HOST     - PostgreSQL host (default: localhost)
#   TUMULT_PG_PORT     - PostgreSQL port (default: 5432)
#   TUMULT_PG_USER     - PostgreSQL user (default: postgres)
#   TUMULT_PG_DATABASE - Target database (required)
#   TUMULT_PG_TABLE    - Table to lock (required)
#   TUMULT_DURATION    - Lock duration in seconds (default: 10)
#   TUMULT_PG_PASSWORD - Password (optional)
set -e

HOST="${TUMULT_PG_HOST:-localhost}"
PORT="${TUMULT_PG_PORT:-5432}"
USER="${TUMULT_PG_USER:-postgres}"
DATABASE="${TUMULT_PG_DATABASE:?TUMULT_PG_DATABASE is required}"
TABLE="${TUMULT_PG_TABLE:?TUMULT_PG_TABLE is required}"
DURATION="${TUMULT_DURATION:-10}"

if ! command -v psql >/dev/null 2>&1; then
    echo "error: psql not found" >&2
    exit 1
fi

export PGPASSWORD="${TUMULT_PG_PASSWORD:-}"

echo "locking table ${TABLE} in ${DATABASE} for ${DURATION}s"
psql -h "${HOST}" -p "${PORT}" -U "${USER}" -d "${DATABASE}" -c \
    "BEGIN; LOCK TABLE ${TABLE} IN ACCESS EXCLUSIVE MODE; SELECT pg_sleep(${DURATION}); COMMIT;"
echo "lock released on ${TABLE}"
