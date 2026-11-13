#!/usr/bin/env bash
# Performance-verdict live proof: a real HTTP service is load-tested with
# oha through the CLI under three service.versions — the second really
# slower, the third with a slow tail only — and the daemon attributes each
# run to the version that was live, compares the per-window p95 series and
# answers `regressed` both times. It then proves what a load does to a run
# when it measured nothing, when its target never answers, when the run is
# interrupted and when a guard halts it, and finally restarts the daemon
# with authentication to exercise roles and environment scopes.
# No mocks. Exit non-zero on any failed expectation, 2 when oha is missing.
# Run it three times in a row before a release.
#
#   cargo build -p tumultd -p tumult-cli && (cd web && npm ci && npm run build)
#   bash scripts/proof/rip-phase4/proof.sh [run-number]
#
# Needs oha (https://github.com/hatoo/oha), python3 and curl. Ports
# 45317/45318 (daemon), 44380 (the service) and 44381 (a service that never
# answers) must be free. Work files land under $PROOF_WORK (default: a temp
# dir). Nothing sleeps for a fixed time: every wait polls for its condition.
set -euo pipefail
S="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$S/../../.." && pwd)"
BIN=${TUMULT_BIN_DIR:-$REPO/target/debug}
RUN=${1:-1}
WORK=${PROOF_WORK:-$(mktemp -d)}/run-$RUN; rm -rf "$WORK"; mkdir -p "$WORK"
export TUMULT_LAKE_PATH=$WORK/lake.duckdb
export KRONIKA_OTLP_GRPC_ADDR=127.0.0.1:45317
export KRONIKA_OTLP_HTTP_ADDR=127.0.0.1:45318
export KRONIKA_METRICS_DIR=$REPO/metrics
URL=http://127.0.0.1:45318
SVC_PORT=44380
SVC=http://127.0.0.1:$SVC_PORT
HUNG_PORT=44381
CT='content-type: application/json'

pass() { echo "  ok   $1"; }
fail() { echo "  FAIL $1"; exit 1; }
py() { python3 -c "import json,sys; d=json.load(sys.stdin); $1"; }
now_ms() { date +%s%3N; }

DPID=""; SPID=""; HPID=""
cleanup() {
    kill $DPID $SPID $HPID 2>/dev/null || true
    pkill -f "oha .*127.0.0.1:(44380|44381)/" 2>/dev/null || true
}
trap cleanup EXIT

# Poll `$2` (a command) every 100 ms until it succeeds; give up after $3
# seconds (default 30) with `$1` as the failure.
await() {
    local deadline=$(( $(now_ms) + ${3:-30} * 1000 ))
    until eval "$2" >/dev/null 2>&1; do
        [[ $(now_ms) -lt $deadline ]] || fail "timed out waiting for $1"
        sleep 0.1
    done
}

# Poll a daemon GET until the python condition over its JSON body `d` holds.
await_json() {
    await "$1" "curl -fs '$URL$2' | python3 -c 'import json,sys; d=json.load(sys.stdin); sys.exit(0 if ($3) else 1)'"
}

start_daemon() {
    RUST_LOG=warn "$BIN/tumultd" serve >>"$WORK/tumultd.log" 2>&1 &
    DPID=$!
    # /healthz answers 401 rather than 200 once the daemon has users.
    await "the daemon to answer /healthz" "[[ \$(curl -s -o /dev/null -w '%{http_code}' $URL/healthz) =~ ^(200|401)\$ ]]" 60
}

# Start (or restart, as a new build) the checkout service: a base delay
# and optionally every Nth request slow.
deploy() {
    if [[ -n "$SPID" ]]; then kill "$SPID"; wait "$SPID" 2>/dev/null || true; fi
    python3 "$S/target.py" "$SVC_PORT" "$@" >"$WORK/service.log" 2>&1 &
    SPID=$!
    await "the checkout service" "curl -fs $SVC/" 10
}

# A service's own telemetry: one server span under a service.version.
emit_span() {
    local code
    code=$(python3 "$S/otlp_span.py" "$@" |
        curl -s -o /dev/null -w '%{http_code}' -X POST \
            -H 'content-type: application/x-protobuf' --data-binary @- "$URL/v1/traces")
    [[ "$code" == 200 ]] || fail "OTLP/HTTP export of a $1 $2 span answered $code"
}

# The daemon has seen service.version $2 on target $1.
await_version() {
    await_json "service.version $2 of $1" "/api/perf/$1" '"'"$2"'" in [v["version"] for v in d["versions_seen"]]'
}

# One run through the CLI; $1 names the journal, $2 the experiment. Prints
# nothing; the exit code is the run's.
run_toon() {
    "$BIN/tumult" run "$S/$2.toon" --journal-path "$WORK/$1.toon" --force >"$WORK/$1.out" 2>&1
}

# One oha soak through the CLI. The exit code alone decides: a load that
# measured nothing or missed its thresholds no longer ends `completed`.
soak() {
    run_toon "$1" checkout-soak || { cat "$WORK/$1.out"; fail "$1 run did not complete"; }
}

# A run that must not complete: $1 journal, $2 experiment, $3 the status.
run_expecting() {
    local rc=0
    run_toon "$1" "$2" || rc=$?
    [[ $rc -ne 0 ]] || { cat "$WORK/$1.out"; fail "$2 must exit non-zero"; }
    grep -q "^status: $3\$" "$WORK/$1.toon" || { cat "$WORK/$1.out"; fail "$2 should end $3"; }
}

journal_field() { sed -n "s/^  $2: //p" "$WORK/$1.toon" | head -1; }
windows_in() { sed -n 's/^  samples\[\([0-9]*\)\].*/\1/p' "$WORK/$1.toon" | head -1; }
oha_running() { pgrep -f "oha .*127.0.0.1:$1/" >/dev/null 2>&1; }
status_of() { curl -s -o "$WORK/last.json" -w '%{http_code}' "$@"; }

echo "[1] start tumultd on a fresh lake and the checkout service (build 1.0.0, 2 ms)"
start_daemon
deploy 2
pass "daemon healthy, service answering"

echo "[2] no telemetry yet: the target is unknown"
code=$(status_of "$URL/api/perf/checkout")
[[ "$code" == 404 ]] || fail "unknown target should be 404, got $code"
pass "GET /api/perf/checkout is 404 before any service.version is seen"

echo "[3] the service reports service.version 1.0.0 over OTLP/HTTP"
emit_span checkout 1.0.0 demo
await_version checkout 1.0.0
curl -fs "$URL/api/perf/checkout" >"$WORK/perf-one.json"
py 'assert d["verdict"]=="inconclusive", d["verdict"]; assert d["baseline"] is None
assert d["candidate"]["version"]=="1.0.0", d["candidate"]; assert "only one service.version" in d["reasons"][0], d["reasons"]
assert d["stored"] is False' <"$WORK/perf-one.json"
code=$(status_of -X POST "$URL/api/perf/checkout/verdicts")
[[ "$code" == 409 ]] || fail "storing with one version should be 409, got $code"
pass "one version: inconclusive, nothing to compare against, and nothing to store (409)"

if ! command -v oha >/dev/null 2>&1; then
    echo "  this proof needs oha on PATH for the load runs (cargo install oha); stopping before step 4"
    exit 2
fi

export TUMULT_DAEMON_URL=$URL
export TUMULT_OTEL_ENABLED=true
export OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:45317

echo "[4] soak build 1.0.0 with oha through the CLI"
soak run-1.0.0
emit_span checkout 1.0.0 demo
await_json "the 1.0.0 run's windows" /api/perf/checkout 'd["candidate"]["samples"]>=20'
pass "run against 1.0.0 completed and reached the daemon ($(windows_in run-1.0.0) windows)"

echo "[5] deploy build 1.1.0 (40 ms per request) and soak it the same way"
deploy 40
emit_span checkout 1.1.0 demo
await_version checkout 1.1.0
soak run-1.1.0
emit_span checkout 1.1.0 demo
await_json "the 1.1.0 run's windows" /api/perf/checkout 'd["candidate"]["version"]=="1.1.0" and d["candidate"]["samples"]>=20'
pass "run against 1.1.0 completed and reached the daemon ($(windows_in run-1.1.0) windows)"

echo "[6] the verdict: two versions, each with its own run, 1.1.0 regressed"
curl -fs "$URL/api/perf/checkout" >"$WORK/perf.json"
py 'vs=[v["version"] for v in d["versions_seen"]]; assert sorted(vs)==["1.0.0","1.1.0"], vs
b,c=d["baseline"],d["candidate"]
assert b["version"]=="1.0.0" and c["version"]=="1.1.0", (b,c)
assert len(b["runs"])==1 and len(c["runs"])==1 and b["runs"]!=c["runs"], (b["runs"],c["runs"])
assert b["samples"]>=20 and c["samples"]>=20, (b["samples"],c["samples"])
assert d["probe"]=="load.latency_p95_ms"
k=d["comparison"]; assert k["quantile"]=="p95" and k["ci_low"]>k["margin"]>0, k
assert c["p95"]>b["p95"]+20, (b["p95"],c["p95"])
assert d["verdict"]=="regressed", (d["verdict"],d["reasons"])
assert d["stored"] is False and d["history"]==[], (d["stored"],d["history"])
print("     baseline 1.0.0 p95 %.1f ms over %d windows; candidate 1.1.0 p95 %.1f ms over %d windows; delta %+.1f ms, CI [%.1f, %.1f], margin %.1f" % (b["p95"],b["samples"],c["p95"],c["samples"],k["delta"],k["ci_low"],k["ci_high"],k["margin"]))' <"$WORK/perf.json"
pass "regressed: the whole interval lies above the margin; the read stored nothing"

echo "[7] the answer is deterministic and a GET never writes, whatever its query says"
curl -fs "$URL/api/perf/checkout?store=true" >"$WORK/perf-again.json"
python3 -c "import json; a=json.load(open('$WORK/perf.json')); b=json.load(open('$WORK/perf-again.json')); assert a['comparison']==b['comparison'], (a['comparison'],b['comparison']); assert b['stored'] is False and b['history']==[]"
curl -fs "$URL/api/perf/checkout" | py 'assert d["history"]==[], d["history"]'
pass "same interval on a second read; GET ?store=true stored nothing"

echo "[8] tumult perf: exit codes, --json, and --store through POST /api/perf/{target}/verdicts"
rc=0; "$BIN/tumult" perf checkout >"$WORK/cli.out" 2>&1 || rc=$?
[[ $rc -ne 0 ]] || { cat "$WORK/cli.out"; fail "tumult perf must exit non-zero on a regression"; }
grep -q 'perf checkout: REGRESSED' "$WORK/cli.out" || { cat "$WORK/cli.out"; fail "tumult perf did not print the verdict"; }
"$BIN/tumult" perf checkout --json --fail-on never >"$WORK/cli.json" 2>"$WORK/cli.err" || { cat "$WORK/cli.err"; fail "tumult perf --json --fail-on never errored"; }
py 'assert d["verdict"]=="regressed" and d["stored"] is False and len(d["versions_seen"])==2, d' <"$WORK/cli.json"
"$BIN/tumult" perf checkout --json --fail-on never --store --env demo >"$WORK/cli-store.json" 2>"$WORK/cli.err" || { cat "$WORK/cli.err"; fail "tumult perf --store errored"; }
py 'assert d["verdict"]=="regressed" and d["stored"] is True, (d["verdict"],d["stored"])' <"$WORK/cli-store.json"
curl -fs "$URL/api/perf/checkout" >"$WORK/perf-history.json"
py 'h=d["history"]; assert len(h)==1, h
assert h[0]["verdict"]=="regressed" and h[0]["baseline_version"]=="1.0.0" and h[0]["candidate_version"]=="1.1.0", h[0]
assert h[0]["envs"]==["demo"], h[0]["envs"]' <"$WORK/perf-history.json"
pass "non-zero exit on regressed, JSON answer, and --store appended one row scoped to demo"

echo "[9] build 1.2.0 keeps the 40 ms but every fifth request takes 0.6 s: a slow tail"
deploy 40 5 600
emit_span checkout 1.2.0 demo
await_version checkout 1.2.0
PROOF_HOLD_S=45 soak run-1.2.0
emit_span checkout 1.2.0 demo
# Each window now lasts until its slow requests are answered, so the 24
# windows take longer and the method holds for longer (PROOF_HOLD_S).
await_json "the 1.2.0 run's windows" /api/perf/checkout 'd["candidate"]["version"]=="1.2.0" and d["candidate"]["samples"]>=20'
curl -fs "$URL/api/perf/checkout" >"$WORK/perf-tail.json"
py 'b,c=d["baseline"],d["candidate"]
assert b["version"]=="1.1.0" and c["version"]=="1.2.0", (b["version"],c["version"])
assert len(c["runs"])==1 and len(b["runs"])==1, (b["runs"],c["runs"])
assert d["verdict"]!="unchanged", "the slow tail was censored: %s" % d["reasons"]
assert d["verdict"]=="regressed", (d["verdict"],d["reasons"])
assert c["p95"]>500, c
print("     baseline 1.1.0 p95 %.1f ms; candidate 1.2.0 p95 %.1f ms over %d windows: %s" % (b["p95"],c["p95"],c["samples"],d["verdict"]))' <"$WORK/perf-tail.json"
[[ "$(journal_field run-1.2.0 error_rate)" == 0 ]] || fail "the slow requests were answered, not errors: error_rate $(journal_field run-1.2.0 error_rate)"
pass "requests still in flight at a window's end are waited for and measured: regressed, not unchanged"

echo "[10] a target that accepts and never answers: requests, no answers, thresholds unmet"
python3 "$S/target.py" "$HUNG_PORT" 0 1 0 >"$WORK/hung.log" 2>&1 &
HPID=$!
await "the hung service's port" "python3 -c 'import socket; socket.create_connection((\"127.0.0.1\", $HUNG_PORT), 1).close()'" 10
TUMULT_OHA_REQUEST_TIMEOUT_S=1 run_expecting run-hung checkout-hung deviated
[[ "$(journal_field run-hung total_requests)" -gt 0 ]] || fail "the hung run should count its timed-out requests"
[[ "$(journal_field run-hung error_rate)" == 1 ]] || fail "every hung request is an error, got $(journal_field run-hung error_rate)"
[[ "$(journal_field run-hung thresholds_met)" == false ]] || fail "a load nobody answered cannot meet thresholds"
pass "all-hung target: $(journal_field run-hung total_requests) requests, error rate 1, thresholds unmet, run deviated, exit non-zero"

echo "[11] a load that cannot send one request fails the run"
run_expecting run-unreachable checkout-unreachable failed
[[ "$(journal_field run-unreachable total_requests)" == 0 ]] || fail "expected zero requests, got $(journal_field run-unreachable total_requests)"
[[ "$(journal_field run-unreachable thresholds_met)" == false ]] || fail "zero requests cannot meet thresholds"
pass "zero requests: thresholds unmet, run failed, exit non-zero"

echo "[12] SIGINT during a ten-minute soak: the load ends at once, the rollback runs"
export PROOF_ROLLBACK_MARK=$WORK/interrupt-rolled-back
"$BIN/tumult" run "$S/checkout-interrupt.toon" --journal-path "$WORK/run-interrupt.toon" --force >"$WORK/run-interrupt.out" 2>&1 &
TPID=$!
# Wait for the first window to finish: its oha is replaced by the next one.
await "the first oha window" "oha_running $SVC_PORT" 20
first=$(pgrep -f "oha .*127.0.0.1:$SVC_PORT/" | head -1)
await "the second oha window" "oha_running $SVC_PORT && [[ \$(pgrep -f 'oha .*127.0.0.1:$SVC_PORT/' | head -1) != $first ]]" 20
t0=$(now_ms)
kill -INT "$TPID"
rc=0; wait "$TPID" || rc=$?
took=$(( $(now_ms) - t0 ))
[[ $rc -ne 0 ]] || fail "an interrupted run must exit non-zero"
[[ $took -lt 5000 ]] || fail "the interrupt took ${took} ms; the load was waited for"
oha_running "$SVC_PORT" && fail "oha is still hitting the service after the interrupt"
grep -q '^status: interrupted$' "$WORK/run-interrupt.toon" || { cat "$WORK/run-interrupt.out"; fail "the run should end interrupted"; }
[[ -e "$PROOF_ROLLBACK_MARK" ]] || fail "the rollback did not run after the interrupt"
kept=$(windows_in run-interrupt)
[[ "${kept:-0}" -ge 1 ]] || fail "the windows completed before the interrupt were lost"
pass "interrupted in ${took} ms of a 600 s plan: no oha left, rollback ran, $kept completed window(s) journaled"

echo "[13] a guard breach during a ten-minute soak halts it the same way"
export PROOF_ROLLBACK_MARK=$WORK/halt-rolled-back PROOF_BREACH_FLAG=$WORK/breach-flag
t0=$(now_ms)
run_expecting run-halt checkout-halt halted
took=$(( $(now_ms) - t0 ))
[[ $took -lt 30000 ]] || fail "the halted run took ${took} ms; the load was waited for"
oha_running "$SVC_PORT" && fail "oha is still hitting the service after the halt"
[[ -e "$PROOF_ROLLBACK_MARK" ]] || fail "the rollback did not run after the halt"
kept=$(windows_in run-halt)
[[ "${kept:-0}" -ge 1 ]] || fail "the windows completed before the halt were lost"
pass "halted: the whole run took ${took} ms of a 600 s plan, no oha left, rollback ran, $kept window(s) journaled"

echo "[14] environments: exact match, three attributes, and an honest 404"
code=$(status_of "$URL/api/perf/checkout?env=prod")
[[ "$code" == 404 ]] || fail "checkout has no prod telemetry; expected 404, got $code"
py 'e=d["error"]; assert "in environment prod" in e and "deployment.environment.name" in e, e' <"$WORK/last.json"
code=$(status_of "$URL/api/perf/checkout?env=")
[[ "$code" == 400 ]] || fail "an empty env should be 400, got $code"
emit_span ledger 3.0.0 -
await_version ledger 3.0.0
code=$(status_of "$URL/api/perf/ledger?env=demo")
[[ "$code" == 404 ]] || fail "an untagged service is in no environment; expected 404, got $code"
py 'e=d["error"]; assert "target ledger in environment demo" in e and "resilience.target.environment" in e, e' <"$WORK/last.json"
emit_span billing 7.0.0 demo deployment.environment.name
emit_span billing 7.1.0 demo deployment.environment
await_json "billing's two versions in demo" "/api/perf/billing?env=demo" 'len(d["versions_seen"])==2'
pass "env=prod and an untagged service are 404 with the reason; env= is 400; both deployment.environment spellings count"

echo "[15] restart the daemon with authentication: roles and scopes on the same lake"
kill "$DPID"; wait "$DPID" 2>/dev/null || true
ADMIN=kro_$(python3 -c 'import secrets; print(secrets.token_hex(24))')
export KRONIKA_BOOTSTRAP_ADMIN_PASSWORD=proof-admin-password KRONIKA_BOOTSTRAP_TOKEN=$ADMIN
start_daemon
admin() { curl -fs -H "Authorization: Bearer $ADMIN" -H "$CT" "$@"; }
# Create a user with a role and one environment scope, take it through the
# forced password change, and mint it a token (printed).
mint() {
    local id
    id=$(admin -X POST -d "{\"username\":\"$1\",\"password\":\"$1-first-password\",\"role\":\"$2\",\"env_scopes\":[\"$3\"]}" "$URL/api/users" | py 'print(d["id"])')
    curl -fs -c "$WORK/$1.jar" -X POST -H "$CT" -d "{\"username\":\"$1\",\"password\":\"$1-first-password\"}" "$URL/api/auth/login" >/dev/null
    curl -fs -b "$WORK/$1.jar" -X POST -H "$CT" -d "{\"current_password\":\"$1-first-password\",\"new_password\":\"$1-second-password\"}" "$URL/api/auth/change-password" >/dev/null
    admin -X POST -d "{\"name\":\"$1-proof\",\"user_id\":\"$id\"}" "$URL/api/tokens" | py 'print(d["token"])'
}
VIEWER=$(mint proof-viewer viewer demo)
OPERATOR=$(mint proof-operator operator demo)
OUTSIDER=$(mint proof-outsider viewer prod)
as() { local token=$1; shift; status_of -H "Authorization: Bearer $token" "$@"; }
code=$(status_of "$URL/api/perf/checkout")
[[ "$code" == 401 ]] || fail "an anonymous read should be 401 once users exist, got $code"
code=$(as "$VIEWER" "$URL/api/perf/checkout")
[[ "$code" == 200 ]] || fail "the demo viewer should read the verdict, got $code"
py 'assert d["verdict"]=="regressed" and len(d["history"])==1 and d["history"][0]["envs"]==["demo"], (d["verdict"],d["history"])' <"$WORK/last.json"
code=$(as "$VIEWER" -X POST "$URL/api/perf/checkout/verdicts")
[[ "$code" == 403 ]] || fail "a viewer storing a verdict should be 403, got $code"
rc=0; TUMULT_DAEMON_TOKEN=$VIEWER "$BIN/tumult" perf checkout --store --fail-on never >"$WORK/cli-viewer.out" 2>&1 || rc=$?
[[ $rc -ne 0 ]] || { cat "$WORK/cli-viewer.out"; fail "tumult perf --store must exit non-zero when nothing was stored"; }
grep -q '403' "$WORK/cli-viewer.out" || { cat "$WORK/cli-viewer.out"; fail "tumult perf --store should report the 403"; }
as "$VIEWER" "$URL/api/perf/checkout" >/dev/null
py 'assert len(d["history"])==1, d["history"]' <"$WORK/last.json"
code=$(as "$OPERATOR" -X POST "$URL/api/perf/checkout/verdicts?env=prod")
[[ "$code" == 403 ]] || fail "an operator outside prod should be 403 there, got $code"
code=$(as "$OPERATOR" -X POST "$URL/api/perf/checkout/verdicts")
[[ "$code" == 201 ]] || fail "the demo operator should store the verdict, got $code"
py 'assert d["stored"] is True and d["candidate"]["version"]=="1.2.0", (d["stored"],d["candidate"])' <"$WORK/last.json"
rc=0; TUMULT_DAEMON_TOKEN=$OPERATOR "$BIN/tumult" perf checkout --store --fail-on never >"$WORK/cli-operator.out" 2>&1 || rc=$?
[[ $rc -eq 0 ]] || { cat "$WORK/cli-operator.out"; fail "tumult perf --store as operator should succeed"; }
grep -q 'verdict stored' "$WORK/cli-operator.out" || { cat "$WORK/cli-operator.out"; fail "tumult perf --store should say it stored"; }
as "$VIEWER" "$URL/api/perf/checkout" >/dev/null
py 'h=d["history"]; assert len(h)==3 and all(r["envs"]==["demo"] for r in h), h
assert h[0]["candidate_version"]=="1.2.0" and h[2]["candidate_version"]=="1.1.0", h' <"$WORK/last.json"
code=$(as "$OUTSIDER" "$URL/api/perf/checkout")
[[ "$code" == 404 ]] || fail "a prod-scoped viewer must not see demo's target, got $code"
py 'e=d["error"]; assert "in environment prod" in e, e' <"$WORK/last.json"
code=$(as "$OUTSIDER" "$URL/api/perf/checkout?env=demo")
[[ "$code" == 403 ]] || fail "a prod-scoped viewer asking for demo should be 403, got $code"
pass "401 anonymous; viewer reads but gets 403 storing (CLI exits $rc for the operator, non-zero for the viewer); operator stores in demo only; history scoped; prod sees nothing"
echo "ALL PERFORMANCE-VERDICT PROOF CHECKS PASSED (run $RUN)"
