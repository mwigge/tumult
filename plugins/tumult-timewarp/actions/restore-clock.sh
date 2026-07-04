#!/bin/sh
# restore-clock — rollback for the clock-skew actions.
#
# The skew actions are deliberately non-persistent:
#   * skew-clock uses libfaketime, which affects only the wrapped process and
#     evaporates when that process exits — there is no host clock to restore.
#   * advance-clock-past-cert-expiry / token-ttl never touch any system clock;
#     they only write temp cert/token state and a marker file.
# So this rollback simply removes the temp cert/token state. A faketime helper
# needs no cleanup: it exits with the command it wrapped, leaving no host state.
# Always exits 0.
#
# Environment variables:
#   TUMULT_TW_STATE_DIR - state/marker dir to remove (default: /tmp/tumult-timewarp)
set -eu

STATE_DIR="${TUMULT_TW_STATE_DIR:-/tmp/tumult-timewarp}"

if [ -d "${STATE_DIR}" ]; then
    rm -f "${STATE_DIR}/timewarp.key" "${STATE_DIR}/timewarp.crt" \
          "${STATE_DIR}/cert-result.txt" "${STATE_DIR}/token-result.txt"
    rmdir "${STATE_DIR}" 2>/dev/null || true
fi

echo "timewarp: clock state restored (faketime is per-process; no host clock was changed)"
