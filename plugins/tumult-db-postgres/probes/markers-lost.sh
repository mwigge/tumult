#!/bin/sh
# Probe: the seconds of confirmed writes a PostgreSQL database lost, the RPO.
# Prints the newest time in the ledger of write-markers minus the newest
# marker left in tumult_markers: 0 when nothing is lost or the ledger is
# empty, the newest minus the oldest ledger time when no marker is left.
# Fails with the message of psql when it cannot read the database, so a
# run whose database is not back is not recovered.
#
# Environment variables:
#   TUMULT_PG_HOST     - PostgreSQL host (default: localhost)
#   TUMULT_PG_PORT     - PostgreSQL port (default: 5432)
#   TUMULT_PG_USER     - PostgreSQL user (default: postgres)
#   TUMULT_PG_PASSWORD - Password (optional)
#   TUMULT_PG_DATABASE - The database the markers were written to (required)
#   TUMULT_LEDGER_PATH - The ledger of write-markers (required)
set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "${SCRIPT_DIR}/../../lib/validate.sh"
. "${SCRIPT_DIR}/../lib/postgres.sh"

HOST="${TUMULT_PG_HOST:-localhost}"
PORT="${TUMULT_PG_PORT:-5432}"
USER="${TUMULT_PG_USER:-postgres}"
DATABASE="${TUMULT_PG_DATABASE:?TUMULT_PG_DATABASE is required}"
LEDGER="${TUMULT_LEDGER_PATH:?TUMULT_LEDGER_PATH is required}"

validate_identifier "TUMULT_PG_DATABASE" "${DATABASE}"
validate_path "TUMULT_LEDGER_PATH" "${LEDGER}"
need_tool psql
use_password_file

# A restore of a backup older than the table takes the table with it: no
# marker is left. A failed query ends the probe with the status of psql.
TABLE=$(psql -h "${HOST}" -p "${PORT}" -U "${USER}" -w -d "${DATABASE}" -t -A \
    -c "SELECT to_regclass('tumult_markers') IS NOT NULL;")
NEWEST=""
if [ "${TABLE}" = "t" ]; then
    NEWEST=$(psql -h "${HOST}" -p "${PORT}" -U "${USER}" -w -d "${DATABASE}" -t -A \
        -c "SELECT max(written_at) FROM tumult_markers;")
fi

# No ledger yet: no marker was confirmed, so none can be lost.
if [ ! -f "${LEDGER}" ]; then
    echo 0
    exit 0
fi

awk -v newest="${NEWEST}" -v ledger="${LEDGER}" '
    /^[0-9]+(\.[0-9]+)?$/ {
        if (n == 0 || $0 + 0 > high) high = $0 + 0
        if (n == 0 || $0 + 0 < low) low = $0 + 0
        n++
        next
    }
    /^$/ { next }
    {
        printf "error: the ledger %s holds a line that is no time: %s\n", ledger, $0 > "/dev/stderr"
        bad = 1
        exit 1
    }
    END {
        if (bad) exit 1
        lost = 0
        if (n > 0 && newest == "") lost = high - low
        if (n > 0 && newest != "") lost = high - newest
        if (lost < 0) lost = 0
        shown = sprintf("%.6f", lost)
        sub(/0+$/, "", shown)
        sub(/\.$/, "", shown)
        print shown
    }' "${LEDGER}"
