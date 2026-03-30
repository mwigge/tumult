#!/bin/sh
# Combined CPU + memory + IO stress injection via stress-ng
#
# Environment variables:
#   TUMULT_CPU_WORKERS - CPU workers (default: 2)
#   TUMULT_CPU_LOAD    - CPU load percentage (default: 80)
#   TUMULT_VM_WORKERS  - Memory workers (default: 1)
#   TUMULT_VM_BYTES    - Memory per worker (default: 256m)
#   TUMULT_HDD_WORKERS - IO workers (default: 1)
#   TUMULT_TIMEOUT     - Duration in seconds (default: 30)
set -eu

CPU_WORKERS="${TUMULT_CPU_WORKERS:-2}"
CPU_LOAD="${TUMULT_CPU_LOAD:-80}"
VM_WORKERS="${TUMULT_VM_WORKERS:-1}"
VM_BYTES="${TUMULT_VM_BYTES:-256m}"
HDD_WORKERS="${TUMULT_HDD_WORKERS:-1}"
TIMEOUT="${TUMULT_TIMEOUT:-30}"

if ! command -v stress-ng >/dev/null 2>&1; then
    echo "error: stress-ng not found. Install with: apt install stress-ng" >&2
    exit 1
fi

echo "injecting combined stress: cpu=${CPU_WORKERS}@${CPU_LOAD}% mem=${VM_WORKERS}x${VM_BYTES} io=${HDD_WORKERS} timeout=${TIMEOUT}s"
stress-ng \
    --cpu "${CPU_WORKERS}" --cpu-load "${CPU_LOAD}" \
    --vm "${VM_WORKERS}" --vm-bytes "${VM_BYTES}" \
    --hdd "${HDD_WORKERS}" \
    --timeout "${TIMEOUT}s" --metrics-brief 2>&1
echo "combined stress completed"
