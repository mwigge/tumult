#!/bin/sh
# Restore one PostgreSQL database from a backup of backup.sh: drop the
# database, make it again and run pg_restore in one transaction. Prints
# JSON: the database, the path and the time the restore finished in epoch
# seconds. Nothing is dropped when the file is no archive pg_restore reads.
# No rollback: to undo a restore, restore a newer backup.
#
# Environment variables:
#   TUMULT_PG_HOST     - PostgreSQL host (default: localhost)
#   TUMULT_PG_PORT     - PostgreSQL port (default: 5432)
#   TUMULT_PG_USER     - PostgreSQL user (default: postgres)
#   TUMULT_PG_PASSWORD - Password (optional)
#   TUMULT_PG_DATABASE - The database to restore (required)
#   TUMULT_BACKUP_PATH - The backup to restore from (required)
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
need_tool psql
need_tool pg_restore

if [ ! -f "${BACKUP_PATH}" ]; then
    echo "error: no backup at ${BACKUP_PATH}" >&2
    exit 1
fi
# Read the archive before the database goes: a file pg_restore cannot
# read must not cost the database.
pg_restore -l "${BACKUP_PATH}" >/dev/null

use_password_file

# DROP DATABASE cannot run in a transaction, so each statement has its own
# call. FORCE ends the connections to the database (PostgreSQL 13 or later).
psql -h "${HOST}" -p "${PORT}" -U "${USER}" -w -d postgres \
    -c "DROP DATABASE IF EXISTS \"${DATABASE}\" WITH (FORCE);" >/dev/null
psql -h "${HOST}" -p "${PORT}" -U "${USER}" -w -d postgres \
    -c "CREATE DATABASE \"${DATABASE}\" TEMPLATE template0;" >/dev/null
pg_restore -h "${HOST}" -p "${PORT}" -U "${USER}" -w -d "${DATABASE}" \
    --single-transaction "${BACKUP_PATH}"

printf '{"database":"%s","path":"%s","finished_at":%s}\n' \
    "${DATABASE}" "${BACKUP_PATH}" "$(epoch_now)"
