#!/bin/sh
# Probe: check PostgreSQL connection pool utilization
# Outputs: JSON with current connections, max connections, and utilization %
#
# Environment variables:
#   TUMULT_PG_HOST     - PostgreSQL host (default: localhost)
#   TUMULT_PG_PORT     - PostgreSQL port (default: 5432)
#   TUMULT_PG_USER     - PostgreSQL user (default: postgres)
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

QUERY="SELECT json_build_object(
    'current_connections', (SELECT count(*) FROM pg_stat_activity),
    'max_connections', (SELECT setting::int FROM pg_settings WHERE name = 'max_connections'),
    'utilization_pct', round((SELECT count(*)::numeric FROM pg_stat_activity) / (SELECT setting::numeric FROM pg_settings WHERE name = 'max_connections') * 100, 1)
);"

psql -h "${HOST}" -p "${PORT}" -U "${USER}" -d postgres -t -A -c "${QUERY}"
