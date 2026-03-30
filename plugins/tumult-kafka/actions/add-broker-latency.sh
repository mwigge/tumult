#!/bin/sh
# Add network latency to a Kafka broker using tc netem
# Requires root/sudo. Linux only.
#
# Environment variables:
#   TUMULT_INTERFACE  - Network interface (default: eth0)
#   TUMULT_DELAY_MS   - Latency in milliseconds (default: 100)
#   TUMULT_JITTER_MS  - Jitter in milliseconds (default: 10)
#   TUMULT_KAFKA_PORT - Kafka port to target (default: 9092)
set -e

INTERFACE="${TUMULT_INTERFACE:-eth0}"
DELAY="${TUMULT_DELAY_MS:-100}"
JITTER="${TUMULT_JITTER_MS:-10}"
KAFKA_PORT="${TUMULT_KAFKA_PORT:-9092}"

if [ "$(uname -s)" != "Linux" ]; then
    echo "error: tc netem requires Linux" >&2
    exit 1
fi

echo "adding ${DELAY}ms (±${JITTER}ms) latency to Kafka port ${KAFKA_PORT} on ${INTERFACE}"
tc qdisc add dev "${INTERFACE}" root handle 1: prio
tc qdisc add dev "${INTERFACE}" parent 1:3 handle 30: netem delay "${DELAY}ms" "${JITTER}ms"
tc filter add dev "${INTERFACE}" parent 1:0 protocol ip u32 match ip dport "${KAFKA_PORT}" 0xffff flowid 1:3
echo "broker latency injection active"
