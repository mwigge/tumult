#!/bin/sh
# Probe: check if Redis is responsive
# Outputs: "PONG" on success, error message on failure
set -e

HOST="${TUMULT_REDIS_HOST:-localhost}"
PORT="${TUMULT_REDIS_PORT:-6379}"

if ! command -v redis-cli >/dev/null 2>&1; then
    echo "error: redis-cli not found" >&2
    exit 1
fi

AUTH_ARG=""
[ -n "${TUMULT_REDIS_AUTH}" ] && AUTH_ARG="-a ${TUMULT_REDIS_AUTH}"

redis-cli -h "${HOST}" -p "${PORT}" ${AUTH_ARG} PING
