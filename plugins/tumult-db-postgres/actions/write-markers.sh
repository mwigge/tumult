#!/bin/sh
# Write RPO markers to PostgreSQL: a row of tumult_markers every interval,
# stamped with the epoch seconds of the server clock. Once the server has
# confirmed a row, the same time goes to the ledger, a file outside the
# database. Stops after the duration or after the count of markers,
# whichever comes first. A write that fails ends the action with the status
# of psql, and the ledger keeps the markers the server confirmed. Prints
# JSON: the database, the markers written, the first and the last time.
# No rollback: the markers are what markers-lost reads back.
#
# Environment variables:
#   TUMULT_PG_HOST      - PostgreSQL host (default: localhost)
#   TUMULT_PG_PORT      - PostgreSQL port (default: 5432)
#   TUMULT_PG_USER      - PostgreSQL user (default: postgres)
#   TUMULT_PG_PASSWORD  - Password (optional)
#   TUMULT_PG_DATABASE  - The database to write to (required)
#   TUMULT_LEDGER_PATH  - The ledger file, added to (required)
#   TUMULT_INTERVAL_MS  - Milliseconds between two markers (default: 1000)
#   TUMULT_DURATION     - Seconds to write for (default: 10)
#   TUMULT_MARKER_COUNT - Markers to write, 0 for no limit (default: 0)
set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "${SCRIPT_DIR}/../../lib/validate.sh"
. "${SCRIPT_DIR}/../lib/postgres.sh"

HOST="${TUMULT_PG_HOST:-localhost}"
PORT="${TUMULT_PG_PORT:-5432}"
USER="${TUMULT_PG_USER:-postgres}"
DATABASE="${TUMULT_PG_DATABASE:?TUMULT_PG_DATABASE is required}"
LEDGER="${TUMULT_LEDGER_PATH:?TUMULT_LEDGER_PATH is required}"
INTERVAL_MS="${TUMULT_INTERVAL_MS:-1000}"
DURATION="${TUMULT_DURATION:-10}"
COUNT="${TUMULT_MARKER_COUNT:-0}"

validate_identifier "TUMULT_PG_DATABASE" "${DATABASE}"
validate_path "TUMULT_LEDGER_PATH" "${LEDGER}"
validate_integer "TUMULT_INTERVAL_MS" "${INTERVAL_MS}"
validate_integer "TUMULT_DURATION" "${DURATION}"
validate_integer "TUMULT_MARKER_COUNT" "${COUNT}"
need_tool psql

# A marker the server has but the ledger cannot take would count as lost.
if ! true >> "${LEDGER}"; then
    echo "error: cannot write the ledger ${LEDGER}" >&2
    exit 1
fi

use_password_file

# Without the SET, a table that is there already gives a notice on stderr,
# which would read as an error of the step.
psql -h "${HOST}" -p "${PORT}" -U "${USER}" -w -d "${DATABASE}" \
    -c "SET client_min_messages = warning; CREATE TABLE IF NOT EXISTS tumult_markers (written_at double precision NOT NULL);" >/dev/null

SLEEP_S=$(awk -v ms="${INTERVAL_MS}" 'BEGIN { printf "%.3f", ms / 1000 }')
END_AT=$(awk -v start="$(epoch_now)" -v s="${DURATION}" 'BEGIN { printf "%.3f", start + s }')
WRITTEN=0
FIRST=""
while :; do
    # psql returns once the server has committed the row. A failed write
    # ends the action with the status and the message of psql.
    LAST=$(psql -h "${HOST}" -p "${PORT}" -U "${USER}" -w -d "${DATABASE}" -t -A \
        -c "WITH marker AS (INSERT INTO tumult_markers (written_at) VALUES (extract(epoch FROM clock_timestamp())) RETURNING written_at) SELECT written_at FROM marker;")
    case "${LAST}" in
        ''|*[!0-9.]*)
            echo "error: the server answered no time for the marker: '${LAST}'" >&2
            exit 1
            ;;
    esac
    printf '%s\n' "${LAST}" >> "${LEDGER}"
    WRITTEN=$((WRITTEN + 1))
    FIRST="${FIRST:-${LAST}}"
    [ "${WRITTEN}" -eq "${COUNT}" ] && break
    awk -v now="$(epoch_now)" -v end="${END_AT}" 'BEGIN { exit !(now >= end) }' && break
    sleep "${SLEEP_S}"
done

printf '{"database":"%s","markers":%s,"first_at":%s,"last_at":%s}\n' \
    "${DATABASE}" "${WRITTEN}" "${FIRST}" "${LAST}"
