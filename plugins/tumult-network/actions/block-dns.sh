#!/bin/sh
# Block DNS resolution via iptables
# Requires root/sudo. Linux only.
#
# Environment variables:
#   TUMULT_DNS_PORT  - DNS port to block (default: 53)
set -eu

DNS_PORT="${TUMULT_DNS_PORT:-53}"

if [ "$(uname -s)" != "Linux" ]; then
    echo "error: iptables requires Linux" >&2
    exit 1
fi

if ! command -v iptables >/dev/null 2>&1; then
    echo "error: iptables not found" >&2
    exit 1
fi

echo "blocking DNS (port ${DNS_PORT}) via iptables"
iptables -A OUTPUT -p udp --dport "${DNS_PORT}" -j DROP -m comment --comment "tumult-dns-block"
iptables -A OUTPUT -p tcp --dport "${DNS_PORT}" -j DROP -m comment --comment "tumult-dns-block"
echo "DNS blocked"
