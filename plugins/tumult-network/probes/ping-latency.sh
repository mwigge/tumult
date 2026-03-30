#!/bin/sh
# Probe: measure round-trip latency to a host
# Outputs: latency in milliseconds (float)
#
# Environment variables:
#   TUMULT_TARGET_HOST - Host to ping (required)
#   TUMULT_PING_COUNT  - Number of pings (default: 3)
set -e

TARGET="${TUMULT_TARGET_HOST:?TUMULT_TARGET_HOST is required}"
COUNT="${TUMULT_PING_COUNT:-3}"

OS="$(uname -s)"
case "${OS}" in
    Linux)
        ping -c "${COUNT}" -q "${TARGET}" 2>/dev/null | awk -F'/' '/^rtt/ { print $5 }'
        ;;
    Darwin)
        ping -c "${COUNT}" -q "${TARGET}" 2>/dev/null | awk -F'/' '/round-trip/ { print $5 }'
        ;;
    *)
        echo "error: unsupported OS: ${OS}" >&2
        exit 1
        ;;
esac
