#!/bin/sh
# Probe: measure round-trip latency to a host
# Outputs: latency in milliseconds (float). A host that does not answer fails
# the probe with the message of ping.
#
# Environment variables:
#   TUMULT_TARGET_HOST - Host to ping (required)
#   TUMULT_PING_COUNT  - Number of pings (default: 3)
set -eu

TARGET="${TUMULT_TARGET_HOST:?TUMULT_TARGET_HOST is required}"
COUNT="${TUMULT_PING_COUNT:-3}"

# The summary line of ping, "rtt min/avg/max/mdev = 0.045/0.052/0.061/0.007 ms"
# on Linux and "round-trip min/avg/max/stddev = ..." on macOS.
OS="$(uname -s)"
case "${OS}" in
    Linux)
        SUMMARY='/^rtt/'
        ;;
    Darwin)
        SUMMARY='/round-trip/'
        ;;
    *)
        echo "error: unsupported OS: ${OS}" >&2
        exit 1
        ;;
esac

# The ping runs on its own first, so a host that does not answer ends the
# probe with the status of ping, and what ping printed goes to stderr.
OUTPUT=$(ping -c "${COUNT}" -q "${TARGET}") || {
    STATUS=$?
    printf '%s\n' "${OUTPUT}" >&2
    exit "${STATUS}"
}

# A ping without a summary line measured nothing either.
LATENCY=$(printf '%s\n' "${OUTPUT}" | awk -F'/' "${SUMMARY} { print \$5 }")
if [ -z "${LATENCY}" ]; then
    echo "error: ping ${TARGET} printed no round-trip time" >&2
    printf '%s\n' "${OUTPUT}" >&2
    exit 1
fi

echo "${LATENCY}"
