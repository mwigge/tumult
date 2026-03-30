#!/bin/sh
# Probe: count active PostgreSQL connections
set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "${SCRIPT_DIR}/../../lib/validate.sh"

HOST="${TUMULT_PG_HOST:-localhost}"
PORT="${TUMULT_PG_PORT:-5432}"
USER="${TUMULT_PG_USER:-postgres}"

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

if [ -n "${TUMULT_PG_DATABASE:-}" ]; then
    validate_identifier "TUMULT_PG_DATABASE" "${TUMULT_PG_DATABASE}"
    psql -h "${HOST}" -p "${PORT}" -U "${USER}" -d postgres -t -A -c \
        "SELECT count(*) FROM pg_stat_activity WHERE datname = \$\$${TUMULT_PG_DATABASE}\$\$;"
else
    psql -h "${HOST}" -p "${PORT}" -U "${USER}" -d postgres -t -A -c \
        "SELECT count(*) FROM pg_stat_activity;"
fi
