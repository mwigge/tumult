#!/bin/sh
# Probe: count under-replicated partitions
# Outputs: integer count (0 = healthy). A failed query exits non-zero with
# the message of the tool.
#
# Environment variables:
#   TUMULT_KAFKA_BOOTSTRAP - Bootstrap servers (default: localhost:9092)
#   TUMULT_KAFKA_DIR       - Kafka install directory (default: /opt/kafka)
set -eu

. "$(dirname "$0")/../../lib/validate.sh"

BOOTSTRAP="${TUMULT_KAFKA_BOOTSTRAP:-localhost:9092}"
KAFKA_DIR="${TUMULT_KAFKA_DIR:-/opt/kafka}"

# Bootstrap servers: host:port list — hostname, IP, port, and separator chars only.
case "${BOOTSTRAP}" in
    ''|*[!a-zA-Z0-9.,:_-]*)
        echo "error: TUMULT_KAFKA_BOOTSTRAP contains invalid characters: '${BOOTSTRAP}'" >&2
        echo "  allowed: letters, digits, '.', ',', ':', '_', '-'" >&2
        exit 1
        ;;
esac

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

# The query runs on its own first, so a failed query ends the probe with
# the status and the message of the tool. Each partition is one line.
PARTITIONS=$("${CMD}" --bootstrap-server "${BOOTSTRAP}" --describe --under-replicated-partitions)
printf '%s\n' "${PARTITIONS}" | awk '/Topic:/ { count++ } END { print count+0 }'
