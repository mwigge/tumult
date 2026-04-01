#!/bin/sh
# tumult-pumba: Get container network interface packet counters.
#
# Reports TX/RX packets, errors, and drops from inside the container.
# Useful for detecting the effects of netem chaos actions.
#
# Environment variables:
#   TUMULT_CONTAINER — target container name or ID (required)
#   TUMULT_INTERFACE — network interface to check (default: eth0)

set -eu

CONTAINER="${TUMULT_CONTAINER:?error: TUMULT_CONTAINER is required}"
INTERFACE="${TUMULT_INTERFACE:-eth0}"

# Read /proc/net/dev inside the container for the target interface
STATS=$(docker exec "${CONTAINER}" sh -c "cat /proc/net/dev" 2>&1) || {
    echo "error: cannot read /proc/net/dev in ${CONTAINER}"
    exit 1
}

# Parse the interface line: face |bytes packets errs drop fifo frame compressed multicast|...
LINE=$(echo "${STATS}" | grep "${INTERFACE}:" | sed "s/${INTERFACE}://")

if [ -z "${LINE}" ]; then
    echo "error: interface ${INTERFACE} not found in ${CONTAINER}"
    exit 1
fi

# Extract fields: rx_bytes rx_packets rx_errs rx_drop ... tx_bytes tx_packets tx_errs tx_drop
set -- ${LINE}
RX_BYTES=$1; RX_PACKETS=$2; RX_ERRS=$3; RX_DROP=$4
shift 8
TX_BYTES=$1; TX_PACKETS=$2; TX_ERRS=$3; TX_DROP=$4

echo "{\"rx_packets\":${RX_PACKETS},\"rx_errors\":${RX_ERRS},\"rx_drops\":${RX_DROP},\"tx_packets\":${TX_PACKETS},\"tx_errors\":${TX_ERRS},\"tx_drops\":${TX_DROP}}"
