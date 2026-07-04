#!/bin/sh
# Probe: time to complete a fixed RNG/crypto operation, in milliseconds.
# Outputs a single integer (elapsed ms). Higher = slower crypto, e.g. under
# rng-pressure / entropy-drain contention.
#
# Operation: generate TUMULT_CRYPTO_BYTES of cryptographic randomness via
# `openssl rand` (falls back to reading /dev/urandom when openssl is absent).
#
# Environment variables:
#   TUMULT_CRYPTO_BYTES - bytes of randomness to generate (default: 33554432 = 32 MiB)
set -eu

. "$(dirname "$0")/../../lib/validate.sh"

BYTES="${TUMULT_CRYPTO_BYTES:-33554432}"
validate_integer "TUMULT_CRYPTO_BYTES" "${BYTES}"

now_ms() {
    # Nanosecond clock -> ms; falls back to seconds*1000 if %N unsupported.
    ns="$(date +%s%N 2>/dev/null || echo '')"
    case "${ns}" in
        *N|'') echo "$(( $(date +%s) * 1000 ))" ;;
        *)     echo "$(( ns / 1000000 ))" ;;
    esac
}

START="$(now_ms)"
if command -v openssl >/dev/null 2>&1; then
    openssl rand "${BYTES}" >/dev/null 2>&1
else
    blocks=$(( BYTES / 1048576 ))
    [ "${blocks}" -lt 1 ] && blocks=1
    dd if=/dev/urandom of=/dev/null bs=1M count="${blocks}" >/dev/null 2>&1
fi
END="$(now_ms)"

echo "$(( END - START ))"
