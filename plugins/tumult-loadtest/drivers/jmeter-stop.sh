#!/bin/sh
# Stop a running JMeter load test
#
# Environment variables:
#   TUMULT_JMETER_PIDFILE - PID file (default: /tmp/tumult-jmeter.pid)
set -eu

PIDFILE="${TUMULT_JMETER_PIDFILE:-/tmp/tumult-jmeter.pid}"

if [ ! -f "${PIDFILE}" ]; then
    echo "warning: no JMeter PID file found at ${PIDFILE}" >&2
    exit 0
fi

PID=$(cat "${PIDFILE}")
if kill -0 "${PID}" 2>/dev/null; then
    echo "stopping JMeter (PID: ${PID})"
    kill -TERM "${PID}"
    i=0
    while [ "$i" -lt 15 ] && kill -0 "${PID}" 2>/dev/null; do
        sleep 1
        i=$((i + 1))
    done
    if kill -0 "${PID}" 2>/dev/null; then
        kill -KILL "${PID}" 2>/dev/null || true
    fi
    echo "JMeter stopped"
else
    echo "JMeter process ${PID} already exited"
fi

rm -f "${PIDFILE}"
