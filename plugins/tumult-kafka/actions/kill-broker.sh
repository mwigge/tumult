#!/bin/sh
# Kill a Kafka broker process
#
# Environment variables:
#   TUMULT_BROKER_HOST - Broker hostname or IP (required for SSH)
#   TUMULT_BROKER_ID   - Broker ID (used to find the right process)
#   TUMULT_SIGNAL      - Signal to send (default: KILL)
#   TUMULT_KAFKA_DIR   - Kafka install directory (default: /opt/kafka)
set -eu

. "$(dirname "$0")/../../lib/validate.sh"

SIGNAL="${TUMULT_SIGNAL:-KILL}"
KAFKA_DIR="${TUMULT_KAFKA_DIR:-/opt/kafka}"
# Documented as optional (only relevant when driving the kill over SSH) — give
# it a default so `set -u` does not abort on the unset read below.
BROKER_HOST="${TUMULT_BROKER_HOST:-}"

# Signal names/numbers: letters and digits only (KILL, SIGTERM, 9).
case "${SIGNAL}" in
    ''|*[!a-zA-Z0-9]*)
        echo "error: TUMULT_SIGNAL contains invalid characters: '${SIGNAL}'" >&2
        echo "  allowed: letters, digits" >&2
        exit 1
        ;;
esac

# Broker host: hostname or IP characters only.
if [ -n "${BROKER_HOST}" ]; then
    case "${BROKER_HOST}" in
        *[!a-zA-Z0-9.-]*)
            echo "error: TUMULT_BROKER_HOST contains invalid characters: '${BROKER_HOST}'" >&2
            echo "  allowed: letters, digits, '.', '-'" >&2
            exit 1
            ;;
    esac
fi

if [ -n "${BROKER_HOST}" ]; then
    echo "killing Kafka broker on ${BROKER_HOST}"
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
