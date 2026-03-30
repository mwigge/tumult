#!/bin/sh
# Network partition a host via iptables DROP
# Requires root/sudo. Linux only.
#
# Environment variables:
#   TUMULT_TARGET_IP   - IP address to partition (required)
#   TUMULT_DIRECTION   - Block direction: both, ingress, egress (default: both)
set -e

TARGET_IP="${TUMULT_TARGET_IP:?TUMULT_TARGET_IP is required}"
DIRECTION="${TUMULT_DIRECTION:-both}"

if [ "$(uname -s)" != "Linux" ]; then
    echo "error: iptables requires Linux" >&2
    exit 1
fi

if ! command -v iptables >/dev/null 2>&1; then
    echo "error: iptables not found" >&2
    exit 1
fi

echo "partitioning ${TARGET_IP} (direction: ${DIRECTION})"

case "${DIRECTION}" in
    both)
        iptables -A INPUT -s "${TARGET_IP}" -j DROP -m comment --comment "tumult-partition"
        iptables -A OUTPUT -d "${TARGET_IP}" -j DROP -m comment --comment "tumult-partition"
        ;;
    ingress)
        iptables -A INPUT -s "${TARGET_IP}" -j DROP -m comment --comment "tumult-partition"
        ;;
    egress)
        iptables -A OUTPUT -d "${TARGET_IP}" -j DROP -m comment --comment "tumult-partition"
        ;;
    *)
        echo "error: TUMULT_DIRECTION must be 'both', 'ingress', or 'egress'" >&2
        exit 1
        ;;
esac

echo "partition active for ${TARGET_IP}"
