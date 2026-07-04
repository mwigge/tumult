#!/bin/sh
# rng-pressure — sustain crypto/RNG load to stress the CRNG + crypto path.
#
# Mechanism & honest limits
# --------------------------
# Spawns N background workers that hammer the userspace crypto RNG by
# generating random bytes in a tight loop (openssl rand when available, else
# reads of /dev/urandom). This contends for the CRNG and for CPU, which is the
# realistic way "entropy/crypto skew" manifests today: not blocking on entropy,
# but degraded throughput/latency for RNG-dependent crypto (TLS handshakes, key
# generation). Measure the effect with the crypto-throughput probe. Honest
# limit: this is contention-driven slowdown, not entropy-pool depletion.
#
# Workers self-terminate after TUMULT_RNG_DURATION. PIDs go to a pidfile so
# stop-entropy-drain can roll both this and entropy-drain back together.
#
# Environment variables:
#   TUMULT_RNG_WORKERS  - number of workers (default: 4)
#   TUMULT_RNG_DURATION - seconds each worker runs before self-exit (default: 60)
#   TUMULT_TW_STATE_DIR - state/pidfile dir (default: /tmp/tumult-timewarp)
#
# Exit codes: 0 = workers started.
set -eu

. "$(dirname "$0")/../../lib/validate.sh"

WORKERS="${TUMULT_RNG_WORKERS:-4}"
DURATION="${TUMULT_RNG_DURATION:-60}"
STATE_DIR="${TUMULT_TW_STATE_DIR:-/tmp/tumult-timewarp}"

validate_integer "TUMULT_RNG_WORKERS" "${WORKERS}"
validate_integer "TUMULT_RNG_DURATION" "${DURATION}"

mkdir -p "${STATE_DIR}"
PIDFILE="${STATE_DIR}/entropy-drain.pids"   # shared with entropy-drain for unified rollback
touch "${PIDFILE}"

if command -v openssl >/dev/null 2>&1; then
    MODE="openssl rand"
    LOOP='while :; do openssl rand 1048576 >/dev/null 2>&1 || exit 0; done'
else
    MODE="/dev/urandom"
    LOOP='while :; do dd if=/dev/urandom of=/dev/null bs=1M count=1 >/dev/null 2>&1 || exit 0; done'
fi

# Each worker runs in its own process group (setsid) and records its leader
# PID (== PGID), so stop-entropy-drain kills the whole tree as a unit. timeout
# is the safety net that stops the worker even without an explicit rollback.
# Values pass via env to keep quoting simple; $0 carries the pidfile path.
export TW_DURATION="${DURATION}" TW_LOOP="${LOOP}"

echo "timewarp: applying RNG pressure — ${WORKERS} worker(s) via ${MODE} for up to ${DURATION}s"
i=0
while [ "${i}" -lt "${WORKERS}" ]; do
    setsid sh -c 'echo $$ >> "$0"; exec timeout "$TW_DURATION" sh -c "$TW_LOOP"' "${PIDFILE}" >/dev/null 2>&1 &
    i=$(( i + 1 ))
done

echo "timewarp: rng-pressure active (pids in ${PIDFILE})"
