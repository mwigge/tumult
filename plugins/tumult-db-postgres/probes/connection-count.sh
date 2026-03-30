#!/bin/sh
# Probe: count active PostgreSQL connections
# Outputs: integer count
#
# Environment variables:
#   TUMULT_PG_HOST     - PostgreSQL host (default: localhost)
#   TUMULT_PG_PORT     - PostgreSQL port (default: 5432)
#   TUMULT_PG_USER     - PostgreSQL user (default: postgres)
#   TUMULT_PG_DATABASE - Target database (optional, counts all if omitted)
#   TUMULT_PG_PASSWORD - Password (optional)
set -e

HOST="${TUMULT_PG_HOST:-localhost}"
PORT="${TUMULT_PG_PORT:-5432}"
USER="${TUMULT_PG_USER:-postgres}"

if ! command -v psql >/dev/null 2>&1; then
    echo "error: psql not found" >&2
    exit 1
fi

export PGPASSWORD="${TUMULT_PG_PASSWORD:-}"

if [ -n "${TUMULT_PG_DATABASE}" ]; then
    QUERY="SELECT count(*) FROM pg_stat_activity WHERE datname = '${TUMULT_PG_DATABASE}';"
else
    QUERY="SELECT count(*) FROM pg_stat_activity;"
fi

psql -h "${HOST}" -p "${PORT}" -U "${USER}" -d postgres -t -A -c "${QUERY}"
