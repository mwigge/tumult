#!/bin/sh
# Probe: get process CPU and memory usage
# Outputs: JSON with cpu_percent and mem_percent
#
# Environment variables:
#   TUMULT_PID       - Process ID to check (optional, preferred)
#   TUMULT_NAME      - Process name to check (optional)
set -eu

if [ -n "${TUMULT_PID}" ]; then
    PID="${TUMULT_PID}"
elif [ -n "${TUMULT_NAME}" ]; then
    PID=$(pgrep -o "${TUMULT_NAME}" 2>/dev/null) || {
        echo '{"cpu_percent": 0, "mem_percent": 0, "running": false}'
        exit 0
    }
else
    echo "error: one of TUMULT_PID or TUMULT_NAME is required" >&2
    exit 1
fi

if ! kill -0 "${PID}" 2>/dev/null; then
    echo '{"cpu_percent": 0, "mem_percent": 0, "running": false}'
    exit 0
fi

OS="$(uname -s)"
case "${OS}" in
    Linux)
        ps -p "${PID}" -o %cpu=,%mem= 2>/dev/null | awk '{ printf "{\"cpu_percent\": %s, \"mem_percent\": %s, \"running\": true}", $1, $2 }'
        ;;
    Darwin)
        ps -p "${PID}" -o %cpu=,%mem= 2>/dev/null | awk '{ printf "{\"cpu_percent\": %s, \"mem_percent\": %s, \"running\": true}", $1, $2 }'
        ;;
    *)
        echo "error: unsupported OS: ${OS}" >&2
        exit 1
        ;;
esac
