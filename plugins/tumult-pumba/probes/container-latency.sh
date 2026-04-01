#!/bin/sh
# tumult-pumba: Measure network latency from inside container.
#
# Runs ping inside the container to measure actual latency experienced
# by the containerized application (including any injected netem effects).
#
# Environment variables:
#   TUMULT_CONTAINER  — target container name or ID (required)
#   TUMULT_PING_HOST  — host to ping from inside container (default: 8.8.8.8)
#   TUMULT_PING_COUNT — number of pings (default: 3)

set -eu

. "$(dirname "$0")/../../lib/validate.sh"

CONTAINER="${TUMULT_CONTAINER:?error: TUMULT_CONTAINER is required}"
PING_HOST="${TUMULT_PING_HOST:-8.8.8.8}"
PING_COUNT="${TUMULT_PING_COUNT:-3}"

validate_integer "TUMULT_PING_COUNT" "$PING_COUNT"

# Run ping inside the target container
RESULT=$(docker exec "${CONTAINER}" ping -c "${PING_COUNT}" -W 5 "${PING_HOST}" 2>&1) || {
    echo "error: ping failed inside ${CONTAINER}"
    exit 1
}

# Extract average latency from ping summary line
# Linux:  rtt min/avg/max/mdev = 0.123/0.456/0.789/0.012 ms
# Alpine: round-trip min/avg/max = 0.123/0.456/0.789 ms
# macOS:  round-trip min/avg/max/stddev = 0.123/0.456/0.789/0.012 ms
AVG=$(echo "${RESULT}" | grep -E "min/avg/max" | sed 's|.*/||; s| .*||' | cut -d'/' -f2)

# Fallback: try extracting second number from the = X/Y/Z line
if [ -z "${AVG}" ]; then
    AVG=$(echo "${RESULT}" | grep "=" | tail -1 | sed 's|.*= *||; s| .*||' | cut -d'/' -f2)
fi

if [ -n "${AVG}" ]; then
    echo "${AVG}"
else
    echo "error: could not parse ping output"
    exit 1
fi
