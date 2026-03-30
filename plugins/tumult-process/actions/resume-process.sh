#!/bin/sh
# Resume a suspended process (SIGCONT)
#
# Environment variables:
#   TUMULT_PID       - Process ID to resume (optional, preferred)
#   TUMULT_NAME      - Process name to resume via pkill (optional)
#   TUMULT_PATTERN   - Process pattern to resume via pkill -f (optional)
set -eu

if [ -n "${TUMULT_PID}" ]; then
    echo "resuming PID ${TUMULT_PID}"
    kill -CONT "${TUMULT_PID}"
    echo "process ${TUMULT_PID} resumed"
elif [ -n "${TUMULT_NAME}" ]; then
    echo "resuming processes named '${TUMULT_NAME}'"
    pkill -CONT "${TUMULT_NAME}" || {
        echo "no processes found matching name '${TUMULT_NAME}'" >&2
        exit 1
    }
    echo "processes named '${TUMULT_NAME}' resumed"
elif [ -n "${TUMULT_PATTERN}" ]; then
    echo "resuming processes matching '${TUMULT_PATTERN}'"
    pkill -CONT -f "${TUMULT_PATTERN}" || {
        echo "no processes found matching pattern '${TUMULT_PATTERN}'" >&2
        exit 1
    }
    echo "processes matching '${TUMULT_PATTERN}' resumed"
else
    echo "error: one of TUMULT_PID, TUMULT_NAME, or TUMULT_PATTERN is required" >&2
    exit 1
fi
