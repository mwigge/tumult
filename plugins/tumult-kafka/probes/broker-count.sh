#!/bin/sh
# Probe: count active brokers in the Kafka cluster
# Outputs: integer count
#
# Environment variables:
#   TUMULT_KAFKA_BOOTSTRAP - Bootstrap servers (default: localhost:9092)
#   TUMULT_KAFKA_DIR       - Kafka install directory (default: /opt/kafka)
set -eu

BOOTSTRAP="${TUMULT_KAFKA_BOOTSTRAP:-localhost:9092}"
KAFKA_DIR="${TUMULT_KAFKA_DIR:-/opt/kafka}"

if [ -x "${KAFKA_DIR}/bin/kafka-broker-api-versions.sh" ]; then
    CMD="${KAFKA_DIR}/bin/kafka-broker-api-versions.sh"
elif command -v kafka-broker-api-versions >/dev/null 2>&1; then
    CMD="kafka-broker-api-versions"
elif command -v kafka-broker-api-versions.sh >/dev/null 2>&1; then
    CMD="kafka-broker-api-versions.sh"
else
    echo "error: kafka-broker-api-versions not found" >&2
    exit 1
fi

"${CMD}" --bootstrap-server "${BOOTSTRAP}" 2>/dev/null | grep -c "^${BOOTSTRAP}" || echo "0"
