#!/bin/sh
# entropy-drain — apply read pressure on the kernel RNG (/dev/random).
#
# Mechanism & limits
# --------------------------
# Spawns N background workers that continuously read from /dev/random. On
# kernels < 5.6 this genuinely drains the blocking pool and drops
# entropy_avail; on modern kernels (>= 5.6, and any container on a current
# host) the CRNG never blocks and entropy_avail is effectively a constant
# (~256), so this becomes RNG *read/CPU* pressure rather than true depletion.
# That is the actual state of the world — /dev/random no longer blocks — and
# is documented in the plugin README. Pair with the crypto-throughput probe to
# observe the real, measurable effect (slower crypto ops under contention).
#
# Workers self-terminate after TUMULT_DRAIN_DURATION as a safety net even if
# rollback never runs. PIDs are written to a pidfile for stop-entropy-drain.
#
# Environment variables:
#   TUMULT_DRAIN_WORKERS  - number of reader workers (default: 4)
#   TUMULT_DRAIN_DURATION - seconds each worker runs before self-exit (default: 60)
#   TUMULT_TW_STATE_DIR   - state/pidfile dir (default: /tmp/tumult-timewarp)
#
# Exit codes: 0 = workers started.
set -eu

. "$(dirname "$0")/../../lib/validate.sh"

WORKERS="${TUMULT_DRAIN_WORKERS:-4}"
DURATION="${TUMULT_DRAIN_DURATION:-60}"
STATE_DIR="${TUMULT_TW_STATE_DIR:-/tmp/tumult-timewarp}"

validate_integer "TUMULT_DRAIN_WORKERS" "${WORKERS}"
validate_integer "TUMULT_DRAIN_DURATION" "${DURATION}"

mkdir -p "${STATE_DIR}"
PIDFILE="${STATE_DIR}/entropy-drain.pids"
touch "${PIDFILE}"

# Each worker runs in its own process group (setsid) and records its leader
# PID (== PGID) in the shared pidfile, so stop-entropy-drain can kill the whole
# worker tree — timeout + reader — as a unit. `timeout` self-terminates the
# worker even if rollback never runs. Values pass via env to keep quoting
# simple; $0 carries the pidfile path.
export TW_DURATION="${DURATION}"

echo "timewarp: draining entropy — ${WORKERS} worker(s) reading /dev/random for up to ${DURATION}s"
i=0
while [ "${i}" -lt "${WORKERS}" ]; do
    setsid sh -c 'echo $$ >> "$0"; exec timeout "$TW_DURATION" dd if=/dev/random of=/dev/null bs=1 count=100000000 2>/dev/null' "${PIDFILE}" >/dev/null 2>&1 &
    i=$(( i + 1 ))
done

echo "timewarp: entropy-drain active (pids in ${PIDFILE})"
echo "timewarp: entropy_avail=$(cat /proc/sys/kernel/random/entropy_avail 2>/dev/null || echo NA)"
