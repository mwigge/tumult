#!/bin/sh
# Terminate active PostgreSQL connections to a database
# Prints how many connections were terminated. A failed query fails the action.
set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "${SCRIPT_DIR}/../../lib/validate.sh"

HOST="${TUMULT_PG_HOST:-localhost}"
PORT="${TUMULT_PG_PORT:-5432}"
USER="${TUMULT_PG_USER:-postgres}"
DATABASE="${TUMULT_PG_DATABASE:?TUMULT_PG_DATABASE is required}"

validate_identifier "TUMULT_PG_DATABASE" "${DATABASE}"

if ! command -v psql >/dev/null 2>&1; then
    echo "error: psql not found" >&2
    exit 1
fi

# Use .pgpass file to avoid /proc/environ exposure
PGPASS_FILE=$(mktemp)
trap "rm -f ${PGPASS_FILE}" EXIT INT TERM
echo "*:*:*:*:${TUMULT_PG_PASSWORD:-}" > "${PGPASS_FILE}"
chmod 600 "${PGPASS_FILE}"
export PGPASSFILE="${PGPASS_FILE}"

# A failed query ends the action with the status and the message of psql.
RESULT=$(psql -h "${HOST}" -p "${PORT}" -U "${USER}" -d postgres -t \
    -c "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = \$\$${DATABASE}\$\$ AND pid <> pg_backend_pid();")
# One row per connection: t when it was terminated, f when it had gone already.
COUNT=$(echo "${RESULT}" | awk '$1 == "t" { count++ } END { print count+0 }')

echo "terminated ${COUNT} connections to ${DATABASE}"
