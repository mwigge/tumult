#!/bin/sh
# Network partition a Kafka broker from other brokers via iptables
# Requires root/sudo. Linux only.
#
# Environment variables:
#   TUMULT_BROKER_IP     - IP of the broker to partition (required)
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

# Validate every cluster IP before any iptables rule is added.
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

echo "partitioning broker ${BROKER_IP} from cluster"

# Split comma-separated IPs
OLD_IFS="$IFS"
IFS=','
for IP in ${CLUSTER_IPS}; do
    IP=$(echo "${IP}" | tr -d ' ')
    iptables -A INPUT -s "${IP}" -p tcp --dport "${KAFKA_PORT}" -j DROP -m comment --comment "tumult-kafka-partition"
    iptables -A OUTPUT -d "${IP}" -p tcp --sport "${KAFKA_PORT}" -j DROP -m comment --comment "tumult-kafka-partition"
    echo "  blocked ${IP} <-> ${BROKER_IP}:${KAFKA_PORT}"
done
IFS="$OLD_IFS"

echo "broker ${BROKER_IP} partitioned from cluster"
