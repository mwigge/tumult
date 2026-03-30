#!/bin/sh
# Kill active MySQL connections to a database
set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "${SCRIPT_DIR}/../../lib/validate.sh"

HOST="${TUMULT_MYSQL_HOST:-localhost}"
PORT="${TUMULT_MYSQL_PORT:-3306}"
USER="${TUMULT_MYSQL_USER:-root}"
DATABASE="${TUMULT_MYSQL_DATABASE:?TUMULT_MYSQL_DATABASE is required}"

validate_identifier "TUMULT_MYSQL_DATABASE" "${DATABASE}"

if ! command -v mysql >/dev/null 2>&1; then
    echo "error: mysql client not found" >&2
    exit 1
fi

# Use MYSQL_PWD env var instead of -p flag (avoids process list exposure)
export MYSQL_PWD="${TUMULT_MYSQL_PASSWORD:-}"

echo "killing connections to ${DATABASE}"
PIDS=$(mysql -h "${HOST}" -P "${PORT}" -u "${USER}" -N -e \
    "SELECT id FROM information_schema.processlist WHERE db = '${DATABASE}' AND id <> CONNECTION_ID();" 2>/dev/null)

COUNT=0
for PID in ${PIDS}; do
    validate_integer "PID" "${PID}"
    mysql -h "${HOST}" -P "${PORT}" -u "${USER}" -e "KILL ${PID};" 2>/dev/null || true
    COUNT=$((COUNT + 1))
done

echo "killed ${COUNT} connections to ${DATABASE}"
