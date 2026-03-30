#!/bin/sh
# Add packet corruption using tc netem
# Requires root/sudo. Linux only.
#
# Environment variables:
#   TUMULT_INTERFACE      - Network interface (default: eth0)
#   TUMULT_CORRUPTION_PCT - Corruption percentage (default: 5)
set -e

. "$(dirname "$0")/../../lib/validate.sh"

INTERFACE="${TUMULT_INTERFACE:-eth0}"
CORRUPTION="${TUMULT_CORRUPTION_PCT:-5}"

validate_number "TUMULT_CORRUPTION_PCT" "${CORRUPTION}"

if [ "$(uname -s)" != "Linux" ]; then
    echo "error: tc netem requires Linux" >&2
    exit 1
fi

echo "adding ${CORRUPTION}% packet corruption on ${INTERFACE}"
tc qdisc add dev "${INTERFACE}" root netem corrupt "${CORRUPTION}%"
echo "packet corruption active"
