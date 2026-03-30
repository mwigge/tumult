#!/bin/sh
# Memory stress injection via stress-ng
#
# Environment variables:
#   TUMULT_WORKERS  - Number of memory workers (default: 1)
#   TUMULT_TIMEOUT  - Duration in seconds (default: 30)
#   TUMULT_BYTES    - Memory per worker (default: 256m)
set -e

WORKERS="${TUMULT_WORKERS:-1}"
TIMEOUT="${TUMULT_TIMEOUT:-30}"
BYTES="${TUMULT_BYTES:-256m}"

if ! command -v stress-ng >/dev/null 2>&1; then
    echo "error: stress-ng not found. Install with: apt install stress-ng" >&2
    exit 1
fi

echo "injecting memory stress: workers=${WORKERS} bytes=${BYTES} timeout=${TIMEOUT}s"
stress-ng --vm "${WORKERS}" --vm-bytes "${BYTES}" --timeout "${TIMEOUT}s" --metrics-brief 2>&1
echo "memory stress completed"
