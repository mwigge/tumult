#!/bin/sh
# Remove all tc netem rules — rollback action
#
# Environment variables:
#   TUMULT_INTERFACE  - Network interface (default: eth0)
set -e

INTERFACE="${TUMULT_INTERFACE:-eth0}"

if [ "$(uname -s)" != "Linux" ]; then
    echo "error: tc netem requires Linux" >&2
    exit 1
fi

echo "removing tc rules on ${INTERFACE}"
tc qdisc del dev "${INTERFACE}" root 2>/dev/null || true
echo "tc rules cleared on ${INTERFACE}"
