#!/bin/sh
# Probe: available kernel entropy in bits.
# Outputs a single integer (contents of /proc/sys/kernel/random/entropy_avail).
#
# Note: on modern kernels (>= 5.6) the CRNG never blocks and this value is
# effectively a constant (~256). It still confirms the interface is readable
# and is the correct signal on older kernels where entropy truly depletes.
#
# Environment variables:
#   TUMULT_TARGET - optional container to read from via docker exec (default: local)
set -eu

TARGET="${TUMULT_TARGET:-}"
PATH_AVAIL="/proc/sys/kernel/random/entropy_avail"

if [ -n "${TARGET}" ]; then
    docker exec "${TARGET}" cat "${PATH_AVAIL}"
else
    cat "${PATH_AVAIL}"
fi
