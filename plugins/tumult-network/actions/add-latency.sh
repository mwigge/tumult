#!/bin/sh
# Add network latency using tc netem
# Requires root/sudo. Works on Linux only.
#
# Environment variables:
#   TUMULT_INTERFACE  - Network interface (default: eth0)
#   TUMULT_DELAY_MS   - Latency in milliseconds (default: 100)
#   TUMULT_JITTER_MS  - Jitter in milliseconds (default: 10)
#   TUMULT_TARGET_IP  - Only affect traffic to this IP (optional)
set -e

. "$(dirname "$0")/../../lib/validate.sh"

INTERFACE="${TUMULT_INTERFACE:-eth0}"
DELAY="${TUMULT_DELAY_MS:-100}"
JITTER="${TUMULT_JITTER_MS:-10}"

validate_number "TUMULT_DELAY_MS" "${DELAY}"
validate_number "TUMULT_JITTER_MS" "${JITTER}"

if [ "$(uname -s)" != "Linux" ]; then
    echo "error: tc netem requires Linux" >&2
    exit 1
fi

if ! command -v tc >/dev/null 2>&1; then
    echo "error: tc not found (install iproute2)" >&2
    exit 1
fi

if [ -n "${TUMULT_TARGET_IP}" ]; then
    echo "adding ${DELAY}ms (±${JITTER}ms) latency to ${TUMULT_TARGET_IP} on ${INTERFACE}"
    tc qdisc add dev "${INTERFACE}" root handle 1: prio
    tc qdisc add dev "${INTERFACE}" parent 1:3 handle 30: netem delay "${DELAY}ms" "${JITTER}ms"
    tc filter add dev "${INTERFACE}" parent 1:0 protocol ip u32 match ip dst "${TUMULT_TARGET_IP}/32" flowid 1:3
else
    echo "adding ${DELAY}ms (±${JITTER}ms) latency on ${INTERFACE}"
    tc qdisc add dev "${INTERFACE}" root netem delay "${DELAY}ms" "${JITTER}ms"
fi

echo "latency injection active"
