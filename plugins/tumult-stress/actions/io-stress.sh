#!/bin/sh
# IO stress injection via stress-ng
#
# Environment variables:
#   TUMULT_WORKERS  - Number of IO workers (default: 1)
#   TUMULT_TIMEOUT  - Duration in seconds (default: 30)
#   TUMULT_HDD_BYTES - Bytes per write operation (default: 1g)
set -e

WORKERS="${TUMULT_WORKERS:-1}"
TIMEOUT="${TUMULT_TIMEOUT:-30}"
HDD_BYTES="${TUMULT_HDD_BYTES:-1g}"

if ! command -v stress-ng >/dev/null 2>&1; then
    echo "error: stress-ng not found. Install with: apt install stress-ng" >&2
    exit 1
fi

echo "injecting IO stress: workers=${WORKERS} hdd_bytes=${HDD_BYTES} timeout=${TIMEOUT}s"
stress-ng --hdd "${WORKERS}" --hdd-bytes "${HDD_BYTES}" --timeout "${TIMEOUT}s" --metrics-brief 2>&1
echo "io stress completed"
