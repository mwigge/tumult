#!/bin/sh
# Probe: get Redis connection and memory stats as JSON
# Outputs structured data for journal capture across experiment phases
set -e

HOST="${TUMULT_REDIS_HOST:-localhost}"
PORT="${TUMULT_REDIS_PORT:-6379}"

if ! command -v redis-cli >/dev/null 2>&1; then
    echo "error: redis-cli not found" >&2
    exit 1
fi


export REDISCLI_AUTH="${TUMULT_REDIS_AUTH:-}"

# Fetch key metrics and format as JSON
INFO=$(redis-cli -h "${HOST}" -p "${PORT}" INFO 2>/dev/null)

CONNECTED=$(echo "${INFO}" | grep "^connected_clients:" | cut -d: -f2 | tr -d '\r')
USED_MEM=$(echo "${INFO}" | grep "^used_memory:" | cut -d: -f2 | tr -d '\r')
USED_MEM_HUMAN=$(echo "${INFO}" | grep "^used_memory_human:" | cut -d: -f2 | tr -d '\r')
OPS_SEC=$(echo "${INFO}" | grep "^instantaneous_ops_per_sec:" | cut -d: -f2 | tr -d '\r')
KEYSPACE_HITS=$(echo "${INFO}" | grep "^keyspace_hits:" | cut -d: -f2 | tr -d '\r')
KEYSPACE_MISSES=$(echo "${INFO}" | grep "^keyspace_misses:" | cut -d: -f2 | tr -d '\r')
UPTIME=$(echo "${INFO}" | grep "^uptime_in_seconds:" | cut -d: -f2 | tr -d '\r')

printf '{"connected_clients": %s, "used_memory_bytes": %s, "used_memory_human": "%s", "ops_per_sec": %s, "keyspace_hits": %s, "keyspace_misses": %s, "uptime_seconds": %s}' \
    "${CONNECTED:-0}" "${USED_MEM:-0}" "${USED_MEM_HUMAN:-0B}" "${OPS_SEC:-0}" "${KEYSPACE_HITS:-0}" "${KEYSPACE_MISSES:-0}" "${UPTIME:-0}"
