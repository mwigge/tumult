#!/usr/bin/env bash
#
# ui-table-stakes-check.sh — end-to-end smoke for the UI table-stakes track
# against a running tumultd: users, tokens, dry-run blast-radius scope,
# global halt, schedules, the events feed, gamedays, and webhooks.
#
# Style follows scripts/demo-check.sh: every check prints PASS/FAIL and the
# script exits non-zero on any failure.
#
# Environment (all overridable):
#   TUMULTD_URL        daemon base URL         (default http://127.0.0.1:24318)
#   SMOKE_ADMIN_TOKEN  admin kro_ bearer token (default: the dev-daemon value)
#
# A suitable local daemon (from the repo root):
#   TUMULT_LAKE_PATH=/tmp/smoke/lake.duckdb \
#   KRONIKA_OTLP_GRPC_ADDR=127.0.0.1:24317 KRONIKA_OTLP_HTTP_ADDR=127.0.0.1:24318 \
#   KRONIKA_BOOTSTRAP_ADMIN_PASSWORD=smoke-admin-pw-123 \
#   KRONIKA_BOOTSTRAP_TOKEN=kro_ui-table-stakes-smoke \
#   TUMULTD_SCHEDULE_TICK_S=5 TUMULTD_WEBHOOK_TICK_S=3 TUMULTD_GAMEDAY_TICK_S=3 \
#   TUMULTD_WEBHOOK_ALLOW_INSECURE=1 TUMULTD_WEBHOOK_ALLOW_LOCAL=1 \
#   target/debug/tumultd
#
# The last two flags let the webhook check use a local http receiver — never
# set them for a real deployment.

set -euo pipefail

BASE="${TUMULTD_URL:-http://127.0.0.1:24318}"
TOKEN="${SMOKE_ADMIN_TOKEN:-kro_ui-table-stakes-smoke}"
AUTH="Authorization: Bearer ${TOKEN}"
CT='content-type: application/json'
RECEIVER_PORT="${SMOKE_RECEIVER_PORT:-18444}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

c_green=$'\033[32m'; c_red=$'\033[31m'; c_reset=$'\033[0m'
pass() { printf "  %sPASS%s  %s\n" "$c_green" "$c_reset" "$1"; }
fail() { printf "  %sFAIL%s  %s\n" "$c_red" "$c_reset" "$1"; FAILURES=$((FAILURES + 1)); }
info() { printf "  ----  %s\n" "$1"; }
FAILURES=0

get()  { curl -sf -H "$AUTH" "$BASE$1"; }
post() { curl -sf -X POST -H "$AUTH" -H "$CT" -d "$2" "$BASE$1"; }
# status METHOD PATH [BODY] — print just the HTTP status code.
status() {
  if [[ $# -ge 3 ]]; then
    curl -s -o /dev/null -w '%{http_code}' -X "$1" -H "$AUTH" -H "$CT" -d "$3" "$BASE$2"
  else
    curl -s -o /dev/null -w '%{http_code}' -X "$1" -H "$AUTH" "$BASE$2"
  fi
}
check() { # check NAME CONDITION-CMD...
  local name="$1"; shift
  if "$@" >/dev/null 2>&1; then pass "$name"; else fail "$name"; fi
}
# pyjson PYTHON-EXPR — evaluate a python snippet with the JSON document on
# stdin bound to `d` (already parsed).
pyjson() {
  python3 -c "import json, sys
d = json.load(sys.stdin)
$1"
}

echo "=================================================================="
echo " UI table-stakes smoke — ${BASE}"
echo "=================================================================="

# Clean up leftovers from a previous (possibly crashed) run, so duplicate
# names never confuse the signature and presence checks.
for wid in $(get /api/webhooks | python3 -c 'import json,sys
print(" ".join(w["id"] for w in json.load(sys.stdin)["webhooks"] if w["name"] == "smoke-hook"))' 2>/dev/null || true); do
  post "/api/webhooks/$wid/delete" '{}' > /dev/null
done
for sid in $(get /api/schedules | python3 -c 'import json,sys
print(" ".join(s["id"] for s in json.load(sys.stdin)["schedules"] if s["name"] == "smoke minutely"))' 2>/dev/null || true); do
  post "/api/schedules/$sid/delete" '{}' > /dev/null
done

# ── 1 · users: CRUD + RBAC boundary ──────────────────────────────
echo ""
echo "Users"
CREATED=$(status POST /api/users '{"username":"smoke-viewer","role":"viewer"}' 2>/dev/null || true)
if [[ "$CREATED" == "201" || "$CREATED" == "409" ]]; then
  pass "create smoke-viewer (201, or exists from a previous run)"
else
  fail "create smoke-viewer ($CREATED)"
fi
# The one-time password path: the viewer must change it before its tokens
# work (must_change gates every other route).
OTP=$(get /api/users | python3 -c 'import json,sys
users = json.load(sys.stdin)["users"]
print(next((u["id"] for u in users if u["username"] == "smoke-viewer"), ""))')
[[ -n "$OTP" ]] && pass "smoke-viewer listed with id" || fail "smoke-viewer listed"
VIEWER_ID="$OTP"
USERS_JSON=$(get /api/users)
check "GET /api/users hides password hashes" bash -c '! grep -q password_hash <<< "$0"' "$USERS_JSON"
# One-time password flow: reset to a known password via admin, then the
# viewer changes it (clears must_change).
post "/api/users/$VIEWER_ID/password" '{"password":"smoke-viewer-otp-1"}' > /dev/null
curl -sf -c "$WORK/jar" -X POST -H "$CT" \
  -d '{"username":"smoke-viewer","password":"smoke-viewer-otp-1"}' \
  "$BASE/api/auth/login" > /dev/null
curl -sf -b "$WORK/jar" -X POST -H "$CT" \
  -d '{"current_password":"smoke-viewer-otp-1","new_password":"smoke-viewer-pw-12"}' \
  "$BASE/api/auth/change-password" > /dev/null \
  && pass "viewer completed the must_change flow" || fail "viewer must_change flow"
VIEWER_TOKEN=$(post /api/tokens "{\"name\":\"smoke-viewer-token\",\"user_id\":\"$VIEWER_ID\"}" \
  | sed -n 's/.*"token":"\([^"]*\)".*/\1/p')
[[ "$VIEWER_TOKEN" == kro_* ]] && pass "viewer token minted" || fail "viewer token minted"
vstatus() { curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $VIEWER_TOKEN" "$BASE$1"; }
check "viewer gets 403 on GET /api/users" test "$(vstatus /api/users)" = "403"
check "viewer gets 200 on GET /api/events (Viewer read)" test "$(vstatus /api/events)" = "200"
check "viewer gets 403 on POST /api/webhooks (Admin)" \
  test "$(curl -s -o /dev/null -w '%{http_code}' -X POST -H "Authorization: Bearer $VIEWER_TOKEN" -H "$CT" -d '{"name":"x","url":"https://hooks.example.com/x"}' "$BASE/api/webhooks")" = "403"

# ── 2 · tokens: list, mint, revoke ───────────────────────────────
echo ""
echo "Tokens"
TOK_ID=$(post /api/tokens '{"name":"smoke-ci"}' | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
[[ -n "$TOK_ID" ]] && pass "mint token returns id" || fail "mint token"
TOKENS_JSON=$(get /api/tokens)
check "GET /api/tokens lists it with owner, no hash" bash -c '
  grep -q "\"name\":\"smoke-ci\"" <<< "$0" &&
  grep -q "\"username\":\"admin\"" <<< "$0" &&
  ! grep -q token_hash <<< "$0"' "$TOKENS_JSON"
REVOKED=$(post "/api/tokens/$TOK_ID/revoke" '{}')
check "revoke" bash -c 'grep -q "\"ok\":true" <<< "$0"' "$REVOKED"
TOKENS_JSON=$(get /api/tokens)
check "revoked token still listed, flagged" bash -c 'grep -q "\"revoked\":true" <<< "$0"' "$TOKENS_JSON"

# ── 3 · dry-run scope (blast-radius preview) ─────────────────────
echo ""
echo "Dry-run scope"
read -r -d '' SCOPE_TOON << 'TOON' || true
title: smoke scope experiment
blast_radius: smoke targets only
method[1]:
  - name: pause db
    activity_type: action
    provider:
      type: native
      plugin: docker
      function: pause
      arguments:
        container: smoke-postgres
rollbacks[1]:
  - name: unpause db
    activity_type: action
    provider:
      type: native
      plugin: docker
      function: unpause
      arguments:
        container: smoke-postgres
TOON
REG=$(post /api/runs/validate "$(python3 -c 'import json,sys; print(json.dumps({"toon": sys.argv[1]}))' "$SCOPE_TOON")" \
  | sed -n 's/.*"registry_id":"\([^"]*\)".*/\1/p')
[[ "$REG" == reg-* ]] && pass "definition registered ($REG)" || fail "register definition"
SCOPE=$(post /api/runs/dry-run "{\"registry_id\":\"$REG\"}")
check "scope carries the declared blast radius" \
  bash -c 'grep -q "\"blast_radius\":\"smoke targets only\"" <<< "$0"' "$SCOPE"
check "scope action targets name the container" \
  bash -c 'grep -q "\"container\":\"smoke-postgres\"" <<< "$0"' "$SCOPE"
check "scope excludes probes from actions" \
  pyjson "assert len(d['plan']['scope']['actions']) == 1" <<< "$SCOPE"

# ── 4 · webhooks: create first so later events are captured ──────
echo ""
echo "Webhooks"
cat > "$WORK/receiver.py" << 'PY'
from http.server import BaseHTTPRequestHandler, HTTPServer
import sys
class H(BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length", 0)))
        with open(sys.argv[2], "a") as f:
            f.write(self.headers.get("x-tumult-signature", "") + "\n" + body.decode() + "\n")
        self.send_response(200); self.end_headers(); self.wfile.write(b"ok")
    def log_message(self, *a):
        pass
HTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
PY
python3 "$WORK/receiver.py" "$RECEIVER_PORT" "$WORK/hits.txt" &
RECEIVER_PID=$!
sleep 1
HOOK_SECRET=$(post /api/webhooks "{\"name\":\"smoke-hook\",\"url\":\"http://127.0.0.1:$RECEIVER_PORT/hook\"}" \
  | sed -n 's/.*"secret":"\([^"]*\)".*/\1/p')
[[ ${#HOOK_SECRET} -eq 64 ]] && pass "webhook created, secret shown once (64 hex)" || fail "create webhook"
HOOKS_JSON=$(get /api/webhooks)
check "list webhooks without secrets" bash -c '
  grep -q "\"name\":\"smoke-hook\"" <<< "$0" &&
  ! grep -q "\"secret\"" <<< "$0"' "$HOOKS_JSON"

# ── 5 · global halt ──────────────────────────────────────────────
echo ""
echo "Global halt"
RUN_ID=$(post /api/runs "{\"registry_id\":\"$REG\",\"env\":\"dev\"}" | sed -n 's/.*"run_id":"\([^"]*\)".*/\1/p')
[[ -n "$RUN_ID" ]] && pass "gated run parked in pending_approval" || fail "create gated run"
HALT=$(post /api/runs/stop-all '{}')
check "stop-all reports the halted run" \
  pyjson "assert d['stopped'] >= 1" <<< "$HALT"
sleep 2
RUN_JSON=$(get "/api/runs/$RUN_ID")
check "halted run is terminal (aborted)" bash -c 'grep -q "\"state\":\"aborted\"" <<< "$0"' "$RUN_JSON"
check "stop_requested audit names the halting actor" bash -c '
  grep -q "\"event\":\"stop_requested\"" <<< "$0" &&
  grep -q "\"actor\":\"admin\"" <<< "$0"' "$RUN_JSON"
HALT2=$(post /api/runs/stop-all '{}')
check "stop-all is a no-op when idle" bash -c 'grep -q "\"stopped\":0" <<< "$0"' "$HALT2"

# ── 6 · schedules: CRUD + a real fire ────────────────────────────
echo ""
echo "Schedules"
check "interval below the 60s floor is rejected" \
  test "$(status POST /api/schedules "{\"name\":\"too fast\",\"registry_id\":\"$REG\",\"interval_s\":30}")" = "400"
SCHED_ID=$(post /api/schedules "{\"name\":\"smoke minutely\",\"registry_id\":\"$REG\",\"interval_s\":60}" \
  | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
[[ -n "$SCHED_ID" ]] && pass "schedule created (60s interval)" || fail "create schedule"
SCHED_JSON=$(get /api/schedules)
check "listed as enabled with definition name" bash -c '
  grep -q "\"enabled\":true" <<< "$0" &&
  grep -q "\"definition_name\":\"smoke scope experiment\"" <<< "$0"' "$SCHED_JSON"
post "/api/schedules/$SCHED_ID/enable" '{"enabled":false}' > /dev/null
SCHED_JSON=$(get /api/schedules)
check "disable flips the flag" bash -c 'grep -q "\"enabled\":false" <<< "$0"' "$SCHED_JSON"
post "/api/schedules/$SCHED_ID/enable" '{"enabled":true}' > /dev/null
info "waiting up to 90s for the first scheduled fire…"
FIRED=""
for _ in $(seq 1 18); do
  if get "/api/events?limit=50" | grep -q '"actor":"schedule:smoke minutely"'; then FIRED=1; break; fi
  sleep 5
done
[[ -n "$FIRED" ]] && pass "scheduler fired — run audited with actor 'schedule:smoke minutely'" \
  || fail "scheduler fired within 90s"
DEL=$(post "/api/schedules/$SCHED_ID/delete" '{}')
check "delete schedule" bash -c 'grep -q "\"ok\":true" <<< "$0"' "$DEL"
SCHED_JSON=$(get /api/schedules)
check "deleted schedule is gone from the list" bash -c '! grep -q "smoke minutely" <<< "$0"' "$SCHED_JSON"

# ── 7 · events feed ──────────────────────────────────────────────
echo ""
echo "Events"
STOPS=$(get "/api/events?event=stop_requested")
check "feed has the stop_requested from the halt check" \
  bash -c 'grep -q "stop_requested" <<< "$0"' "$STOPS"
EVENTS_JSON=$(get "/api/events?limit=5")
check "feed rows are newest-first with hash-chain links" \
  pyjson "ts = [e['at_ns'] for e in d['events']]
assert ts == sorted(ts, reverse=True), 'not newest-first'
assert all(e['new_hash'] for e in d['events']), 'chain links missing'" <<< "$EVENTS_JSON"

# ── 8 · gameday: validate, launch, first child ───────────────────
echo ""
echo "GameDay"
read -r -d '' GD << 'TOON' || true
title: smoke campaign
description: two trivial steps
experiments[2]:
  - path: one.toon
    compliance_maps[0]:
  - path: two.toon
    compliance_maps[0]:
scoring:
  pass_threshold: 0.5
  mttr_target_s: 30.0
TOON
read -r -d '' EXP_ONE << 'TOON' || true
title: smoke step one
method[1]:
  - name: check-1
    activity_type: probe
    provider:
      type: process
      path: sh
      arguments[2]: "-c", "true"
TOON
read -r -d '' EXP_TWO << 'TOON' || true
title: smoke step two
method[1]:
  - name: check-1
    activity_type: probe
    provider:
      type: process
      path: sh
      arguments[2]: "-c", "true"
TOON
GD_PAYLOAD=$(python3 -c '
import json, sys
print(json.dumps({"toon": sys.argv[1], "experiments": {"one.toon": sys.argv[2], "two.toon": sys.argv[3]}}))' \
  "$GD" "$EXP_ONE" "$EXP_TWO")
GD_ID=$(post /api/gamedays/validate "$GD_PAYLOAD" | sed -n 's/.*"gameday_registry_id":"\([^"]*\)".*/\1/p')
[[ "$GD_ID" == reg-* ]] && pass "gameday registered ($GD_ID)" || fail "validate gameday"
GDS_JSON=$(get /api/gamedays)
check "gameday listed" bash -c 'grep -q "\"name\":\"smoke campaign\"" <<< "$0"' "$GDS_JSON"
GD_JSON=$(get "/api/gamedays/$GD_ID")
check "gameday detail has two ordered steps" \
  pyjson "names = [s['name'] for s in d['experiments']]
assert names == ['smoke step one', 'smoke step two'], names" <<< "$GD_JSON"
CAMPAIGN=$(post "/api/gamedays/$GD_ID/runs" '{"env":"dev"}')
check "campaign launched (202 with parent run)" \
  pyjson "assert d['steps'] == 2" <<< "$CAMPAIGN"
PARENT=$(echo "$CAMPAIGN" | sed -n 's/.*"run_id":"\([^"]*\)".*/\1/p')
check "second campaign conflicts (409)" \
  test "$(status POST "/api/gamedays/$GD_ID/runs" '{}')" = "409"
info "waiting up to 30s for the first child run…"
CHILD=""
for _ in $(seq 1 6); do
  N=$(get "/api/runs?gameday_id=$PARENT" | python3 -c 'import json,sys; print(json.load(sys.stdin)["count"])')
  [[ "$N" -ge 1 ]] && CHILD=1 && break
  sleep 5
done
[[ -n "$CHILD" ]] && pass "first child run observed (campaign progressing)" || fail "first child run within 30s"

# ── 9 · webhook delivery + signature ─────────────────────────────
echo ""
echo "Webhook delivery"
info "waiting up to 30s for a signed POST at the receiver…"
HIT=""
for _ in $(seq 1 10); do
  [[ -s "$WORK/hits.txt" ]] && HIT=1 && break
  sleep 3
done
[[ -n "$HIT" ]] && pass "receiver got a webhook POST" || fail "webhook POST received"
if [[ -n "$HIT" ]]; then
  SIG=$(head -1 "$WORK/hits.txt")
  BODY=$(sed -n '2p' "$WORK/hits.txt")
  EXPECTED=$(python3 -c 'import hmac, hashlib, sys; print("sha256=" + hmac.new(sys.argv[1].encode(), sys.argv[2].encode(), hashlib.sha256).hexdigest())' \
    "$HOOK_SECRET" "$BODY")
  [[ "$SIG" == "$EXPECTED" ]] && pass "X-Tumult-Signature verifies (HMAC-SHA256)" \
    || fail "signature mismatch: $SIG != $EXPECTED"
fi
kill "$RECEIVER_PID" 2>/dev/null || true

# ── summary ──────────────────────────────────────────────────────
echo ""
echo "------------------------------------------------------------------"
if [[ $FAILURES -eq 0 ]]; then
  pass "ALL CHECKS PASSED"
  echo "------------------------------------------------------------------"
  exit 0
else
  fail "${FAILURES} check(s) failed"
  echo "------------------------------------------------------------------"
  exit 1
fi
