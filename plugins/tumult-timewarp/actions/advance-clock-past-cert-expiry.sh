#!/bin/sh
# advance-clock-past-cert-expiry — prove that clock skew breaks TLS auth.
#
# Mechanism & honest limits
# --------------------------
# Mints a short-lived self-signed cert, then verifies it TWICE with openssl:
#   1. at the real current time            -> MUST succeed (steady state)
#   2. at now + TUMULT_SKEW_SECONDS        -> MUST fail  (skew past notAfter)
# `openssl verify -attime <epoch>` feeds the verifier the exact wall-clock a
# skewed client/server would see, so this reproduces a real clock-skew auth
# failure deterministically WITHOUT changing any system clock (which in a
# shared-kernel container we cannot safely do). This is the honest, portable
# stand-in for "the node's clock jumped past the cert's expiry".
#
# Writes a machine-readable marker file for a follow-up probe to assert on.
#
# Environment variables:
#   TUMULT_CERT_TTL_SECONDS - cert lifetime in seconds (default: 5; openssl
#                             -days rounds up, so the real window is >= 1 day)
#   TUMULT_SKEW_SECONDS     - how far to advance the clock (default: 8640000 =
#                             100 days, safely past a 1-day cert)
#   TUMULT_TW_STATE_DIR     - state/marker dir (default: /tmp/tumult-timewarp)
#
# Exit codes: 0 = scenario reproduced (valid now, invalid under skew);
#             1 = openssl missing or scenario could not be reproduced.
set -eu

. "$(dirname "$0")/../../lib/validate.sh"

TTL="${TUMULT_CERT_TTL_SECONDS:-5}"
SKEW="${TUMULT_SKEW_SECONDS:-8640000}"
STATE_DIR="${TUMULT_TW_STATE_DIR:-/tmp/tumult-timewarp}"

validate_integer "TUMULT_CERT_TTL_SECONDS" "${TTL}"
validate_integer "TUMULT_SKEW_SECONDS" "${SKEW}"

if ! command -v openssl >/dev/null 2>&1; then
    echo "error: openssl not found (required for the cert-expiry scenario)" >&2
    exit 1
fi

mkdir -p "${STATE_DIR}"
KEY="${STATE_DIR}/timewarp.key"
CRT="${STATE_DIR}/timewarp.crt"
MARKER="${STATE_DIR}/cert-result.txt"
rm -f "${MARKER}"

# Mint a self-signed cert whose validity window is TTL seconds wide.
# -days rounds to whole days, so use a short window via a not-before of now.
DAYS=1
[ "${TTL}" -ge 86400 ] && DAYS=$(( (TTL + 86399) / 86400 ))
openssl req -x509 -newkey rsa:2048 -nodes \
    -keyout "${KEY}" -out "${CRT}" \
    -subj "/CN=timewarp.demo" -days "${DAYS}" >/dev/null 2>&1

NOW="$(date -u +%s)"
FUTURE=$(( NOW + SKEW ))

echo "timewarp: minted cert ${CRT} (valid ~${DAYS}d); now=${NOW} skewed=${FUTURE} (+${SKEW}s)"

# 1. Verify at real now — expected to pass (this is the steady state).
if openssl verify -attime "${NOW}" -CAfile "${CRT}" "${CRT}" >/dev/null 2>&1; then
    echo "timewarp: verify @now = VALID (steady state ok)"
else
    echo "VALID_NOW_FAILED cert unexpectedly invalid at current time" > "${MARKER}"
    echo "error: cert failed to verify at current time — cannot run scenario" >&2
    exit 1
fi

# 2. Verify at skewed future time — expected to FAIL (skew past notAfter).
if openssl verify -attime "${FUTURE}" -CAfile "${CRT}" "${CRT}" >/dev/null 2>&1; then
    echo "SKEW_TOO_SMALL cert still valid at now+${SKEW}s; increase TUMULT_SKEW_SECONDS" > "${MARKER}"
    echo "error: cert still valid under skew (+${SKEW}s) — increase TUMULT_SKEW_SECONDS beyond cert TTL" >&2
    exit 1
fi

printf 'EXPIRED_UNDER_SKEW skew=%s cert_ttl=%s now=%s skewed=%s\n' "${SKEW}" "${TTL}" "${NOW}" "${FUTURE}" > "${MARKER}"
echo "timewarp: verify @now+${SKEW}s = INVALID (clock-skew auth failure reproduced)"
echo "timewarp: EXPIRED_UNDER_SKEW"
