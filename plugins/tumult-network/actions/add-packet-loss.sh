#!/bin/sh
# Add packet loss using tc netem
# Requires root/sudo. Linux only.
#
# Environment variables:
#   TUMULT_INTERFACE   - Network interface (default: eth0)
#   TUMULT_LOSS_PCT    - Packet loss percentage (default: 10)
#   TUMULT_CORRELATION  - Loss correlation percentage (default: 25)
set -e

INTERFACE="${TUMULT_INTERFACE:-eth0}"
LOSS="${TUMULT_LOSS_PCT:-10}"
CORRELATION="${TUMULT_CORRELATION:-25}"

if [ "$(uname -s)" != "Linux" ]; then
    echo "error: tc netem requires Linux" >&2
    exit 1
fi

echo "adding ${LOSS}% packet loss (${CORRELATION}% correlation) on ${INTERFACE}"
tc qdisc add dev "${INTERFACE}" root netem loss "${LOSS}%" "${CORRELATION}%"
echo "packet loss injection active"
