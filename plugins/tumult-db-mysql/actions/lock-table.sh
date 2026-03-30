#!/bin/sh
# Acquire exclusive lock on a MySQL table
#
# Environment variables:
#   TUMULT_MYSQL_HOST     - MySQL host (default: localhost)
#   TUMULT_MYSQL_PORT     - MySQL port (default: 3306)
#   TUMULT_MYSQL_USER     - MySQL user (default: root)
#   TUMULT_MYSQL_PASSWORD - Password (optional)
#   TUMULT_MYSQL_DATABASE - Target database (required)
#   TUMULT_MYSQL_TABLE    - Table to lock (required)
#   TUMULT_DURATION       - Lock duration in seconds (default: 10)
set -e

HOST="${TUMULT_MYSQL_HOST:-localhost}"
PORT="${TUMULT_MYSQL_PORT:-3306}"
USER="${TUMULT_MYSQL_USER:-root}"
PASSWORD="${TUMULT_MYSQL_PASSWORD:-}"
DATABASE="${TUMULT_MYSQL_DATABASE:?TUMULT_MYSQL_DATABASE is required}"
TABLE="${TUMULT_MYSQL_TABLE:?TUMULT_MYSQL_TABLE is required}"
DURATION="${TUMULT_DURATION:-10}"

if ! command -v mysql >/dev/null 2>&1; then
    echo "error: mysql client not found" >&2
    exit 1
fi

PASS_ARG=""
[ -n "${PASSWORD}" ] && PASS_ARG="-p${PASSWORD}"

echo "locking table ${TABLE} in ${DATABASE} for ${DURATION}s"
mysql -h "${HOST}" -P "${PORT}" -u "${USER}" ${PASS_ARG} "${DATABASE}" -e \
    "LOCK TABLES ${TABLE} WRITE; SELECT SLEEP(${DURATION}); UNLOCK TABLES;"
echo "lock released on ${TABLE}"
