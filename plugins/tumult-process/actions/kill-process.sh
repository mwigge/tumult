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
set -eu

SIGNAL="${TUMULT_SIGNAL:-KILL}"

# Validate signal name
case "${SIGNAL}" in
    HUP|INT|QUIT|ABRT|KILL|TERM|STOP|CONT|USR1|USR2|ALRM|PIPE|SEGV|0|1|2|3|6|9|14|15|17|18|19|23) ;;
    *) echo "error: invalid signal: ${SIGNAL}" >&2; exit 1 ;;
esac

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
