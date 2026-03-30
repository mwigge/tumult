#!/bin/sh
# Probe: count under-replicated partitions
# Outputs: integer count (0 = healthy)
#
# Environment variables:
#   TUMULT_KAFKA_BOOTSTRAP - Bootstrap servers (default: localhost:9092)
#   TUMULT_KAFKA_DIR       - Kafka install directory (default: /opt/kafka)
set -eu

BOOTSTRAP="${TUMULT_KAFKA_BOOTSTRAP:-localhost:9092}"
KAFKA_DIR="${TUMULT_KAFKA_DIR:-/opt/kafka}"

if [ -x "${KAFKA_DIR}/bin/kafka-topics.sh" ]; then
    CMD="${KAFKA_DIR}/bin/kafka-topics.sh"
elif command -v kafka-topics >/dev/null 2>&1; then
    CMD="kafka-topics"
elif command -v kafka-topics.sh >/dev/null 2>&1; then
    CMD="kafka-topics.sh"
else
    echo "error: kafka-topics not found" >&2
    exit 1
fi

"${CMD}" --bootstrap-server "${BOOTSTRAP}" --describe --under-replicated-partitions 2>/dev/null \
    | grep -c "Topic:" || echo "0"
