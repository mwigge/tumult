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

# The pidfile lives in world-writable /tmp — never signal a PID that cannot
# be identified as k6 (PID reuse or a planted file).
case "${PID}" in
    ''|*[!0-9]*)
        echo "warning: ${PIDFILE} does not contain a numeric PID ('${PID}'); removing stale file" >&2
        rm -f "${PIDFILE}"
        exit 0
        ;;
esac

if [ "$(uname -s)" = "Linux" ]; then
    if [ ! -d "/proc/${PID}" ]; then
        echo "k6 process ${PID} already exited"
        rm -f "${PIDFILE}"
        exit 0
    fi
    # Identity check: /proc/<pid>/cmdline must mention k6, otherwise the
    # pidfile is stale (or planted) and the current holder of the PID is an
    # unrelated process we must not signal.
    if ! tr '\0' ' ' < "/proc/${PID}/cmdline" 2>/dev/null | grep -q "k6"; then
        echo "warning: PID ${PID} from ${PIDFILE} is not a k6 process; refusing to kill, removing stale pidfile" >&2
        rm -f "${PIDFILE}"
        exit 0
    fi
fi

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
