#!/bin/sh
# Inject query latency by creating a function that wraps pg_sleep
#
# Environment variables:
#   TUMULT_PG_HOST     - PostgreSQL host (default: localhost)
#   TUMULT_PG_PORT     - PostgreSQL port (default: 5432)
#   TUMULT_PG_USER     - PostgreSQL user (default: postgres)
#   TUMULT_PG_DATABASE - Target database (required)
#   TUMULT_LATENCY_MS  - Latency to inject in milliseconds (default: 100)
#   TUMULT_PG_PASSWORD - Password (optional)
set -e

HOST="${TUMULT_PG_HOST:-localhost}"
PORT="${TUMULT_PG_PORT:-5432}"
USER="${TUMULT_PG_USER:-postgres}"
DATABASE="${TUMULT_PG_DATABASE:?TUMULT_PG_DATABASE is required}"
LATENCY_MS="${TUMULT_LATENCY_MS:-100}"

if ! command -v psql >/dev/null 2>&1; then
    echo "error: psql not found" >&2
    exit 1
fi

export PGPASSWORD="${TUMULT_PG_PASSWORD:-}"

LATENCY_S=$(awk "BEGIN { printf \"%.3f\", ${LATENCY_MS} / 1000 }")

echo "injecting ${LATENCY_MS}ms latency via pg_sleep in ${DATABASE}"
psql -h "${HOST}" -p "${PORT}" -U "${USER}" -d "${DATABASE}" -c \
    "SELECT pg_sleep(${LATENCY_S});"
echo "latency injection completed"
