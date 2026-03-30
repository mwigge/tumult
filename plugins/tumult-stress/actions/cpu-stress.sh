#!/bin/sh
# CPU stress injection via stress-ng
#
# Environment variables:
#   TUMULT_WORKERS  - Number of CPU workers (default: number of CPUs)
#   TUMULT_TIMEOUT  - Duration in seconds (default: 30)
#   TUMULT_LOAD     - CPU load percentage 0-100 (default: 100)
set -e

WORKERS="${TUMULT_WORKERS:-0}"
TIMEOUT="${TUMULT_TIMEOUT:-30}"
LOAD="${TUMULT_LOAD:-100}"

if ! command -v stress-ng >/dev/null 2>&1; then
    echo "error: stress-ng not found. Install with: apt install stress-ng" >&2
    exit 1
fi

echo "injecting CPU stress: workers=${WORKERS} load=${LOAD}% timeout=${TIMEOUT}s"
stress-ng --cpu "${WORKERS}" --cpu-load "${LOAD}" --timeout "${TIMEOUT}s" --metrics-brief 2>&1
echo "cpu stress completed"
