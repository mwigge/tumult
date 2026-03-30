#!/bin/sh
# CPU stress injection via stress-ng
#
# Environment variables:
#   TUMULT_WORKERS  - Number of CPU workers (default: number of CPUs)
#   TUMULT_TIMEOUT  - Duration in seconds (default: 30)
#   TUMULT_LOAD     - CPU load percentage 0-100 (default: 100)
set -eu

. "$(dirname "$0")/../../lib/validate.sh"

WORKERS="${TUMULT_WORKERS:-0}"
TIMEOUT="${TUMULT_TIMEOUT:-30}"
LOAD="${TUMULT_LOAD:-100}"

validate_integer "TUMULT_WORKERS" "${WORKERS}"
validate_integer "TUMULT_TIMEOUT" "${TIMEOUT}"
validate_integer "TUMULT_LOAD" "${LOAD}"

if ! command -v stress-ng >/dev/null 2>&1; then
    echo "error: stress-ng not found. Install with: apt install stress-ng" >&2
    exit 1
fi

echo "injecting CPU stress: workers=${WORKERS} load=${LOAD}% timeout=${TIMEOUT}s"
stress-ng --cpu "${WORKERS}" --cpu-load "${LOAD}" --timeout "${TIMEOUT}s" --metrics-brief 2>&1
echo "cpu stress completed"
