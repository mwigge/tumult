#!/bin/sh
# Fill Kafka broker disk by manipulating log retention
# Reduces retention to force segment accumulation
#
# Environment variables:
#   TUMULT_KAFKA_BOOTSTRAP - Bootstrap servers (default: localhost:9092)
#   TUMULT_TOPIC           - Topic to manipulate (required)
#   TUMULT_RETENTION_MS    - Set retention to this value in ms (default: 1000 = 1s)
#   TUMULT_KAFKA_DIR       - Kafka install directory (default: /opt/kafka)
set -eu

BOOTSTRAP="${TUMULT_KAFKA_BOOTSTRAP:-localhost:9092}"
TOPIC="${TUMULT_TOPIC:?TUMULT_TOPIC is required}"
RETENTION_MS="${TUMULT_RETENTION_MS:-1000}"
KAFKA_DIR="${TUMULT_KAFKA_DIR:-/opt/kafka}"

if [ -x "${KAFKA_DIR}/bin/kafka-configs.sh" ]; then
    CMD="${KAFKA_DIR}/bin/kafka-configs.sh"
elif command -v kafka-configs >/dev/null 2>&1; then
    CMD="kafka-configs"
else
    echo "error: kafka-configs not found" >&2
    exit 1
fi

echo "setting retention.ms=${RETENTION_MS} on topic ${TOPIC}"
"${CMD}" --bootstrap-server "${BOOTSTRAP}" \
    --entity-type topics --entity-name "${TOPIC}" \
    --alter --add-config "retention.ms=${RETENTION_MS}"
echo "retention policy updated — disk pressure will build as segments expire"
