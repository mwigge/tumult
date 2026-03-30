#!/bin/sh
# Kill a Kafka broker process
#
# Environment variables:
#   TUMULT_BROKER_HOST - Broker hostname or IP (required for SSH)
#   TUMULT_BROKER_ID   - Broker ID (used to find the right process)
#   TUMULT_SIGNAL      - Signal to send (default: KILL)
#   TUMULT_KAFKA_DIR   - Kafka install directory (default: /opt/kafka)
set -e

SIGNAL="${TUMULT_SIGNAL:-KILL}"
KAFKA_DIR="${TUMULT_KAFKA_DIR:-/opt/kafka}"

if [ -n "${TUMULT_BROKER_HOST}" ]; then
    echo "killing Kafka broker on ${TUMULT_BROKER_HOST}"
    # When run via SSH, kill the local Kafka process
fi

# Find Kafka broker process
PID=$(pgrep -f "kafka.Kafka" 2>/dev/null | head -1) || true

if [ -z "${PID}" ]; then
    # Try alternate process name patterns
    PID=$(pgrep -f "kafka-server-start" 2>/dev/null | head -1) || true
fi

if [ -z "${PID}" ]; then
    echo "error: no Kafka broker process found" >&2
    exit 1
fi

echo "killing Kafka broker PID ${PID} with signal ${SIGNAL}"
kill -s "${SIGNAL}" "${PID}"
echo "Kafka broker killed"
