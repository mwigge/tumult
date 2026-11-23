#!/bin/sh
# Promote a PostgreSQL streaming standby to primary with pg_promote(), then
# take a transaction id, which a server in recovery refuses and every write
# needs. Prints JSON: the time the server took it, in epoch seconds by its
# own clock. No rollback: a promoted server cannot be a standby again. Make
# the old primary a standby of the new one (pg_rewind or a new base backup).
#
# Environment variables:
#   TUMULT_PG_HOST     - Host of the standby (default: localhost)
#   TUMULT_PG_PORT     - Port of the standby (default: 5432)
#   TUMULT_PG_USER     - PostgreSQL user (default: postgres)
#   TUMULT_PG_PASSWORD - Password (optional)
#   TUMULT_TIMEOUT     - Seconds to wait for the promotion (default: 60)
set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "${SCRIPT_DIR}/../../lib/validate.sh"
. "${SCRIPT_DIR}/../lib/postgres.sh"

HOST="${TUMULT_PG_HOST:-localhost}"
PORT="${TUMULT_PG_PORT:-5432}"
USER="${TUMULT_PG_USER:-postgres}"
TIMEOUT="${TUMULT_TIMEOUT:-60}"

validate_integer "TUMULT_TIMEOUT" "${TIMEOUT}"
need_tool psql
use_password_file

# A failed query ends the action with the status and the message of psql.
PROMOTED=$(psql -h "${HOST}" -p "${PORT}" -U "${USER}" -w -d postgres -t -A \
    -c "SELECT pg_promote(true, ${TIMEOUT});")
if [ "${PROMOTED}" != "t" ]; then
    echo "error: the standby at ${HOST}:${PORT} did not end its promotion within ${TIMEOUT}s" >&2
    exit 1
fi

# One row: the transaction id and the time, as "748|1763869012.5".
WRITE=$(psql -h "${HOST}" -p "${PORT}" -U "${USER}" -w -d postgres -t -A \
    -c "SELECT txid_current(), extract(epoch FROM clock_timestamp());")
WRITABLE_AT="${WRITE#*|}"
case "${WRITABLE_AT}" in
    ''|*[!0-9.]*)
        echo "error: the server answered no time for its first write: '${WRITE}'" >&2
        exit 1
        ;;
esac

printf '{"writable_at":%s}\n' "${WRITABLE_AT}"
