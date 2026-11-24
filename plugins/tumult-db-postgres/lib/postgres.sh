#!/bin/sh
# Shared steps of the PostgreSQL disaster recovery scripts.
# Source it after validate.sh: . "${SCRIPT_DIR}/../lib/postgres.sh"

# Fail when a tool is not on PATH.
# Usage: need_tool pg_dump
need_tool() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "error: $1 not found" >&2
        exit 1
    fi
}

# Give TUMULT_PG_PASSWORD to the tools in a temporary .pgpass file, never on
# their command line or in their environment. The file format asks for a
# backslash before each ':' and '\'. The file goes when the script ends.
use_password_file() {
    PGPASS_FILE=$(mktemp)
    trap 'rm -f "${PGPASS_FILE}"' EXIT
    trap 'exit 1' INT TERM
    ESCAPED=$(printf '%s' "${TUMULT_PG_PASSWORD:-}" | sed 's/[\\:]/\\&/g')
    printf '*:*:*:*:%s\n' "${ESCAPED}" > "${PGPASS_FILE}"
    unset TUMULT_PG_PASSWORD ESCAPED
    export PGPASSFILE="${PGPASS_FILE}"
}

# The time now in epoch seconds: to the millisecond where date prints
# nanoseconds (GNU), else to the second. busybox prints nothing for %N and
# BSD prints the letter.
epoch_now() {
    NOW=$(date +%s.%N)
    case "${NOW}" in
        ''|*[!0-9.]*|*.) NOW=$(date +%s) ;;
    esac
    printf '%s\n' "${NOW}" | sed 's/^\([0-9]*\.[0-9][0-9][0-9]\).*$/\1/'
}
