#!/bin/sh
# Kill active MySQL connections to a database
# Prints how many connections were killed. A failed list fails the action.
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
# A failed list ends the action with the status and the message of mysql.
PIDS=$(mysql -h "${HOST}" -P "${PORT}" -u "${USER}" -N -e \
    "SELECT id FROM information_schema.processlist WHERE db = '${DATABASE}' AND id <> CONNECTION_ID();")

# A connection can end between the list and its KILL. That KILL fails
# without failing the action, and only the connections killed are counted.
COUNT=0
for PID in ${PIDS}; do
    validate_integer "PID" "${PID}"
    if mysql -h "${HOST}" -P "${PORT}" -u "${USER}" -e "KILL ${PID};"; then
        COUNT=$((COUNT + 1))
    fi
done

echo "killed ${COUNT} connections to ${DATABASE}"
