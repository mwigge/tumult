#!/bin/sh
# Flush all data from Redis (FLUSHALL)
#
# WARNING: This is destructive — all data will be lost.
#
# Environment variables:
#   TUMULT_REDIS_HOST - Redis host (default: localhost)
#   TUMULT_REDIS_PORT - Redis port (default: 6379)
#   TUMULT_REDIS_AUTH - AUTH password (optional)
set -e

HOST="${TUMULT_REDIS_HOST:-localhost}"
PORT="${TUMULT_REDIS_PORT:-6379}"

if ! command -v redis-cli >/dev/null 2>&1; then
    echo "error: redis-cli not found" >&2
    exit 1
fi

AUTH_ARG=""
[ -n "${TUMULT_REDIS_AUTH}" ] && AUTH_ARG="-a ${TUMULT_REDIS_AUTH}"

echo "flushing all data from Redis at ${HOST}:${PORT}"
redis-cli -h "${HOST}" -p "${PORT}" ${AUTH_ARG} FLUSHALL
echo "all data flushed"
