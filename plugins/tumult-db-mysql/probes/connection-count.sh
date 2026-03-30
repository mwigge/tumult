#!/bin/sh
# Probe: count active MySQL connections
# Outputs: integer count
set -e

HOST="${TUMULT_MYSQL_HOST:-localhost}"
PORT="${TUMULT_MYSQL_PORT:-3306}"
USER="${TUMULT_MYSQL_USER:-root}"
PASSWORD="${TUMULT_MYSQL_PASSWORD:-}"

if ! command -v mysql >/dev/null 2>&1; then
    echo "error: mysql client not found" >&2
    exit 1
fi

PASS_ARG=""
[ -n "${PASSWORD}" ] && PASS_ARG="-p${PASSWORD}"

mysql -h "${HOST}" -P "${PORT}" -u "${USER}" ${PASS_ARG} -N -e "SELECT count(*) FROM information_schema.processlist;" 2>/dev/null
