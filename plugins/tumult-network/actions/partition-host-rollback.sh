#!/bin/sh
# Rollback: remove the iptables DROP rules added by partition-host.sh
# Requires root/sudo. Linux only. Idempotent — deleting a rule that is
# already absent is ignored, so this is safe to run repeatedly.
#
# Environment variables (same inputs as partition-host.sh):
#   TUMULT_TARGET_IP   - IP address that was partitioned (required)
#   TUMULT_DIRECTION   - Block direction used: both, ingress, egress (default: both)
set -eu

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

case "${DIRECTION}" in
    both)
        iptables -D INPUT -s "${TARGET_IP}" -j DROP -m comment --comment "tumult-partition" 2>/dev/null && \
            echo "removed INPUT drop rule for ${TARGET_IP}" || true
        iptables -D OUTPUT -d "${TARGET_IP}" -j DROP -m comment --comment "tumult-partition" 2>/dev/null && \
            echo "removed OUTPUT drop rule for ${TARGET_IP}" || true
        ;;
    ingress)
        iptables -D INPUT -s "${TARGET_IP}" -j DROP -m comment --comment "tumult-partition" 2>/dev/null && \
            echo "removed INPUT drop rule for ${TARGET_IP}" || true
        ;;
    egress)
        iptables -D OUTPUT -d "${TARGET_IP}" -j DROP -m comment --comment "tumult-partition" 2>/dev/null && \
            echo "removed OUTPUT drop rule for ${TARGET_IP}" || true
        ;;
    *)
        echo "error: TUMULT_DIRECTION must be 'both', 'ingress', or 'egress'" >&2
        exit 1
        ;;
esac

echo "partition rollback complete for ${TARGET_IP}"
