#!/bin/sh
# Add latency to DNS queries using tc netem on port 53
# Requires root/sudo. Linux only.
#
# This injects delay on all outbound DNS traffic (UDP and TCP port 53)
# using tc's prio qdisc with a netem child. Unlike iptables --string,
# this works regardless of DNS wire format encoding.
#
# Environment variables:
#   TUMULT_INTERFACE    - Network interface (default: eth0)
#   TUMULT_DNS_DELAY_MS - Latency to add in milliseconds (default: 500)
#   TUMULT_DNS_JITTER_MS - Jitter in milliseconds (default: 50)
set -eu

. "$(dirname "$0")/../../lib/validate.sh"

INTERFACE="${TUMULT_INTERFACE:-eth0}"
DELAY="${TUMULT_DNS_DELAY_MS:-500}"
JITTER="${TUMULT_DNS_JITTER_MS:-50}"

validate_number "TUMULT_DNS_DELAY_MS" "${DELAY}"
validate_number "TUMULT_DNS_JITTER_MS" "${JITTER}"

if [ "$(uname -s)" != "Linux" ]; then
    echo "error: tc netem requires Linux" >&2
    exit 1
fi

if ! command -v tc >/dev/null 2>&1; then
    echo "error: tc not found (install iproute2)" >&2
    exit 1
fi

echo "adding ${DELAY}ms (±${JITTER}ms) latency to DNS (port 53) on ${INTERFACE}"

# Create prio qdisc as root, attach netem to band 3
tc qdisc add dev "${INTERFACE}" root handle 1: prio 2>/dev/null || \
    tc qdisc replace dev "${INTERFACE}" root handle 1: prio
tc qdisc add dev "${INTERFACE}" parent 1:3 handle 30: netem delay "${DELAY}ms" "${JITTER}ms"

# Filter UDP and TCP port 53 traffic to the netem band
tc filter add dev "${INTERFACE}" parent 1:0 protocol ip u32 \
    match ip dport 53 0xffff flowid 1:3

echo "DNS latency injection active"
