#!/bin/sh
# Simulate Redis hang using DEBUG SLEEP
#
# Environment variables:
#   TUMULT_REDIS_HOST - Redis host (default: localhost)
#   TUMULT_REDIS_PORT - Redis port (default: 6379)
#   TUMULT_REDIS_AUTH - AUTH password (optional)
#   TUMULT_DURATION   - Sleep duration in seconds (default: 5)
set -e

HOST="${TUMULT_REDIS_HOST:-localhost}"
PORT="${TUMULT_REDIS_PORT:-6379}"
DURATION="${TUMULT_DURATION:-5}"

if ! command -v redis-cli >/dev/null 2>&1; then
    echo "error: redis-cli not found" >&2
    exit 1
fi

AUTH_ARG=""
[ -n "${TUMULT_REDIS_AUTH}" ] && AUTH_ARG="-a ${TUMULT_REDIS_AUTH}"

echo "simulating Redis hang for ${DURATION}s via DEBUG SLEEP"
redis-cli -h "${HOST}" -p "${PORT}" ${AUTH_ARG} DEBUG SLEEP "${DURATION}" &
echo "Redis hang initiated for ${DURATION}s"
