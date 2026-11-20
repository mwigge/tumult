#!/bin/sh
# Probe: check Kafka consumer group lag
# Outputs: total lag across all partitions (integer). A failed query exits
# non-zero with the message of the tool.
#
# Environment variables:
#   TUMULT_KAFKA_BOOTSTRAP - Bootstrap servers (default: localhost:9092)
#   TUMULT_CONSUMER_GROUP  - Consumer group to check (required)
#   TUMULT_KAFKA_DIR       - Kafka install directory (default: /opt/kafka)
set -eu

. "$(dirname "$0")/../../lib/validate.sh"

BOOTSTRAP="${TUMULT_KAFKA_BOOTSTRAP:-localhost:9092}"
GROUP="${TUMULT_CONSUMER_GROUP:?TUMULT_CONSUMER_GROUP is required}"
KAFKA_DIR="${TUMULT_KAFKA_DIR:-/opt/kafka}"

# Bootstrap servers: host:port list — hostname, IP, port, and separator chars only.
case "${BOOTSTRAP}" in
    ''|*[!a-zA-Z0-9.,:_-]*)
        echo "error: TUMULT_KAFKA_BOOTSTRAP contains invalid characters: '${BOOTSTRAP}'" >&2
        echo "  allowed: letters, digits, '.', ',', ':', '_', '-'" >&2
        exit 1
        ;;
esac

# Consumer group names: letters, digits, '_', '.', '-'.
case "${GROUP}" in
    ''|*[!a-zA-Z0-9._-]*)
        echo "error: TUMULT_CONSUMER_GROUP contains invalid characters: '${GROUP}'" >&2
        echo "  allowed: letters, digits, '_', '.', '-'" >&2
        exit 1
        ;;
esac

# Try kafka-consumer-groups.sh first, then kafka CLI
if [ -x "${KAFKA_DIR}/bin/kafka-consumer-groups.sh" ]; then
    CMD="${KAFKA_DIR}/bin/kafka-consumer-groups.sh"
elif command -v kafka-consumer-groups >/dev/null 2>&1; then
    CMD="kafka-consumer-groups"
elif command -v kafka-consumer-groups.sh >/dev/null 2>&1; then
    CMD="kafka-consumer-groups.sh"
else
    echo "error: kafka-consumer-groups not found" >&2
    exit 1
fi

# The query runs on its own first, so a failed query ends the probe with
# the status and the message of the tool instead of a lag of zero.
DESCRIBE=$("${CMD}" --bootstrap-server "${BOOTSTRAP}" --group "${GROUP}" --describe)

# Sum up lag across all partitions
printf '%s\n' "${DESCRIBE}" \
    | awk 'NR > 1 && $6 ~ /^[0-9]+$/ { total += $6 } END { print total+0 }'
