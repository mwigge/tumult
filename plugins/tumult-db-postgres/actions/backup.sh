#!/bin/sh
# Back up one PostgreSQL database to a file with pg_dump (custom format).
# Prints JSON: the database, the path, the size in bytes and the time the
# backup finished in epoch seconds. A failed dump leaves the file at the
# path as it was. No rollback: a backup changes nothing in the database.
#
# Environment variables:
#   TUMULT_PG_HOST     - PostgreSQL host (default: localhost)
#   TUMULT_PG_PORT     - PostgreSQL port (default: 5432)
#   TUMULT_PG_USER     - PostgreSQL user (default: postgres)
#   TUMULT_PG_PASSWORD - Password (optional)
#   TUMULT_PG_DATABASE - The database to back up (required)
#   TUMULT_BACKUP_PATH - The file to write (required)
set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "${SCRIPT_DIR}/../../lib/validate.sh"
. "${SCRIPT_DIR}/../lib/postgres.sh"

HOST="${TUMULT_PG_HOST:-localhost}"
PORT="${TUMULT_PG_PORT:-5432}"
USER="${TUMULT_PG_USER:-postgres}"
DATABASE="${TUMULT_PG_DATABASE:?TUMULT_PG_DATABASE is required}"
BACKUP_PATH="${TUMULT_BACKUP_PATH:?TUMULT_BACKUP_PATH is required}"

validate_identifier "TUMULT_PG_DATABASE" "${DATABASE}"
validate_path "TUMULT_BACKUP_PATH" "${BACKUP_PATH}"
need_tool pg_dump
use_password_file

# Dump to a file beside the path first, so a failed dump keeps an older
# backup at the path.
PARTIAL="${BACKUP_PATH}.partial"
pg_dump -h "${HOST}" -p "${PORT}" -U "${USER}" -w -d "${DATABASE}" -Fc -f "${PARTIAL}" || {
    STATUS=$?
    rm -f "${PARTIAL}"
    exit "${STATUS}"
}
mv -f "${PARTIAL}" "${BACKUP_PATH}"
SIZE=$(wc -c < "${BACKUP_PATH}" | tr -d ' ')

printf '{"database":"%s","path":"%s","size_bytes":%s,"finished_at":%s}\n' \
    "${DATABASE}" "${BACKUP_PATH}" "${SIZE}" "$(epoch_now)"
