#!/bin/sh
# Rollback: remove tumult DNS block entries
# Handles both /etc/hosts entries and iptables rules
set -eu

MARKER="tumult-dns-block"

# Remove /etc/hosts entries if any
if grep -q "${MARKER}" /etc/hosts 2>/dev/null; then
    sed -i.bak "/${MARKER}/d" /etc/hosts
    rm -f /etc/hosts.bak
    echo "DNS block entries removed from /etc/hosts"
fi

# Remove iptables rules if present
if command -v iptables >/dev/null 2>&1 && [ "$(uname -s)" = "Linux" ]; then
    iptables -D OUTPUT -p udp --dport 53 -j DROP -m comment --comment "${MARKER}" 2>/dev/null && \
        echo "removed UDP DNS block rule" || true
    iptables -D OUTPUT -p tcp --dport 53 -j DROP -m comment --comment "${MARKER}" 2>/dev/null && \
        echo "removed TCP DNS block rule" || true
fi

echo "DNS block rollback complete"
