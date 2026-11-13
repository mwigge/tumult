#!/usr/bin/env bash
# Performance-verdict live proof: a real HTTP service is load-tested with
# oha through the CLI under two service.versions, the second build really
# slower than the first; the daemon attributes each run to the version that
# was live, compares the per-window p95 series and answers `regressed`.
# No mocks. Exit non-zero on any failed expectation, 2 when oha is missing.
# Run it three times in a row before a release.
#
#   cargo build -p tumultd -p tumult-cli && (cd web && npm ci && npm run build)
#   bash scripts/proof/rip-phase4/proof.sh [run-number]
#
# Needs oha (https://github.com/hatoo/oha), python3 and curl. Ports
# 45317/45318 (daemon) and 44380 (the service) must be free. Work files
# land under $PROOF_WORK (default: a temp dir).
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

pass() { echo "  ok   $1"; }
fail() { echo "  FAIL $1"; exit 1; }
py() { python3 -c "import json,sys; d=json.load(sys.stdin); $1"; }

DPID=""; SPID=""
cleanup() { kill $DPID $SPID 2>/dev/null || true; }
trap cleanup EXIT

# Start (or restart, as a new build) the checkout service with a delay.
deploy() {
    if [[ -n "$SPID" ]]; then kill "$SPID"; wait "$SPID" 2>/dev/null || true; fi
    python3 "$S/target.py" "$SVC_PORT" "$1" >"$WORK/service.log" 2>&1 &
    SPID=$!
    for i in $(seq 1 40); do curl -fs "$SVC/" >/dev/null 2>&1 && return 0; sleep 0.25; done
    cat "$WORK/service.log"; fail "checkout service did not come up"
}

# The service's own telemetry: one server span under a service.version.
emit_span() {
    local code
    code=$(python3 "$S/otlp_span.py" checkout "$1" demo |
        curl -s -o /dev/null -w '%{http_code}' -X POST \
            -H 'content-type: application/x-protobuf' --data-binary @- "$URL/v1/traces")
    [[ "$code" == 200 ]] || fail "OTLP/HTTP export of a $1 span answered $code"
}

# One oha soak through the CLI; $1 names the journal.
soak() {
    "$BIN/tumult" run "$S/checkout-soak.toon" --journal-path "$WORK/$1.toon" --force \
        >"$WORK/$1.out" 2>&1 || { cat "$WORK/$1.out"; fail "$1 run errored"; }
    grep -q 'status: completed' "$WORK/$1.toon" || { cat "$WORK/$1.out"; fail "$1 run not completed"; }
    grep -q 'tool: oha' "$WORK/$1.toon" || fail "$1 journal has no oha load result"
    grep -q 'thresholds_met: true' "$WORK/$1.toon" || { cat "$WORK/$1.out"; fail "$1: oha completed no window"; }
}

echo "[1] start tumultd on a fresh lake and the checkout service (build 1.0.0, 2 ms)"
RUST_LOG=warn "$BIN/tumultd" serve >"$WORK/tumultd.log" 2>&1 &
DPID=$!
for i in $(seq 1 60); do curl -fs $URL/healthz >/dev/null 2>&1 && break; sleep 0.5; done
curl -fs $URL/healthz >/dev/null || { cat "$WORK/tumultd.log"; fail "daemon did not come up"; }
deploy 2
pass "daemon healthy, service answering"

echo "[2] no telemetry yet: the target is unknown"
code=$(curl -s -o /dev/null -w '%{http_code}' "$URL/api/perf/checkout")
[[ "$code" == 404 ]] || fail "unknown target should be 404, got $code"
pass "GET /api/perf/checkout is 404 before any service.version is seen"

echo "[3] the service reports service.version 1.0.0 over OTLP/HTTP"
emit_span 1.0.0
sleep 2
curl -fs "$URL/api/perf/checkout" >"$WORK/perf-one.json"
py 'assert d["verdict"]=="inconclusive", d["verdict"]; assert d["baseline"] is None
assert d["candidate"]["version"]=="1.0.0", d["candidate"]; assert "only one service.version" in d["reasons"][0], d["reasons"]
assert d["stored"] is False' <"$WORK/perf-one.json"
pass "one version: inconclusive, nothing to compare against, nothing stored"

if ! command -v oha >/dev/null 2>&1; then
    echo "  this proof needs oha on PATH for the load runs (cargo install oha); stopping before step 4"
    exit 2
fi

export TUMULT_DAEMON_URL=$URL
export TUMULT_OTEL_ENABLED=true
export OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:45317

echo "[4] soak build 1.0.0 with oha through the CLI"
soak run-1.0.0
emit_span 1.0.0
pass "run against 1.0.0 journaled to the daemon"

echo "[5] deploy build 1.1.0 (40 ms per request) and soak it the same way"
deploy 40
emit_span 1.1.0
sleep 1
soak run-1.1.0
emit_span 1.1.0
pass "run against 1.1.0 journaled to the daemon"
sleep 3

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

echo "[7] the answer is deterministic and reads never write"
curl -fs "$URL/api/perf/checkout" >"$WORK/perf-again.json"
python3 -c "import json; a=json.load(open('$WORK/perf.json')); b=json.load(open('$WORK/perf-again.json')); assert a['comparison']==b['comparison'], (a['comparison'],b['comparison']); assert b['history']==[]"
pass "same interval on a second read, history still empty"

echo "[8] tumult perf: exit codes, --json, and --store"
set +e
"$BIN/tumult" perf checkout >"$WORK/cli.out" 2>&1; rc=$?
set -e
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

echo "[9] an environment with no evidence does not borrow demo's"
code=$(curl -s -o /dev/null -w '%{http_code}' "$URL/api/perf/checkout?env=prod")
[[ "$code" == 404 ]] || fail "checkout has no prod telemetry; expected 404, got $code"
pass "env=prod is 404: versions are scoped by environment too"
echo "ALL PERFORMANCE-VERDICT PROOF CHECKS PASSED (run $RUN)"
