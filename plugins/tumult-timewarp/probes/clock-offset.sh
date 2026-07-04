#!/bin/sh
# Probe: wall-clock offset in seconds between a target and the runner.
# Outputs a single integer (target_epoch - runner_epoch). ~0 when clocks agree.
#
# Because containers share the host kernel clock, this normally reads 0 in
# docker-compose (proving no unintended host skew). It becomes non-zero only
# when the target process's perceived time is shifted (e.g. a faketime-wrapped
# command via skew-clock), which is exactly the signal to assert on.
#
# Environment variables:
#   TUMULT_TARGET       - container to compare against (default: none -> reads 0)
#   TUMULT_TARGET_CMD   - command giving target epoch (default: "date -u +%s")
set -eu

TARGET="${TUMULT_TARGET:-}"
TARGET_CMD="${TUMULT_TARGET_CMD:-date -u +%s}"

RUNNER="$(date -u +%s)"

if [ -z "${TARGET}" ]; then
    echo "0"
    exit 0
fi

TGT="$(docker exec "${TARGET}" sh -c "${TARGET_CMD}" 2>/dev/null || echo "${RUNNER}")"
case "${TGT}" in
    ''|*[!0-9]*) TGT="${RUNNER}" ;;
esac

echo "$(( TGT - RUNNER ))"
