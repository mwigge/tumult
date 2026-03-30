#!/bin/sh
# Kill active MySQL connections to a database
#
# Environment variables:
#   TUMULT_MYSQL_HOST     - MySQL host (default: localhost)
#   TUMULT_MYSQL_PORT     - MySQL port (default: 3306)
#   TUMULT_MYSQL_USER     - MySQL user (default: root)
#   TUMULT_MYSQL_PASSWORD - Password (optional)
#   TUMULT_MYSQL_DATABASE - Target database (required)
set -e

HOST="${TUMULT_MYSQL_HOST:-localhost}"
PORT="${TUMULT_MYSQL_PORT:-3306}"
USER="${TUMULT_MYSQL_USER:-root}"
PASSWORD="${TUMULT_MYSQL_PASSWORD:-}"
DATABASE="${TUMULT_MYSQL_DATABASE:?TUMULT_MYSQL_DATABASE is required}"

if ! command -v mysql >/dev/null 2>&1; then
    echo "error: mysql client not found" >&2
    exit 1
fi

PASS_ARG=""
[ -n "${PASSWORD}" ] && PASS_ARG="-p${PASSWORD}"

echo "killing connections to ${DATABASE}"
PIDS=$(mysql -h "${HOST}" -P "${PORT}" -u "${USER}" ${PASS_ARG} -N -e \
    "SELECT id FROM information_schema.processlist WHERE db = '${DATABASE}' AND id <> CONNECTION_ID();" 2>/dev/null)

COUNT=0
for PID in ${PIDS}; do
    mysql -h "${HOST}" -P "${PORT}" -u "${USER}" ${PASS_ARG} -e "KILL ${PID};" 2>/dev/null || true
    COUNT=$((COUNT + 1))
done

echo "killed ${COUNT} connections to ${DATABASE}"
