#!/bin/sh
# Kill a process by PID, name, or pattern
#
# Environment variables:
#   TUMULT_PID       - Process ID to kill (optional, preferred)
#   TUMULT_NAME      - Process name to kill via pkill (optional)
#   TUMULT_PATTERN   - Process pattern to kill via pkill -f (optional)
#   TUMULT_SIGNAL    - Signal to send (default: KILL)
#
# Priority: PID > NAME > PATTERN (first match wins)
set -e

SIGNAL="${TUMULT_SIGNAL:-KILL}"

if [ -n "${TUMULT_PID}" ]; then
    echo "killing PID ${TUMULT_PID} with signal ${SIGNAL}"
    kill -s "${SIGNAL}" "${TUMULT_PID}"
    echo "process ${TUMULT_PID} killed"
elif [ -n "${TUMULT_NAME}" ]; then
    echo "killing processes named '${TUMULT_NAME}' with signal ${SIGNAL}"
    pkill -"${SIGNAL}" "${TUMULT_NAME}" || {
        echo "no processes found matching name '${TUMULT_NAME}'" >&2
        exit 1
    }
    echo "processes named '${TUMULT_NAME}' killed"
elif [ -n "${TUMULT_PATTERN}" ]; then
    echo "killing processes matching '${TUMULT_PATTERN}' with signal ${SIGNAL}"
    pkill -"${SIGNAL}" -f "${TUMULT_PATTERN}" || {
        echo "no processes found matching pattern '${TUMULT_PATTERN}'" >&2
        exit 1
    }
    echo "processes matching '${TUMULT_PATTERN}' killed"
else
    echo "error: one of TUMULT_PID, TUMULT_NAME, or TUMULT_PATTERN is required" >&2
    exit 1
fi
