#!/bin/sh
# Probe: count active MySQL connections
set -eu

HOST="${TUMULT_MYSQL_HOST:-localhost}"
PORT="${TUMULT_MYSQL_PORT:-3306}"
USER="${TUMULT_MYSQL_USER:-root}"

if ! command -v mysql >/dev/null 2>&1; then
    echo "error: mysql client not found" >&2
    exit 1
fi

export MYSQL_PWD="${TUMULT_MYSQL_PASSWORD:-}"

mysql -h "${HOST}" -P "${PORT}" -u "${USER}" -N -e "SELECT count(*) FROM information_schema.processlist;" 2>/dev/null
