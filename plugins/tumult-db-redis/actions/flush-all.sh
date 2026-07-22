#!/bin/sh
# Flush all data from Redis (FLUSHALL)
#
# *** WARNING: DESTRUCTIVE — NO ROLLBACK ***
# This action performs FLUSHALL: every key in every database on the target
# Redis instance is deleted immediately and irreversibly. There is no
# rollback action; the only recovery is from an external backup, replica,
# or AOF/RDB snapshot taken beforehand. Do not run this against an instance
# whose data you cannot afford to lose.
#
# Environment variables:
#   TUMULT_REDIS_HOST - Redis host (default: localhost)
#   TUMULT_REDIS_PORT - Redis port (default: 6379)
#   TUMULT_REDIS_AUTH - AUTH password (optional)
set -eu

HOST="${TUMULT_REDIS_HOST:-localhost}"
PORT="${TUMULT_REDIS_PORT:-6379}"

if ! command -v redis-cli >/dev/null 2>&1; then
    echo "error: redis-cli not found" >&2
    exit 1
fi


export REDISCLI_AUTH="${TUMULT_REDIS_AUTH:-}"

echo "flushing all data from Redis at ${HOST}:${PORT}"
redis-cli -h "${HOST}" -p "${PORT}" FLUSHALL
echo "all data flushed"
