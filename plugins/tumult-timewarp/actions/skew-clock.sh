#!/bin/sh
# skew-clock — shift a process's perceived wall-clock forward/back by N seconds.
#
# Mechanism & limits
# --------------------------
# Containers in docker-compose share the HOST kernel clock. Linux time
# namespaces virtualize only CLOCK_MONOTONIC / CLOCK_BOOTTIME, NOT
# CLOCK_REALTIME (wall clock), and `date -s` needs CAP_SYS_TIME and would
# move the *host* clock (unsafe, usually denied). The realistic lever
# for per-target wall-clock skew is therefore libfaketime (LD_PRELOAD): it
# shifts the perceived time of the *process it wraps only*, without touching
# the host or other processes. That is what this action does.
#
# It launches a command under `faketime "<offset>"` and prints the command's
# perceived time. Point it at a real workload (TUMULT_FAKETIME_CMD) to make
# that workload believe the clock has moved. To skew a process INSIDE another
# container, set TUMULT_TARGET (the target must have libfaketime installed).
#
# Environment variables:
#   TUMULT_SKEW_SECONDS   - Offset in seconds; may be negative (default: 3600)
#   TUMULT_FAKETIME_CMD   - Command to run under skew (default: "date -u +%s")
#   TUMULT_TARGET         - Optional container to docker exec into (default: local)
#
# Exit codes: 0 = skew applied and command ran; 1 = faketime unavailable / error.
set -eu

. "$(dirname "$0")/../../lib/validate.sh"

SKEW="${TUMULT_SKEW_SECONDS:-3600}"
CMD="${TUMULT_FAKETIME_CMD:-date -u +%s}"
TARGET="${TUMULT_TARGET:-}"

# Allow a leading minus for backward skew; validate the magnitude as integer.
MAG="${SKEW#-}"
validate_integer "TUMULT_SKEW_SECONDS" "${MAG}"

# libfaketime accepts relative offsets like "+3600s" / "-120s".
case "${SKEW}" in
    -*) OFFSET="-${MAG}s" ;;
    *)  OFFSET="+${MAG}s" ;;
esac

run() {
    # $1 = shell command string to run under faketime
    if [ -n "${TARGET}" ]; then
        if ! command -v docker >/dev/null 2>&1; then
            echo "error: docker CLI not found (needed for TUMULT_TARGET=${TARGET})" >&2
            exit 1
        fi
        docker exec "${TARGET}" sh -c "command -v faketime >/dev/null 2>&1 || { echo 'error: faketime not installed in ${TARGET}' >&2; exit 1; }; faketime '${OFFSET}' $1"
    else
        if ! command -v faketime >/dev/null 2>&1; then
            echo "error: faketime (libfaketime) not found on the runner." >&2
            echo "  install with: apt-get install -y libfaketime   (Debian/Ubuntu)" >&2
            echo "  faketime is the only safe per-process wall-clock lever in a" >&2
            echo "  shared-kernel container; see the plugin README." >&2
            exit 1
        fi
        faketime "${OFFSET}" sh -c "$1"
    fi
}

echo "timewarp: applying wall-clock skew ${OFFSET} to '${CMD}'${TARGET:+ in ${TARGET}}"
REAL_NOW="$(date -u +%s)"
PERCEIVED="$(run "${CMD}")"
echo "timewarp: real_epoch=${REAL_NOW} perceived_output=${PERCEIVED}"
echo "timewarp: clock skew applied (per-process, faketime)"
