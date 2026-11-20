#!/bin/sh
# Probe: wall-clock offset in seconds between a target and the runner.
# Outputs a single integer (target_epoch - runner_epoch). ~0 when clocks agree.
# Fails when docker exec fails or the target answers no epoch in seconds.
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

# A failed docker exec ends the probe with the status and the message of
# docker. An answer that is not an epoch is no measurement either.
TGT="$(docker exec "${TARGET}" sh -c "${TARGET_CMD}")"
case "${TGT}" in
    ''|*[!0-9]*)
        echo "error: ${TARGET} answered '${TGT}' to '${TARGET_CMD}', not an epoch in seconds" >&2
        exit 1
        ;;
esac

echo "$(( TGT - RUNNER ))"
