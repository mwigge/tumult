#!/bin/sh
# Probe: check if a process is running
# Outputs: "true" or "false"
#
# Environment variables:
#   TUMULT_PID       - Process ID to check (optional, preferred)
#   TUMULT_NAME      - Process name to check via pgrep (optional)
#   TUMULT_PATTERN   - Process pattern to check via pgrep -f (optional)
set -eu

if [ -n "${TUMULT_PID}" ]; then
    if kill -0 "${TUMULT_PID}" 2>/dev/null; then
        echo "true"
    else
        echo "false"
    fi
elif [ -n "${TUMULT_NAME}" ]; then
    if pgrep "${TUMULT_NAME}" >/dev/null 2>&1; then
        echo "true"
    else
        echo "false"
    fi
elif [ -n "${TUMULT_PATTERN}" ]; then
    if pgrep -f "${TUMULT_PATTERN}" >/dev/null 2>&1; then
        echo "true"
    else
        echo "false"
    fi
else
    echo "error: one of TUMULT_PID, TUMULT_NAME, or TUMULT_PATTERN is required" >&2
    exit 1
fi
