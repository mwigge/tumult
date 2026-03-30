#!/bin/sh
# Stop a running k6 load test
#
# Environment variables:
#   TUMULT_K6_PIDFILE - PID file location (default: /tmp/tumult-k6.pid)
set -eu

PIDFILE="${TUMULT_K6_PIDFILE:-/tmp/tumult-k6.pid}"

if [ ! -f "${PIDFILE}" ]; then
    echo "warning: no k6 PID file found at ${PIDFILE}" >&2
    exit 0
fi

PID=$(cat "${PIDFILE}")
if kill -0 "${PID}" 2>/dev/null; then
    echo "stopping k6 (PID: ${PID})"
    kill -TERM "${PID}"
    # Wait up to 10 seconds for graceful shutdown
    i=0
    while [ "$i" -lt 10 ] && kill -0 "${PID}" 2>/dev/null; do
        sleep 1
        i=$((i + 1))
    done
    if kill -0 "${PID}" 2>/dev/null; then
        kill -KILL "${PID}" 2>/dev/null || true
    fi
    echo "k6 stopped"
else
    echo "k6 process ${PID} already exited"
fi

rm -f "${PIDFILE}"
