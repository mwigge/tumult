#!/bin/sh
# Shared input validation functions for Tumult script plugins.
# Source this file from any plugin script: . "$(dirname "$0")/../lib/validate.sh"

# Validate that a value is a safe SQL identifier (alphanumeric + underscores + dots)
# Usage: validate_identifier "TUMULT_PG_DATABASE" "$DATABASE"
validate_identifier() {
    VAR_NAME="$1"
    VALUE="$2"
    if ! echo "${VALUE}" | grep -qE '^[a-zA-Z_][a-zA-Z0-9_.]*$'; then
        echo "error: ${VAR_NAME} contains invalid characters: '${VALUE}'" >&2
        echo "  allowed: letters, digits, underscores, dots" >&2
        exit 1
    fi
}

# Validate that a value is a positive integer
# Usage: validate_integer "TUMULT_TIMEOUT" "$TIMEOUT"
validate_integer() {
    VAR_NAME="$1"
    VALUE="$2"
    case "${VALUE}" in
        ''|*[!0-9]*)
            echo "error: ${VAR_NAME} must be a positive integer, got: '${VALUE}'" >&2
            exit 1
            ;;
    esac
}

# Validate that a value is a positive number (integer or float)
# Usage: validate_number "TUMULT_LATENCY_MS" "$LATENCY_MS"
validate_number() {
    VAR_NAME="$1"
    VALUE="$2"
    # Must match: integer (123) or decimal (1.5) — no multiple dots, no leading/trailing dot alone
    if ! echo "${VALUE}" | grep -qE '^[0-9]+(\.[0-9]+)?$'; then
        echo "error: ${VAR_NAME} must be a number, got: '${VALUE}'" >&2
        exit 1
    fi
}

# Validate that a value is one of an allowed set
# Usage: validate_enum "TUMULT_RUNTIME" "$RUNTIME" "docker podman"
validate_enum() {
    VAR_NAME="$1"
    VALUE="$2"
    ALLOWED="$3"
    for ITEM in ${ALLOWED}; do
        [ "${VALUE}" = "${ITEM}" ] && return 0
    done
    echo "error: ${VAR_NAME} must be one of: ${ALLOWED}, got: '${VALUE}'" >&2
    exit 1
}
