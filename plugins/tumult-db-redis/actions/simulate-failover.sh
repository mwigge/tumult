#!/bin/sh
# Simulate Redis hang using DEBUG SLEEP
#
# Environment variables:
#   TUMULT_REDIS_HOST - Redis host (default: localhost)
#   TUMULT_REDIS_PORT - Redis port (default: 6379)
#   TUMULT_REDIS_AUTH - AUTH password (optional)
#   TUMULT_DURATION   - Sleep duration in seconds (default: 5)
set -eu

. "$(dirname "$0")/../../lib/validate.sh"

HOST="${TUMULT_REDIS_HOST:-localhost}"
PORT="${TUMULT_REDIS_PORT:-6379}"
DURATION="${TUMULT_DURATION:-5}"

# DEBUG SLEEP takes an integer second count — reject anything else before it
# reaches redis-cli.
validate_integer "TUMULT_DURATION" "${DURATION}"

if ! command -v redis-cli >/dev/null 2>&1; then
    echo "error: redis-cli not found" >&2
    exit 1
fi


export REDISCLI_AUTH="${TUMULT_REDIS_AUTH:-}"

echo "simulating Redis hang for ${DURATION}s via DEBUG SLEEP"
# NOTE: redis-cli is backgrounded so this action returns immediately. The
# DEBUG SLEEP self-terminates after TUMULT_DURATION seconds, but the
# background process is invisible to the runner — it cannot be tracked,
# awaited, or rolled back early.
redis-cli -h "${HOST}" -p "${PORT}" DEBUG SLEEP "${DURATION}" &
echo "Redis hang initiated for ${DURATION}s"
