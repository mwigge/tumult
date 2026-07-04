#!/bin/sh
# stop-entropy-drain — rollback for entropy-drain / rng-pressure.
#
# entropy-drain and rng-pressure each launch their workers with setsid, so the
# pidfile holds process-group leader PIDs (== PGIDs). We kill each whole group
# with `kill -TERM -<pgid>`, which tears down the timeout wrapper AND the reader
# together — no orphans, no re-spawn race. Safe to run repeatedly and safe when
# no workers are active (always exits 0).
#
# Environment variables:
#   TUMULT_TW_STATE_DIR - state/pidfile dir (default: /tmp/tumult-timewarp)
set -eu

STATE_DIR="${TUMULT_TW_STATE_DIR:-/tmp/tumult-timewarp}"
PIDFILE="${STATE_DIR}/entropy-drain.pids"

killed=0
if [ -f "${PIDFILE}" ]; then
    while IFS= read -r pgid; do
        [ -n "${pgid}" ] || continue
        case "${pgid}" in *[!0-9]*) continue ;; esac
        # Negative PID targets the whole process group (leader + children).
        if kill -TERM "-${pgid}" 2>/dev/null; then
            killed=$(( killed + 1 ))
        else
            # Group already gone (timeout fired); reap the leader if it lingers.
            kill -TERM "${pgid}" 2>/dev/null || true
        fi
    done < "${PIDFILE}"
    rm -f "${PIDFILE}"
fi

echo "timewarp: entropy pressure stopped (terminated ${killed} worker group(s))"
