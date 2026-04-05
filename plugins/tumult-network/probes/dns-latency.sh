#!/bin/sh
# Probe: measure DNS resolution time in milliseconds
# Outputs: resolution time as integer ms, or "error" if resolution fails
#
# Environment variables:
#   TUMULT_TARGET_HOST  - Hostname to resolve (required)
#   TUMULT_DNS_SERVER   - DNS server to query (optional, uses system default)
set -eu

TARGET="${TUMULT_TARGET_HOST:?TUMULT_TARGET_HOST is required}"
DNS_SERVER="${TUMULT_DNS_SERVER:-}"

if command -v dig >/dev/null 2>&1; then
    if [ -n "${DNS_SERVER}" ]; then
        RESULT=$(dig +noall +stats "${TARGET}" "@${DNS_SERVER}" 2>/dev/null | awk '/Query time:/ {print $4}')
    else
        RESULT=$(dig +noall +stats "${TARGET}" 2>/dev/null | awk '/Query time:/ {print $4}')
    fi
elif command -v nslookup >/dev/null 2>&1; then
    # nslookup doesn't report timing; measure wall-clock time
    START=$(date +%s%N 2>/dev/null || echo "0")
    if [ -n "${DNS_SERVER}" ]; then
        nslookup "${TARGET}" "${DNS_SERVER}" >/dev/null 2>&1
    else
        nslookup "${TARGET}" >/dev/null 2>&1
    fi
    END=$(date +%s%N 2>/dev/null || echo "0")
    if [ "${START}" != "0" ] && [ "${END}" != "0" ]; then
        RESULT=$(( (END - START) / 1000000 ))
    else
        # Fallback: use time command
        RESULT=$(sh -c "time nslookup ${TARGET} >/dev/null 2>&1" 2>&1 | awk '/real/ {split($2,a,"[ms]"); print a[1]*1000+a[2]}' || echo "error")
    fi
else
    echo "error: no DNS tool found (dig or nslookup)" >&2
    exit 1
fi

if [ -z "${RESULT}" ] || [ "${RESULT}" = "error" ]; then
    echo "error"
    exit 1
fi

echo "${RESULT}"
