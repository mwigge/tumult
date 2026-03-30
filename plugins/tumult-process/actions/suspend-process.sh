#!/bin/sh
# Suspend a process (SIGSTOP)
#
# Environment variables:
#   TUMULT_PID       - Process ID to suspend (optional, preferred)
#   TUMULT_NAME      - Process name to suspend via pkill (optional)
#   TUMULT_PATTERN   - Process pattern to suspend via pkill -f (optional)
set -e

if [ -n "${TUMULT_PID}" ]; then
    echo "suspending PID ${TUMULT_PID}"
    kill -STOP "${TUMULT_PID}"
    echo "process ${TUMULT_PID} suspended"
elif [ -n "${TUMULT_NAME}" ]; then
    echo "suspending processes named '${TUMULT_NAME}'"
    pkill -STOP "${TUMULT_NAME}" || {
        echo "no processes found matching name '${TUMULT_NAME}'" >&2
        exit 1
    }
    echo "processes named '${TUMULT_NAME}' suspended"
elif [ -n "${TUMULT_PATTERN}" ]; then
    echo "suspending processes matching '${TUMULT_PATTERN}'"
    pkill -STOP -f "${TUMULT_PATTERN}" || {
        echo "no processes found matching pattern '${TUMULT_PATTERN}'" >&2
        exit 1
    }
    echo "processes matching '${TUMULT_PATTERN}' suspended"
else
    echo "error: one of TUMULT_PID, TUMULT_NAME, or TUMULT_PATTERN is required" >&2
    exit 1
fi
