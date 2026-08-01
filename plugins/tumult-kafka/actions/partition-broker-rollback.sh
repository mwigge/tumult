#!/bin/sh
# Rollback: remove the iptables DROP rules added by partition-broker.sh
# Requires root/sudo. Linux only. Idempotent — deleting a rule that is
# already absent is ignored, so this is safe to run repeatedly.
#
# Environment variables (same inputs as partition-broker.sh):
#   TUMULT_BROKER_IP     - IP of the broker that was partitioned (required)
#   TUMULT_CLUSTER_IPS   - Comma-separated IPs of other brokers (required)
#   TUMULT_KAFKA_PORT    - Kafka port (default: 9092)
set -eu

. "$(dirname "$0")/../../lib/validate.sh"

BROKER_IP="${TUMULT_BROKER_IP:?TUMULT_BROKER_IP is required}"
CLUSTER_IPS="${TUMULT_CLUSTER_IPS:?TUMULT_CLUSTER_IPS is required}"
KAFKA_PORT="${TUMULT_KAFKA_PORT:-9092}"

validate_integer "TUMULT_KAFKA_PORT" "${KAFKA_PORT}"

# IP addresses: IPv4/IPv6 characters only (digits, hex letters, '.', ':').
case "${BROKER_IP}" in
    ''|*[!0-9a-fA-F.:]*)
        echo "error: TUMULT_BROKER_IP contains invalid characters: '${BROKER_IP}'" >&2
        echo "  allowed: digits, hex letters, '.', ':'" >&2
        exit 1
        ;;
esac

# Validate every cluster IP before any iptables rule is touched.
OLD_IFS="$IFS"
IFS=','
for IP in ${CLUSTER_IPS}; do
    IP=$(echo "${IP}" | tr -d ' ')
    case "${IP}" in
        ''|*[!0-9a-fA-F.:]*)
            echo "error: TUMULT_CLUSTER_IPS contains invalid IP: '${IP}'" >&2
            echo "  allowed: digits, hex letters, '.', ':'" >&2
            exit 1
            ;;
    esac
done
IFS="$OLD_IFS"

if ! command -v iptables >/dev/null 2>&1; then
    echo "error: iptables not found" >&2
    exit 1
fi

# Split comma-separated IPs (same parsing as partition-broker.sh)
OLD_IFS="$IFS"
IFS=','
for IP in ${CLUSTER_IPS}; do
    IP=$(echo "${IP}" | tr -d ' ')
    iptables -D INPUT -s "${IP}" -p tcp --dport "${KAFKA_PORT}" -j DROP -m comment --comment "tumult-kafka-partition" 2>/dev/null && \
        echo "  removed INPUT drop rule for ${IP}" || true
    iptables -D OUTPUT -d "${IP}" -p tcp --sport "${KAFKA_PORT}" -j DROP -m comment --comment "tumult-kafka-partition" 2>/dev/null && \
        echo "  removed OUTPUT drop rule for ${IP}" || true
done
IFS="$OLD_IFS"

echo "broker ${BROKER_IP} partition rollback complete"
