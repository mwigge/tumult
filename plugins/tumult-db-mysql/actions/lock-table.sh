#!/bin/sh
# Acquire exclusive lock on a MySQL table
set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "${SCRIPT_DIR}/../../lib/validate.sh"

HOST="${TUMULT_MYSQL_HOST:-localhost}"
PORT="${TUMULT_MYSQL_PORT:-3306}"
USER="${TUMULT_MYSQL_USER:-root}"
DATABASE="${TUMULT_MYSQL_DATABASE:?TUMULT_MYSQL_DATABASE is required}"
TABLE="${TUMULT_MYSQL_TABLE:?TUMULT_MYSQL_TABLE is required}"
DURATION="${TUMULT_DURATION:-10}"

validate_identifier "TUMULT_MYSQL_DATABASE" "${DATABASE}"
validate_identifier "TUMULT_MYSQL_TABLE" "${TABLE}"
validate_integer "TUMULT_DURATION" "${DURATION}"

if ! command -v mysql >/dev/null 2>&1; then
    echo "error: mysql client not found" >&2
    exit 1
fi

export MYSQL_PWD="${TUMULT_MYSQL_PASSWORD:-}"

echo "locking table ${TABLE} in ${DATABASE} for ${DURATION}s"
mysql -h "${HOST}" -P "${PORT}" -u "${USER}" "${DATABASE}" -e \
    "LOCK TABLES \`${TABLE}\` WRITE; SELECT SLEEP(${DURATION}); UNLOCK TABLES;"
echo "lock released on ${TABLE}"
