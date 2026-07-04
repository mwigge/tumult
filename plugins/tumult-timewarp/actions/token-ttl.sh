#!/bin/sh
# token-ttl — prove that clock skew breaks short-lived token (JWT/session) auth.
#
# Mechanism & limits
# --------------------------
# Mints a signed bearer token of the form  "exp=<epoch>.<hmac>"  with a short
# TTL, then validates it TWICE:
#   1. at the real current time      -> ACCEPTED (steady state)
#   2. at now + TUMULT_SKEW_SECONDS  -> REJECTED (skew past exp)
# The signature is an HMAC over the payload using sha256sum, so this needs
# only coreutils — no openssl, no external service — which makes it the most
# portable clock-skew auth demonstration (works in any container with a shell).
# The skewed "now" is fed to the validator directly, exactly as a drifted node
# clock would; no system clock is modified.
#
# Environment variables:
#   TUMULT_TOKEN_TTL_SECONDS - token lifetime in seconds (default: 2)
#   TUMULT_SKEW_SECONDS      - how far to advance the validator's clock (default: 3600)
#   TUMULT_TOKEN_SECRET      - HMAC secret (default: tumult-timewarp-demo)
#   TUMULT_TW_STATE_DIR      - state/marker dir (default: /tmp/tumult-timewarp)
#
# Exit codes: 0 = scenario reproduced (accepted now, rejected under skew);
#             1 = tooling missing or scenario could not be reproduced.
set -eu

. "$(dirname "$0")/../../lib/validate.sh"

TTL="${TUMULT_TOKEN_TTL_SECONDS:-2}"
SKEW="${TUMULT_SKEW_SECONDS:-3600}"
SECRET="${TUMULT_TOKEN_SECRET:-tumult-timewarp-demo}"
STATE_DIR="${TUMULT_TW_STATE_DIR:-/tmp/tumult-timewarp}"

validate_integer "TUMULT_TOKEN_TTL_SECONDS" "${TTL}"
validate_integer "TUMULT_SKEW_SECONDS" "${SKEW}"

if ! command -v sha256sum >/dev/null 2>&1; then
    echo "error: sha256sum not found (required for token HMAC)" >&2
    exit 1
fi

mkdir -p "${STATE_DIR}"
MARKER="${STATE_DIR}/token-result.txt"
rm -f "${MARKER}"

sign() {
    # $1 = payload string -> prints hex hmac (keyed sha256 over secret+payload)
    printf '%s' "${SECRET}$1" | sha256sum | cut -d' ' -f1
}

NOW="$(date -u +%s)"
EXP=$(( NOW + TTL ))
SIG="$(sign "exp=${EXP}")"
TOKEN="exp=${EXP}.${SIG}"

echo "timewarp: minted token exp=${EXP} (ttl=${TTL}s) now=${NOW}"

validate_at() {
    # $1 = the "now" the validator perceives. echoes ACCEPT|REJECT-<reason>
    _now="$1"
    _exp="${TOKEN%%.*}"; _exp="${_exp#exp=}"
    _sig="${TOKEN#*.}"
    if [ "$(sign "exp=${_exp}")" != "${_sig}" ]; then
        echo "REJECT-badsig"; return
    fi
    if [ "${_now}" -ge "${_exp}" ]; then
        echo "REJECT-expired"; return
    fi
    echo "ACCEPT"
}

# 1. Validate at real now — expected ACCEPT.
R1="$(validate_at "${NOW}")"
if [ "${R1}" != "ACCEPT" ]; then
    echo "ACCEPT_NOW_FAILED token rejected at current time (${R1})" > "${MARKER}"
    echo "error: token unexpectedly rejected at current time (${R1})" >&2
    exit 1
fi
echo "timewarp: validate @now = ACCEPT (steady state ok)"

# 2. Validate at skewed future now — expected REJECT-expired.
FUTURE=$(( NOW + SKEW ))
R2="$(validate_at "${FUTURE}")"
if [ "${R2}" = "ACCEPT" ]; then
    echo "SKEW_TOO_SMALL token still valid at now+${SKEW}s" > "${MARKER}"
    echo "error: token still valid under skew (+${SKEW}s) — increase TUMULT_SKEW_SECONDS beyond TTL" >&2
    exit 1
fi

printf 'REJECTED_UNDER_SKEW skew=%s ttl=%s now=%s skewed=%s reason=%s\n' "${SKEW}" "${TTL}" "${NOW}" "${FUTURE}" "${R2}" > "${MARKER}"
echo "timewarp: validate @now+${SKEW}s = ${R2} (clock-skew auth failure reproduced)"
echo "timewarp: REJECTED_UNDER_SKEW"
