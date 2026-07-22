#!/bin/sh
# Start a k6 load test as a background process
#
# Environment variables:
#   TUMULT_K6_SCRIPT    - Path to k6 test script (required)
#   TUMULT_K6_VUS       - Number of virtual users (default: 10)
#   TUMULT_K6_DURATION  - Test duration (default: 30s)
#   TUMULT_K6_BINARY    - Path to k6 binary (default: k6)
#   TUMULT_K6_PIDFILE   - PID file location (default: /tmp/tumult-k6.pid)
#   TUMULT_K6_OUT       - Output format (default: json=/tmp/tumult-k6-results.json)
#   TUMULT_OTEL_ENDPOINT - If set, k6 exports OTLP metrics for trace correlation
set -eu

. "$(dirname "$0")/../../lib/validate.sh"

SCRIPT="${TUMULT_K6_SCRIPT:?TUMULT_K6_SCRIPT is required}"
VUS="${TUMULT_K6_VUS:-10}"
DURATION="${TUMULT_K6_DURATION:-30s}"
K6="${TUMULT_K6_BINARY:-k6}"
PIDFILE="${TUMULT_K6_PIDFILE:-/tmp/tumult-k6.pid}"
# The `:-` default also covers an explicitly empty TUMULT_K6_OUT, so OUT can
# never reach --out empty.
OUT="${TUMULT_K6_OUT:-json=/tmp/tumult-k6-results.json}"

# VUS is passed to --vus and must be a positive integer.
validate_integer "TUMULT_K6_VUS" "${VUS}"

# Duration must be a k6 duration such as 30s, 5m, 1h30m — digits plus the
# h/m/s unit letters only, so nothing can word-split or inject extra args.
case "${DURATION}" in
    ''|*[!0-9hms]*)
        echo "error: TUMULT_K6_DURATION must be a k6 duration like 30s, 5m, 1h30m, got: '${DURATION}'" >&2
        exit 1
        ;;
esac

if ! command -v "${K6}" >/dev/null 2>&1; then
    echo "error: k6 not found (install from https://k6.io)" >&2
    exit 1
fi

if [ ! -f "${SCRIPT}" ]; then
    echo "error: k6 script not found: ${SCRIPT}" >&2
    exit 1
fi

# Build the k6 argv as discrete quoted words (never an unquoted string,
# which would word-split and allow argument injection).
set -- run --vus "${VUS}" --duration "${DURATION}" --out "${OUT}"

# Add OTLP export if endpoint is configured
if [ -n "${TUMULT_OTEL_ENDPOINT:-}" ]; then
    set -- "$@" --out experimental-opentelemetry
    export K6_OTEL_EXPORTER_OTLP_ENDPOINT="${TUMULT_OTEL_ENDPOINT}"
fi

K6_PID=""
cleanup() { [ -n "${K6_PID}" ] && kill "${K6_PID}" 2>/dev/null; rm -f "${PIDFILE}"; }
trap cleanup INT TERM

echo "starting k6: ${VUS} VUs, ${DURATION}, script=${SCRIPT}"
"${K6}" "$@" "${SCRIPT}" &
K6_PID=$!
echo "${K6_PID}" > "${PIDFILE}"
echo "k6 started (PID: ${K6_PID})"
