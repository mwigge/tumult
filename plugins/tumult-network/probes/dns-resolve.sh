#!/bin/sh
# Probe: check if DNS resolution works for a hostname
# Outputs: resolved IP address, or "failed" if resolution fails
#
# Environment variables:
#   TUMULT_TARGET_HOST - Hostname to resolve (required)
set -eu

TARGET="${TUMULT_TARGET_HOST:?TUMULT_TARGET_HOST is required}"

if command -v dig >/dev/null 2>&1; then
    RESULT=$(dig +short "${TARGET}" 2>/dev/null | head -1)
elif command -v nslookup >/dev/null 2>&1; then
    RESULT=$(nslookup "${TARGET}" 2>/dev/null | awk '/^Address:/ && !/127/ { print $2 }' | head -1)
elif command -v host >/dev/null 2>&1; then
    RESULT=$(host "${TARGET}" 2>/dev/null | awk '/has address/ { print $4 }' | head -1)
else
    echo "error: no DNS tool found (dig, nslookup, or host)" >&2
    exit 1
fi

if [ -z "${RESULT}" ]; then
    echo "failed"
else
    echo "${RESULT}"
fi
