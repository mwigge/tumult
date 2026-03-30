#!/bin/sh
# Terminate active PostgreSQL connections to a database
#
# Environment variables:
#   TUMULT_PG_HOST     - PostgreSQL host (default: localhost)
#   TUMULT_PG_PORT     - PostgreSQL port (default: 5432)
#   TUMULT_PG_USER     - PostgreSQL user (default: postgres)
#   TUMULT_PG_DATABASE - Target database (required)
#   TUMULT_PG_PASSWORD - Password (optional, uses PGPASSWORD)
set -e

HOST="${TUMULT_PG_HOST:-localhost}"
PORT="${TUMULT_PG_PORT:-5432}"
USER="${TUMULT_PG_USER:-postgres}"
DATABASE="${TUMULT_PG_DATABASE:?TUMULT_PG_DATABASE is required}"

if ! command -v psql >/dev/null 2>&1; then
    echo "error: psql not found" >&2
    exit 1
fi

export PGPASSWORD="${TUMULT_PG_PASSWORD:-}"

QUERY="SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '${DATABASE}' AND pid <> pg_backend_pid();"

RESULT=$(psql -h "${HOST}" -p "${PORT}" -U "${USER}" -d postgres -t -c "${QUERY}" 2>&1)
COUNT=$(echo "${RESULT}" | grep -c "t" || echo "0")

echo "terminated ${COUNT} connections to ${DATABASE}"
