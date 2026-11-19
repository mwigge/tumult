#!/bin/sh
# Probe: get Redis connection and memory stats as JSON
# Outputs structured data for journal capture across experiment phases
# Fails when redis-cli fails or its reply lacks a field; the reply goes to stderr.
set -eu

HOST="${TUMULT_REDIS_HOST:-localhost}"
PORT="${TUMULT_REDIS_PORT:-6379}"

if ! command -v redis-cli >/dev/null 2>&1; then
    echo "error: redis-cli not found" >&2
    exit 1
fi


export REDISCLI_AUTH="${TUMULT_REDIS_AUTH:-}"

# Fetch key metrics and format as JSON. A failed query ends the probe with
# the status and the message of redis-cli.
INFO=$(redis-cli -h "${HOST}" -p "${PORT}" INFO)

# The value of the field $1 in the reply. An error reply such as NOAUTH can
# come with exit 0 and holds no field: the probe then fails and shows it.
field() {
    VALUE=$(echo "${INFO}" | grep "^$1:" | cut -d: -f2 | tr -d '\r')
    if [ -z "${VALUE}" ]; then
        echo "error: the reply of redis-cli INFO has no $1:" >&2
        echo "${INFO}" >&2
        exit 1
    fi
    echo "${VALUE}"
}

CONNECTED=$(field connected_clients)
USED_MEM=$(field used_memory)
USED_MEM_HUMAN=$(field used_memory_human)
OPS_SEC=$(field instantaneous_ops_per_sec)
KEYSPACE_HITS=$(field keyspace_hits)
KEYSPACE_MISSES=$(field keyspace_misses)
UPTIME=$(field uptime_in_seconds)

printf '{"connected_clients": %s, "used_memory_bytes": %s, "used_memory_human": "%s", "ops_per_sec": %s, "keyspace_hits": %s, "keyspace_misses": %s, "uptime_seconds": %s}' \
    "${CONNECTED}" "${USED_MEM}" "${USED_MEM_HUMAN}" "${OPS_SEC}" "${KEYSPACE_HITS}" "${KEYSPACE_MISSES}" "${UPTIME}"
