#!/bin/sh
# Probe: check Kafka consumer group lag
# Outputs: total lag across all partitions (integer)
#
# Environment variables:
#   TUMULT_KAFKA_BOOTSTRAP - Bootstrap servers (default: localhost:9092)
#   TUMULT_CONSUMER_GROUP  - Consumer group to check (required)
#   TUMULT_KAFKA_DIR       - Kafka install directory (default: /opt/kafka)
set -e

BOOTSTRAP="${TUMULT_KAFKA_BOOTSTRAP:-localhost:9092}"
GROUP="${TUMULT_CONSUMER_GROUP:?TUMULT_CONSUMER_GROUP is required}"
KAFKA_DIR="${TUMULT_KAFKA_DIR:-/opt/kafka}"

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

# Sum up lag across all partitions
"${CMD}" --bootstrap-server "${BOOTSTRAP}" --group "${GROUP}" --describe 2>/dev/null \
    | awk 'NR > 1 && $6 ~ /^[0-9]+$/ { total += $6 } END { print total+0 }'
