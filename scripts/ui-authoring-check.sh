#!/usr/bin/env bash
# ui-authoring-check.sh — smoke-test the web-UI experiment-authoring API
# against a running tumultd.
#
# Verifies the flow the /author pages drive end to end:
#   SPA shell served → fault catalog non-empty → scaffold a known action →
#   validate-and-register (Operator) → content-hash dedup → dry-run →
#   validation-error path → probe-as-action 400 → RBAC (viewer 403 on
#   register, viewer 200 on catalog/scaffold).
#
# Usage:
#   ADMIN_TOKEN=<admin-or-bootstrap-token> scripts/ui-authoring-check.sh
#
# Environment:
#   TUMULTD_URL   daemon base URL        (default http://localhost:14318,
#                 the kronika compose stack's host port)
#   ADMIN_TOKEN   admin bearer token     (required; falls back to
#                 KRONIKA_BOOTSTRAP_TOKEN)
#   CURL_TIMEOUT  per-request timeout s  (default 8)
#
# RBAC checks need an auth-enabled daemon (the kronika stack qualifies via
# its bootstrap env vars). Against an open, zero-user daemon they are
# skipped with a warning — every principal is synthetic-admin there.
# Tokens are minted for throwaway smoke-viewer / smoke-operator users via
# the admin API; re-runs reset those users' passwords, so the script is
# idempotent. Requires curl and python3.

set -euo pipefail

BASE="${TUMULTD_URL:-http://localhost:14318}"
ADMIN_TOKEN="${ADMIN_TOKEN:-${KRONIKA_BOOTSTRAP_TOKEN:-}}"
TIMEOUT="${CURL_TIMEOUT:-8}"

if [[ -z "${ADMIN_TOKEN}" ]]; then
  echo "error: ADMIN_TOKEN (or KRONIKA_BOOTSTRAP_TOKEN) is required" >&2
  exit 2
fi
for cmd in curl python3; do
  command -v "${cmd}" > /dev/null 2>&1 || { echo "error: ${cmd} not found" >&2; exit 2; }
done

GREEN='\033[0;32m'; RED='\033[0;31m'; YELLOW='\033[0;33m'; DIM='\033[0;90m'; NC='\033[0m'
FAILURES=0
pass() { echo -e "${GREEN}pass${NC} $1"; }
fail() { echo -e "${RED}fail${NC} $1"; FAILURES=$((FAILURES + 1)); }
warn() { echo -e "${YELLOW}warn${NC} $1"; }
info() { echo -e "${DIM}info${NC} $1"; }

ADMIN="Authorization: Bearer ${ADMIN_TOKEN}"
CT='content-type: application/json'
BODY="$(mktemp)"
trap 'rm -f "${BODY}"' EXIT

# req <method> <path> <auth-header> [json-body] → echoes the HTTP status,
# leaves the response body in $BODY.
req() {
  local method="$1" path="$2" auth="$3" data="${4:-}"
  local args=(-s -m "${TIMEOUT}" -o "${BODY}" -w '%{http_code}' -X "${method}" -H "${auth}")
  if [[ -n "${data}" ]]; then
    args+=(-H "${CT}" -d "${data}")
  fi
  curl "${args[@]}" "${BASE}${path}"
}

# json_get <python-expr-on-doc> — evaluate against $BODY, exit 1 on error.
json_get() {
  python3 - "$1" "${BODY}" <<'PY'
import json, sys
expr, path = sys.argv[1], sys.argv[2]
try:
    doc = json.load(open(path))
    print(eval(expr))
except Exception as e:
    print(f"json error: {e}", file=sys.stderr)
    sys.exit(1)
PY
}

echo "=== ui-authoring smoke against ${BASE}"

# --- 1 · SPA shell ----------------------------------------------------------
code=$(req GET / "${ADMIN}")
if [[ "${code}" == "200" ]] && grep -q '_app/' "${BODY}"; then
  pass "SPA shell served at /"
else
  fail "SPA shell (status ${code})"
fi

# --- 2 · fault catalog ------------------------------------------------------
code=$(req GET /api/authoring/catalog "${ADMIN}")
if [[ "${code}" == "200" ]] && grep -q '"pause-container"' "${BODY}" \
  && [[ "$(json_get "doc['action_count']")" -gt 0 ]]; then
  pass "catalog serves mounted plugins (pause-container present)"
else
  fail "catalog (status ${code}): $(head -c 200 "${BODY}")"
fi

# --- 3 · RBAC fixture: smoke-operator + smoke-viewer -------------------------
# Users are created (or password-reset) with a one-time password, taken
# through the mandatory change, then minted a working token — the same
# recipe as the kronika seed.
provision_token() {
  local username="$1" role="$2"
  local onetime="smoke-one-time-pw-123" permanent="smoke-permanent-pw-456"
  req POST /api/users "${ADMIN}" \
    "{\"username\":\"${username}\",\"password\":\"${onetime}\",\"role\":\"${role}\"}" > /dev/null
  local uid
  uid=$(req GET /api/users "${ADMIN}" > /dev/null && \
    json_get "next(u['id'] for u in doc['users'] if u['username'] == '${username}')")
  # Re-arm the one-time password (idempotent re-runs), then change it.
  req POST "/api/users/${uid}/password" "${ADMIN}" "{\"password\":\"${onetime}\"}" > /dev/null
  local tmp_token
  tmp_token=$(req POST /api/tokens "${ADMIN}" "{\"name\":\"smoke-tmp\",\"user_id\":\"${uid}\"}" > /dev/null && \
    json_get "doc['token']")
  req POST /api/auth/change-password "Authorization: Bearer ${tmp_token}" \
    "{\"current_password\":\"${onetime}\",\"new_password\":\"${permanent}\"}" > /dev/null
  req POST /api/tokens "${ADMIN}" "{\"name\":\"smoke\",\"user_id\":\"${uid}\"}" > /dev/null && \
    json_get "doc['token']"
}

OPERATOR_TOKEN=""
VIEWER_TOKEN=""
AUTH_REQUIRED=$(req GET /api/me "${ADMIN}" > /dev/null && json_get "doc.get('auth_required', False)")
if [[ "${AUTH_REQUIRED}" == "True" ]]; then
  OPERATOR_TOKEN=$(provision_token smoke-operator operator)
  VIEWER_TOKEN=$(provision_token smoke-viewer viewer)
  OPERATOR="Authorization: Bearer ${OPERATOR_TOKEN}"
  VIEWER="Authorization: Bearer ${VIEWER_TOKEN}"
  info "provisioned smoke-operator / smoke-viewer tokens"
else
  OPERATOR="${ADMIN}"
  warn "daemon is in open mode (no users) — RBAC checks will be skipped"
fi

# --- 4 · scaffold a known action ---------------------------------------------
code=$(req POST /api/authoring/scaffold "${OPERATOR}" '{
  "plugin": "tumult-containers",
  "action": "pause-container",
  "args": {"container_id": "demo-postgres"},
  "target": "demo-postgres",
  "probe_command": "pg_isready -h demo-postgres",
  "probe_expect": "accepting connections"
}')
TOON=""
if [[ "${code}" == "200" ]] && [[ "$(json_get "doc['valid']")" == "True" ]] \
  && [[ "$(json_get "'pause-container' in doc['toon']")" == "True" ]]; then
  TOON=$(json_get "doc['toon']")
  pass "scaffold pause-container returns valid TOON"
else
  fail "scaffold (status ${code}): $(head -c 200 "${BODY}")"
fi

# --- 5 · validate-and-register + dedup + dry-run -----------------------------
if [[ -n "${TOON}" ]]; then
  payload=$(python3 -c 'import json,sys; print(json.dumps({"toon": sys.stdin.read()}))' <<< "${TOON}")
  code=$(req POST /api/runs/validate "${OPERATOR}" "${payload}")
  REG_ID=""
  if [[ "${code}" == "200" ]] && [[ "$(json_get "doc['valid']")" == "True" ]]; then
    REG_ID=$(json_get "doc['registry_id']")
    if [[ "${REG_ID}" == reg-* ]]; then
      pass "validate-and-register returned ${REG_ID}"
    else
      fail "registry id shape: ${REG_ID}"
    fi
  else
    fail "validate-and-register (status ${code}): $(head -c 200 "${BODY}")"
  fi

  code=$(req POST /api/runs/validate "${OPERATOR}" "${payload}")
  if [[ "${code}" == "200" ]] && [[ "$(json_get "doc['registered']")" == "False" ]]; then
    pass "identical TOON dedups (registered: false)"
  else
    fail "content-hash dedup (status ${code}): $(head -c 200 "${BODY}")"
  fi

  code=$(req POST /api/runs/dry-run "${OPERATOR}" "{\"registry_id\":\"${REG_ID}\",\"vars\":{}}")
  if [[ "${code}" == "200" ]] && [[ "$(json_get "doc['valid']")" == "True" ]] \
    && [[ -n "$(json_get "doc['plan']['title']")" ]]; then
    pass "dry-run resolves a plan for ${REG_ID}"
  else
    fail "dry-run (status ${code}): $(head -c 200 "${BODY}")"
  fi
fi

# --- 6 · validation-error path ------------------------------------------------
code=$(req POST /api/runs/validate "${OPERATOR}" '{"toon":"title: [unclosed"}')
if [[ "${code}" == "200" ]] && [[ "$(json_get "doc['valid']")" == "False" ]] \
  && [[ -n "$(json_get "doc['error']")" ]]; then
  pass "broken TOON returns valid:false with an error (HTTP 200)"
else
  fail "validation-error path (status ${code}): $(head -c 200 "${BODY}")"
fi

# --- 7 · probe-as-action is a 400 ----------------------------------------------
code=$(req POST /api/authoring/scaffold "${OPERATOR}" '{
  "plugin": "tumult-containers",
  "action": "container-status",
  "args": {"container_id": "demo-postgres"},
  "target": "demo-postgres"
}')
if [[ "${code}" == "400" ]] && grep -q 'unknown action' "${BODY}"; then
  pass "scaffolding a catalog probe is rejected (400 unknown action)"
else
  fail "probe-as-action (status ${code}): $(head -c 200 "${BODY}")"
fi

# --- 8 · RBAC -------------------------------------------------------------------
if [[ "${AUTH_REQUIRED}" == "True" ]]; then
  code=$(req GET /api/authoring/catalog "${VIEWER}")
  if [[ "${code}" == "200" ]]; then
    pass "viewer can read the catalog"
  else
    fail "viewer catalog read (status ${code})"
  fi

  code=$(req POST /api/runs/validate "${VIEWER}" '{"toon":"title: x"}')
  if [[ "${code}" == "403" ]]; then
    pass "viewer is 403 on validate-and-register (operator-gated)"
  else
    fail "viewer register (status ${code}, want 403)"
  fi
fi

echo
if [[ "${FAILURES}" -eq 0 ]]; then
  echo -e "${GREEN}=== ui-authoring smoke: all checks passed${NC}"
  exit 0
else
  echo -e "${RED}=== ui-authoring smoke: ${FAILURES} check(s) failed${NC}"
  exit 1
fi
