#!/usr/bin/env bash
# Performance-verdict live proof: a real HTTP service is load-tested with
# oha through the CLI under seven versions, three runs each where a verdict
# is asked for, and the daemon compares the per-window p95 series and the
# share of failed requests of the runs that measured each of them.
#
# How a run gets its version is proven three ways: read from the target's
# spans afterwards because the run recorded none (1.0.0 and 1.1.0, the
# second soaked while the first stays busy in another environment),
# declared on the command line before any span carries it (1.2.0), and
# observed by the runner when the run ends, from a build that reports its
# own telemetry while a second traced build is soaked in another
# environment at the same time (1.3.0 in demo, 2.0.0 in prod). The
# verdicts are `regressed`, `regressed` and `improved`; they follow the
# order of deployment and do not move when an old version reports again.
# One run a side is not enough for a verdict, and the proof shows that too.
#
# A verdict is never better than the evidence: a build that answers fast
# and hangs a third of its requests (1.3.1) is `regressed` on its failure
# share, and a build that answers nothing (1.4.0) is `regressed` too, and
# `tumult perf` exits 3 on both; with the dark build as the baseline the
# verdict is `inconclusive` and the exit code 4.
#
# It then proves what a load does to a run and to the verdict when its
# plan is truncated, when its target never answers (with and without
# thresholds), when it measured nothing, when the run is interrupted and
# when a guard halts a windowed and a flat load; that a version rolled
# back to is the candidate again whatever the range asked for; and finally
# restarts the daemon with authentication to exercise roles, environment
# scopes and the idempotent store.
# No mocks. Exit non-zero on any failed expectation, 2 when a tool is
# missing, 3 when a port the proof chose was taken before it could use it.
# Run it three times in a row before a release.
#
#   cargo build -p tumultd -p tumult-cli && (cd web && npm ci && npm run build)
#   bash scripts/proof/rip-phase4/proof.sh [run-number]
#
# Needs oha (https://github.com/hatoo/oha), python3, curl, setsid and ss
# (iproute2). The five ports (two for the daemon, two for the service's
# builds, one for a service that never answers) are asked of the kernel
# when the proof starts, and the experiments are written for them under
# the work directory. After its health check and before every write the
# proof asks ss who listens on the daemon's two ports: it must be the
# process the proof started. Work files, the lake and the daemon's metrics
# directory land under $PROOF_WORK (default: a temp dir), which must not be
# inside the repository. No step waits a fixed time: every wait polls for
# its condition, and no expectation depends on how long ago a span arrived.
set -euo pipefail

# The proof runs in a session of its own and this outer shell supervises
# it: whatever the proof started and left behind — a daemon, a service, an
# oha whose runner died — is in that session and is ended with it, and
# nothing outside it is ever signalled.
if [[ -z "${PROOF_SESSION:-}" ]]; then
    command -v setsid >/dev/null 2>&1 || { echo "this proof needs setsid (util-linux)"; exit 2; }
    PROOF_SESSION=1 setsid bash "${BASH_SOURCE[0]}" "$@" &
    inner=$!
    end_session() { pkill -TERM -s "$inner" 2>/dev/null || true; }
    trap end_session EXIT
    trap 'end_session; exit 130' INT TERM
    rc=0
    wait "$inner" || rc=$?
    exit "$rc"
fi

S="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$S/../../.." && pwd)"
BIN=${TUMULT_BIN_DIR:-$REPO/target/debug}
RUN=${1:-1}
for tool in python3 curl ss; do
    command -v "$tool" >/dev/null 2>&1 || { echo "this proof needs $tool"; exit 2; }
done
WORK=${PROOF_WORK:-$(mktemp -d)}/run-$RUN; rm -rf "$WORK"; mkdir -p "$WORK/toons" "$WORK/metrics"
WORK=$(cd "$WORK" && pwd -P)
case "$WORK/" in
    "$(cd "$REPO" && pwd -P)"/*) echo "PROOF_WORK ($WORK) is inside the repository; the proof writes nothing there"; exit 2 ;;
esac
export TUMULT_LAKE_PATH=$WORK/lake.duckdb
export KRONIKA_METRICS_DIR=$WORK/metrics
# Five ports nobody listens on, held together while they are read so that
# they differ, and released for the daemon and the services to take.
read -r GRPC_PORT HTTP_PORT SVC_PORT HUNG_PORT PROD_PORT < <(python3 -c '
import socket
held = [socket.socket() for _ in range(5)]
for s in held:
    s.bind(("127.0.0.1", 0))
print(*[s.getsockname()[1] for s in held])')
export KRONIKA_OTLP_GRPC_ADDR=127.0.0.1:$GRPC_PORT
export KRONIKA_OTLP_HTTP_ADDR=127.0.0.1:$HTTP_PORT
URL=http://127.0.0.1:$HTTP_PORT
SVC=http://127.0.0.1:$SVC_PORT
CT='content-type: application/json'
# The experiments name the service by the ports the files were written
# with; the proof runs copies that name the ports it was given.
for toon in "$S"/*.toon; do
    sed -e "s/127\.0\.0\.1:44380/127.0.0.1:$SVC_PORT/g" \
        -e "s/127\.0\.0\.1:44381/127.0.0.1:$HUNG_PORT/g" \
        -e "s/127\.0\.0\.1:44382/127.0.0.1:$PROD_PORT/g" "$toon" >"$WORK/toons/$(basename "$toon")"
done

pass() { echo "  ok   $1"; }
fail() { echo "  FAIL $1"; exit 1; }
py() { python3 -c "import json,sys; d=json.load(sys.stdin); $1"; }
now_ms() { date +%s%3N; }
less_than() { python3 -c "import sys; sys.exit(0 if float('$1') < float('$2') else 1)"; }

DPID=""; SPID=""; HPID=""; BPID=""; PRPID=""
cleanup() { kill $DPID $SPID $HPID $BPID $PRPID 2>/dev/null || true; }
trap cleanup EXIT

# Poll a command (`$3` and its arguments) every 100 ms until it succeeds;
# give up after `$2` seconds with `$1` as the failure.
await() {
    local what=$1 deadline=$(( $(now_ms) + $2 * 1000 )); shift 2
    until "$@" >/dev/null 2>&1; do
        [[ $(now_ms) -lt $deadline ]] || fail "timed out waiting for $what"
        sleep 0.1
    done
}

# The daemon's JSON answer to GET `$1` satisfies the python condition `$2`
# over the body `d`.
json_holds() {
    curl -fs "$URL$1" | python3 -c "import json,sys; d=json.load(sys.stdin); sys.exit(0 if ($2) else 1)"
}
await_json() { await "$1" 60 json_holds "$2" "$3"; }

port_taken() { python3 -c "import socket,sys; socket.create_connection(('127.0.0.1', $1), 1).close()" 2>/dev/null; }
port_free() { ! port_taken "$1"; }

# The listener on port $1 is the daemon this proof started.
listener_is_ours() {
    local held
    held=$(ss -ltnpH "sport = :$1")
    [[ "$held" == *"pid=$DPID,"* ]]
}

# The daemon this proof started is still running and both its ports are
# its own. Asked after its health check and before every write: an answer
# on a port alone does not say whose daemon answered.
daemon_alive() {
    local port
    kill -0 "$DPID" 2>/dev/null || { tail -20 "$WORK/tumultd.log"; fail "the proof's tumultd (pid $DPID) is not running"; }
    for port in "$GRPC_PORT" "$HTTP_PORT"; do
        listener_is_ours "$port" || { ss -ltnpH "sport = :$port"; fail "port $port is not held by the proof's tumultd (pid $DPID)"; }
    done
}

# /healthz answers 401 rather than 200 once the daemon has users.
healthz_answers() { [[ $(curl -s -o /dev/null -w '%{http_code}' "$URL/healthz") =~ ^(200|401)$ ]]; }

start_daemon() {
    RUST_LOG=warn "$BIN/tumultd" serve >>"$WORK/tumultd.log" 2>&1 &
    DPID=$!
    await "the daemon to answer /healthz" 120 healthz_answers
    await "the daemon's OTLP/gRPC listener" 60 listener_is_ours "$GRPC_PORT"
    daemon_alive
}

# Start (or restart, as a new build) the checkout service: a base delay
# and optionally every Nth request slow.
deploy() {
    if [[ -n "$SPID" ]]; then kill "$SPID"; wait "$SPID" 2>/dev/null || true; fi
    python3 "$S/target.py" "$SVC_PORT" "$@" >"$WORK/service.log" 2>&1 &
    SPID=$!
    await "the checkout service" 30 curl -fs "$SVC/ready"
}

# The same, as build $1 that reports its own telemetry: a span for the
# requests it serves, so it says nothing while nobody calls it. /ready is
# not traced.
deploy_traced() {
    local version=$1; shift
    PROOF_TELEMETRY_URL=$URL PROOF_SERVICE_VERSION=$version deploy "$@"
}

# A service's own telemetry: one server span under a service.version.
emit_span() {
    local code
    daemon_alive
    code=$(python3 "$S/otlp_span.py" "$@" |
        curl -s -o /dev/null -w '%{http_code}' -X POST \
            -H 'content-type: application/x-protobuf' --data-binary @- "$URL/v1/traces")
    [[ "$code" == 200 ]] || fail "OTLP/HTTP export of a $1 $2 span answered $code"
}

# The daemon has seen service.version $2 on target $1 in demo.
await_version() {
    await_json "service.version $2 of $1" "/api/perf/$1?env=demo" '"'"$2"'" in [v["version"] for v in d["versions_seen"]]'
}

# Target $1 said nothing in demo in the last three seconds: a run started
# now holds no span of it, although the runner reads two seconds past
# both ends of its run for the clocks' sake.
silent() {
    local now
    now=$(date +%s%N)
    json_holds "/api/perf/$1/version?env=demo&from_ns=$(( now - 3000000000 ))&to_ns=$now" 'd["versions"]==[]'
}
await_silence() { await "$1 to have been silent in demo for three seconds" 60 silent "$1"; }

# The run of journal $1 returned more than three seconds ago. A run that
# recorded no version is given the one its target spoke of during it, read
# two seconds past both ends of its root span: a build that first speaks
# within those seconds of a silent build's run cannot be told from one
# that served it. The root span outlives the journal's `ended_at_ns` by the
# time the runner spent asking for the version, so the clock starts when
# `tumult run` returned.
run_past() {
    [[ $(( $(date +%s%N) - $(<"$WORK/$1.returned") )) -gt 3000000000 ]]
}
await_past() { await "run $1 to have returned three seconds ago" 60 run_past "$1"; }

# One run through the CLI; $1 names the journal, $2 the experiment, the
# rest goes to `tumult run`. Prints nothing; the exit code is the run's.
run_toon() {
    local journal=$1 toon=$2; shift 2
    daemon_alive
    local rc=0
    "$BIN/tumult" run "$WORK/toons/$toon.toon" --journal-path "$WORK/$journal.toon" --force "$@" >"$WORK/$journal.out" 2>&1 || rc=$?
    date +%s%N >"$WORK/$journal.returned"
    return "$rc"
}

# One oha soak of checkout through the CLI. The exit code alone decides: a
# load that measured nothing or missed its thresholds does not end
# `completed`.
soak() {
    local journal=$1; shift
    run_toon "$journal" checkout-soak "$@" || { cat "$WORK/$journal.out"; fail "$journal run did not complete"; }
}
# Three of them, `$1-1` to `$1-3`: a verdict takes three runs a side. With
# FROM=2 the first has been run already.
soaks() {
    local name=$1 i; shift
    for i in $(seq "${FROM:-1}" 3); do soak "$name-$i" "$@"; done
}
# `tumult perf` with the arguments given must exit `$1`; its output is kept
# in `$2`.
perf_exits() {
    local want=$1 out=$2 rc=0; shift 2
    "$BIN/tumult" perf "$@" >"$WORK/$out" 2>&1 || rc=$?
    [[ $rc -eq $want ]] || { cat "$WORK/$out"; fail "tumult perf $* should exit $want, exited $rc"; }
}

# A run that must not complete: $1 journal, $2 experiment, $3 the status,
# the rest goes to `tumult run`.
run_expecting() {
    local journal=$1 toon=$2 status=$3 rc=0; shift 3
    run_toon "$journal" "$toon" "$@" || rc=$?
    [[ $rc -ne 0 ]] || { cat "$WORK/$journal.out"; fail "$toon must exit non-zero"; }
    grep -q "^status: $status\$" "$WORK/$journal.toon" || { cat "$WORK/$journal.out"; fail "$toon should end $status"; }
}

# Field $2 of journal $1's load_result, at any depth (the execution block
# is nested in it).
load_field() {
    awk '/^load_result:/{inside=1; next} /^[^ ]/{inside=0} inside' "$WORK/$1.toon" |
        sed -n "s/^ *$2: //p" | head -1
}
# The version journal $1 recorded as "<version> <source>"; empty when it
# recorded none.
recorded_version() {
    awk '/^target_version:/{inside=1; next} /^[^ ]/{inside=0} inside' "$WORK/$1.toon" |
        sed -n 's/^ *version: //p; s/^ *version_source: //p' | tr -d '"' | paste -sd' ' -
}
windows_in() { sed -n 's/^  samples\[\([0-9]*\)\].*/\1/p' "$WORK/$1.toon" | head -1; }
# An oha of this proof's own session is hitting port $1.
oha_pids() { pgrep -s 0 -f "oha .*127.0.0.1:$1/" 2>/dev/null; }
oha_running() { [[ -n "$(oha_pids "$1")" ]]; }
# An oha other than `$2` is hitting port $1: the next window has begun.
oha_replaced() { oha_running "$1" && [[ $(oha_pids "$1" | head -1) != "$2" ]]; }
status_of() { curl -s -o "$WORK/last.json" -w '%{http_code}' "$@"; }

echo "[1] five free ports; start tumultd on a fresh lake and the checkout service (build 1.0.0, 2 ms)"
for port in "$GRPC_PORT" "$HTTP_PORT" "$SVC_PORT" "$HUNG_PORT" "$PROD_PORT"; do
    if port_taken "$port"; then echo "  port $port was taken after the proof chose it; the proof would talk to somebody else's process"; exit 3; fi
done
start_daemon
deploy 2
pass "ports $GRPC_PORT/$HTTP_PORT (daemon) and $SVC_PORT, $HUNG_PORT, $PROD_PORT (services) chosen now; both daemon listeners belong to the proof's own tumultd (pid $DPID); service answering"

echo "[2] no telemetry yet: the target is unknown"
code=$(status_of "$URL/api/perf/checkout")
[[ "$code" == 404 ]] || fail "unknown target should be 404, got $code"
pass "GET /api/perf/checkout is 404 before any service.version is seen"

echo "[3] the service reports service.version 1.0.0 over OTLP/HTTP"
emit_span checkout 1.0.0 demo
await_version checkout 1.0.0
curl -fs "$URL/api/perf/checkout" >"$WORK/perf-one.json"
py 'assert d["verdict"]=="inconclusive", d["verdict"]; assert d["baseline"] is None
assert d["environment"]=="demo" and d["selection"]=="latest_deployment", (d["environment"],d["selection"])
assert d["candidate"]["version"]=="1.0.0", d["candidate"]; assert d["reason"]=="no_baseline", (d["reason"],d["reasons"])
assert d["backfill"]=="done", d["backfill"]
assert d["stored"] is False' <"$WORK/perf-one.json"
daemon_alive
code=$(status_of -X POST "$URL/api/perf/checkout/verdicts")
[[ "$code" == 409 ]] || fail "storing with one version should be 409, got $code"
pass "one version: inconclusive, nothing to compare against, and nothing to store (409)"

if ! command -v oha >/dev/null 2>&1; then
    echo "  this proof needs oha on PATH for the load runs (cargo install oha); stopping before step 4"
    exit 2
fi

export TUMULT_DAEMON_URL=$URL
export TUMULT_OTEL_ENABLED=true
export OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:$GRPC_PORT

echo "[4] soak build 1.0.0 three times with oha through the CLI; the build says nothing during the runs"
await_silence checkout
soaks run-1.0.0
for i in 1 2 3; do
    [[ -z "$(recorded_version "run-1.0.0-$i")" ]] || fail "nothing spoke inside run $i, yet it recorded $(recorded_version "run-1.0.0-$i")"
done
await_json "the 1.0.0 runs' windows" /api/perf/checkout 'd["candidate"]["samples"]>=60'
curl -fs "$URL/api/perf/checkout" | py 'a=d["candidate"]["attribution"]; assert a=={"declared":0,"observed":0,"inferred":3}, a'
pass "three runs against 1.0.0 completed with no version of their own ($(windows_in run-1.0.0-1) windows each); the daemon reads it from the span before the runs (inferred)"

echo "[5] deploy build 1.1.0 (40 ms) to demo and soak it while 1.0.0 stays busy in prod; one run is no verdict, three are"
await_past run-1.0.0-3
deploy 40
emit_span checkout 1.1.0 demo
await_version checkout 1.1.0
await_silence checkout
# The old build keeps serving another environment: its spans fall inside
# the candidate's whole run, and it speaks last.
( while :; do emit_span checkout 1.0.0 prod; sleep 0.2; done ) >/dev/null 2>&1 &
BPID=$!
soak run-1.1.0-1
await_json "the first 1.1.0 run's windows" /api/perf/checkout 'd["candidate"]["version"]=="1.1.0" and d["candidate"]["samples"]>=20'
curl -fs "$URL/api/perf/checkout" >"$WORK/perf-one-run.json"
py 'assert d["verdict"]=="inconclusive" and d["reason"]=="insufficient_evidence", (d["verdict"],d["reason"],d["reasons"])
assert len(d["candidate"]["runs"])==1 and len(d["baseline"]["runs"])==3 and d["comparison"]["min_runs"]==3, (d["candidate"]["runs"],d["baseline"]["runs"],d["comparison"]["min_runs"])
print("     one run: %s" % d["reasons"][0])' <"$WORK/perf-one-run.json"
perf_exits 4 cli-one-run.out checkout
FROM=2 soaks run-1.1.0
kill "$BPID"; wait "$BPID" 2>/dev/null || true; BPID=""
emit_span checkout 1.0.0 prod
for i in 1 2 3; do
    [[ -z "$(recorded_version "run-1.1.0-$i")" ]] || fail "only prod spoke inside demo run $i, yet it recorded $(recorded_version "run-1.1.0-$i")"
done
await_json "the 1.1.0 runs' windows" /api/perf/checkout 'd["candidate"]["version"]=="1.1.0" and d["candidate"]["samples"]>=60'
pass "one run of 1.1.0 against three of 1.0.0 is inconclusive (insufficient_evidence) and tumult perf exits 4; three completed and reached the daemon; prod's 1.0.0 spoke through all of them and they recorded nothing"

echo "[6] the verdict: one environment, the version deployed latest against the one before it, 1.1.0 regressed"
curl -fs "$URL/api/perf/checkout" >"$WORK/perf.json"
py 'vs=[v["version"] for v in d["versions_seen"]]; assert vs==["1.1.0","1.0.0"], vs
assert d["environment"]=="demo" and d["environments_seen"]==["demo","prod"], (d["environment"],d["environments_seen"])
assert d["selection"]=="latest_deployment" and "1.1.0" in d["selection_rule"] and "1.0.0" in d["selection_rule"], (d["selection"],d["selection_rule"])
assert not any(v["redeployed"] for v in d["versions_seen"]), d["versions_seen"]
b,c=d["baseline"],d["candidate"]
assert b["version"]=="1.0.0" and c["version"]=="1.1.0", (b,c)
assert len(b["runs"])==3 and len(c["runs"])==3 and not set(b["runs"])&set(c["runs"]), (b["runs"],c["runs"])
assert b["attribution"]["inferred"]==3 and c["attribution"]["inferred"]==3, (b["attribution"],c["attribution"])
assert b["samples"]>=60 and c["samples"]>=60, (b["samples"],c["samples"])
assert b["partial"]["excluded"]==0 and c["partial"]["excluded"]==0 and d["unattributed_runs"]["count"]==0, (b["partial"],c["partial"],d["unattributed_runs"])
assert c["windows"]["planned"]==72 and c["windows"]["unanswered"]==0 and c["windows"]["censored"]==0, c["windows"]
for side in (b,c):
    r=side["requests"]; assert r["sent"]>0 and r["failed"]==0 and r["runs_without_counts"]==0, r
assert d["probe"]=="load.latency_p95_ms"
k=d["comparison"]; assert k["quantile"]=="p95" and k["ci_low"]>k["margin"]>0, k
assert c["p95"]>b["p95"]+20, (b["p95"],c["p95"])
assert d["verdict"]=="regressed" and d["reason"]=="latency", (d["verdict"],d["reason"],d["reasons"])
assert d["stored"] is False and d["history"]==[], (d["stored"],d["history"])
print("     baseline 1.0.0 p95 %.1f ms over %d windows; candidate 1.1.0 p95 %.1f ms over %d windows; delta %+.1f ms, CI [%.1f, %.1f], margin %.1f" % (b["p95"],b["samples"],c["p95"],c["samples"],k["delta"],k["ci_low"],k["ci_high"],k["margin"]))' <"$WORK/perf.json"
curl -fs "$URL/api/perf/checkout?env=prod" >"$WORK/perf-prod.json"
py 'assert [v["version"] for v in d["versions_seen"]]==["1.0.0"], d["versions_seen"]
assert d["environment"]=="prod", d["environment"]
assert d["candidate"]["version"]=="1.0.0" and d["baseline"] is None, (d["candidate"],d["baseline"])
assert d["candidate"]["runs"]==[] and d["candidate"]["samples"]==0, d["candidate"]
assert d["verdict"]=="inconclusive", d["verdict"]' <"$WORK/perf-prod.json"
pass "regressed with three runs each, all read from demo's own spans although prod's 1.0.0 filled the run; prod itself holds no demo run"

echo "[7] the old version speaks last in demo: the same verdict; a request that asks to store is refused"
await_past run-1.1.0-3
for _ in 1 2 3; do emit_span checkout 1.0.0 demo; done
await_json "1.0.0 to be the last heard in demo" "/api/perf/checkout" '[v["last_seen_ns"] for v in d["versions_seen"] if v["version"]=="1.0.0"][0] > [v["last_seen_ns"] for v in d["versions_seen"] if v["version"]=="1.1.0"][0]'
curl -fs "$URL/api/perf/checkout" >"$WORK/perf-again.json"
python3 -c "
import json
a=json.load(open('$WORK/perf.json')); b=json.load(open('$WORK/perf-again.json'))
last=lambda d,v: [s['last_seen_ns'] for s in d['versions_seen'] if s['version']==v][0]
assert last(b,'1.0.0')>last(a,'1.0.0'), 'the old version did not speak again'
same=lambda d: (d['verdict'], d['baseline']['version'], d['candidate']['version'], d['baseline']['runs'], d['candidate']['runs'], d['comparison'], [s['version'] for s in d['versions_seen']])
assert same(a)==same(b), (same(a), same(b))
assert b['stored'] is False and b['history']==[]"
code=$(status_of "$URL/api/perf/checkout?store=true")
[[ "$code" == 400 ]] || fail "GET ?store=true should be 400, got $code"
py 'assert "POST /api/perf/checkout/verdicts" in d["error"], d["error"]' <"$WORK/last.json"
code=$(status_of -X POST "$URL/api/perf/checkout/verdicts?store=true")
[[ "$code" == 400 ]] || fail "POST ?store=true should be 400, got $code"
curl -fs "$URL/api/perf/checkout" | py 'assert d["history"]==[], d["history"]'
pass "1.0.0 reported again after 1.1.0 and candidate, baseline, runs and interval are the same; ?store= is 400 on GET and POST and stored nothing"

echo "[8] tumult perf: exit codes, --json, named versions, and an idempotent --store"
perf_exits 3 cli.out checkout
grep -q 'perf checkout: REGRESSED' "$WORK/cli.out" || { cat "$WORK/cli.out"; fail "tumult perf did not print the verdict"; }
grep -q 'versions: 1.1.0 is the version deployed latest in environment demo' "$WORK/cli.out" || { cat "$WORK/cli.out"; fail "tumult perf did not say how the versions were chosen"; }
grep -q '3 run(s) attributed from spans' "$WORK/cli.out" || { cat "$WORK/cli.out"; fail "tumult perf did not say the runs were attributed from spans"; }
perf_exits 3 cli-regressed.out checkout --fail-on regressed
perf_exits 0 cli-never.out checkout --fail-on never
"$BIN/tumult" perf checkout --json --fail-on never >"$WORK/cli.json" 2>"$WORK/cli.err" || { cat "$WORK/cli.err"; fail "tumult perf --json --fail-on never errored"; }
py 'assert d["verdict"]=="regressed" and d["stored"] is False and len(d["versions_seen"])==2, d' <"$WORK/cli.json"
# A range and the partial runs, asked for on the command line.
"$BIN/tumult" perf checkout --json --fail-on never --range 1h --include partial >"$WORK/cli-range.json" 2>"$WORK/cli.err" || { cat "$WORK/cli.err"; fail "tumult perf --range --include partial errored"; }
py 'assert d["range"]=="1h" and d["include_partial"] is True and d["verdict"]=="regressed", (d["range"],d["include_partial"],d["verdict"])' <"$WORK/cli-range.json"
# The comparison turned around by naming the versions.
"$BIN/tumult" perf checkout --json --fail-on never --baseline 1.1.0 --candidate 1.0.0 >"$WORK/cli-named.json" 2>"$WORK/cli.err" || { cat "$WORK/cli.err"; fail "tumult perf with named versions errored"; }
py 'assert d["selection"]=="explicit" and d["baseline"]["version"]=="1.1.0" and d["candidate"]["version"]=="1.0.0", (d["selection"],d["baseline"]["version"],d["candidate"]["version"])
assert d["verdict"]=="improved", (d["verdict"],d["reasons"])' <"$WORK/cli-named.json"
code=$(status_of "$URL/api/perf/checkout?env=demo&candidate=9.9.9")
[[ "$code" == 400 ]] || fail "a version never seen should be 400, got $code"
py 'assert "version 9.9.9 of checkout was not seen in environment demo" in d["error"], d["error"]' <"$WORK/last.json"
daemon_alive
"$BIN/tumult" perf checkout --json --fail-on never --store --env demo >"$WORK/cli-store.json" 2>"$WORK/cli.err" || { cat "$WORK/cli.err"; fail "tumult perf --store errored"; }
py 'assert d["verdict"]=="regressed" and d["stored"] is True and d["duplicate"] is False, (d["verdict"],d["stored"],d["duplicate"])' <"$WORK/cli-store.json"
# The same request again, as a pipeline retries after a client timeout.
daemon_alive
"$BIN/tumult" perf checkout --json --fail-on never --store --env demo >"$WORK/cli-store-again.json" 2>"$WORK/cli.err" || { cat "$WORK/cli.err"; fail "a retried tumult perf --store errored"; }
py 'assert d["stored"] is True and d["duplicate"] is True, (d["stored"],d["duplicate"])' <"$WORK/cli-store-again.json"
curl -fs "$URL/api/perf/checkout" >"$WORK/perf-history.json"
py 'h=d["history"]; assert len(h)==1, h
assert h[0]["verdict"]=="regressed" and h[0]["baseline_version"]=="1.0.0" and h[0]["candidate_version"]=="1.1.0", h[0]
assert h[0]["envs"]==["demo"], h[0]["envs"]' <"$WORK/perf-history.json"
pass "exit 3 on regressed (0 with --fail-on never) with the selection rule and attribution printed; --range and --include partial reach the daemon; --baseline/--candidate turn it to improved (explicit); an unseen version is 400; --store appended one demo row and the retry none"

echo "[9] build 1.2.0 keeps the 40 ms but every fifth request takes 0.3 s; the run declares its version"
deploy 40 5 300
soaks run-1.2.0 --target-version 1.2.0
for i in 1 2 3; do
    [[ "$(recorded_version "run-1.2.0-$i")" == "1.2.0 declared" ]] || fail "the journal should record 1.2.0 as declared, got '$(recorded_version "run-1.2.0-$i")'"
done
grep -q 'Target version: 1.2.0 (declared)' "$WORK/run-1.2.0-1.out" || { cat "$WORK/run-1.2.0-1.out"; fail "tumult run did not print the declared version"; }
await_json "the 1.2.0 runs' windows" /api/perf/checkout 'd["candidate"]["version"]=="1.2.0" and d["candidate"]["samples"]>=60'
curl -fs "$URL/api/perf/checkout" >"$WORK/perf-tail.json"
py 'b,c=d["baseline"],d["candidate"]
assert [v["version"] for v in d["versions_seen"]]==["1.2.0","1.1.0","1.0.0"], d["versions_seen"]
assert b["version"]=="1.1.0" and c["version"]=="1.2.0", (b["version"],c["version"])
assert len(c["runs"])==3 and len(b["runs"])==3, (b["runs"],c["runs"])
assert c["attribution"]=={"declared":3,"observed":0,"inferred":0}, c["attribution"]
assert d["runs_on_other_versions"]==3, d["runs_on_other_versions"]
assert c["requests"]["failed"]==0 and c["windows"]["censored"]==0, (c["requests"],c["windows"])
assert d["verdict"]!="unchanged", "the slow tail was censored: %s" % d["reasons"]
assert d["verdict"]=="regressed", (d["verdict"],d["reasons"])
assert c["p95"]>250, c
print("     baseline 1.1.0 p95 %.1f ms; candidate 1.2.0 p95 %.1f ms over %d windows: %s" % (b["p95"],c["p95"],c["samples"],d["verdict"]))' <"$WORK/perf-tail.json"
[[ "$(load_field run-1.2.0-1 error_rate)" == 0 ]] || fail "the slow requests were answered, not errors: error_rate $(load_field run-1.2.0-1 error_rate)"
# Every window waits about 0.3 s for its requests in flight and the next
# one gives that time back, so the plan ends on its clock: all 24 windows,
# none skipped. Windows run back to back would have taken 31 s and more.
[[ "$(load_field run-1.2.0-1 planned_windows)" == 24 && "$(load_field run-1.2.0-1 completed_windows)" == 24 ]] ||
    fail "the slow tail cost windows: $(load_field run-1.2.0-1 completed_windows) of $(load_field run-1.2.0-1 planned_windows)"
took=$(load_field run-1.2.0-1 duration_s)
less_than "$took" 30 || fail "the 24 s plan drifted to $took s"
pass "no span carries 1.2.0, the three runs declared it and are the candidate; requests in flight at a window's end are measured (regressed, not unchanged); 24 of 24 windows in $took s"

echo "[10] two traced builds, 1.3.0 in demo and 2.0.0 in prod, soaked at the same time"
deploy_traced 1.3.0 20
PROOF_TELEMETRY_URL=$URL PROOF_SERVICE_VERSION=2.0.0 PROOF_SERVICE_ENV=prod \
    python3 "$S/target.py" "$PROD_PORT" 20 >"$WORK/service-prod.log" 2>&1 &
PRPID=$!
await "the prod build" 30 curl -fs "http://127.0.0.1:$PROD_PORT/ready"
curl -fs "$URL/api/perf/checkout" | py 'vs=[v["version"] for v in d["versions_seen"]]; assert "1.3.0" not in vs, vs'
( run_toon run-1.3.0-1 checkout-soak ) &
DEMO_RUN=$!
# The prod soak starts once the demo load is running and ends before it:
# each run's window holds spans of both builds.
await "the demo load" 60 oha_running "$SVC_PORT"
run_toon run-prod-2.0.0 checkout-prod || { cat "$WORK/run-prod-2.0.0.out"; fail "the prod soak did not complete"; }
oha_running "$SVC_PORT" || fail "the demo soak ended before the prod soak; the two windows did not overlap"
wait "$DEMO_RUN" || { cat "$WORK/run-1.3.0-1.out"; fail "the demo soak did not complete"; }
[[ "$(recorded_version run-prod-2.0.0)" == "2.0.0 observed" ]] || fail "the prod run should have observed 2.0.0, got '$(recorded_version run-prod-2.0.0)'"
FROM=2 soaks run-1.3.0
for i in 1 2 3; do
    [[ "$(recorded_version "run-1.3.0-$i")" == "1.3.0 observed" ]] || fail "demo run $i should have observed 1.3.0, got '$(recorded_version "run-1.3.0-$i")'"
done
await_json "the 1.3.0 runs' windows" "/api/perf/checkout?env=demo" 'd["candidate"]["version"]=="1.3.0" and d["candidate"]["samples"]>=60'
curl -fs "$URL/api/perf/checkout?env=demo" >"$WORK/perf-idle.json"
py 'b,c=d["baseline"],d["candidate"]
assert b["version"]=="1.2.0" and c["version"]=="1.3.0", (b["version"],c["version"])
assert len(c["runs"])==3 and len(b["runs"])==3, (b["runs"],c["runs"])
assert c["attribution"]=={"declared":0,"observed":3,"inferred":0}, c["attribution"]
assert "2.0.0" not in [v["version"] for v in d["versions_seen"]], d["versions_seen"]
assert d["verdict"]=="improved", (d["verdict"],d["reasons"])
print("     demo: 1.3.0 p95 %.1f ms over %d windows against 1.2.0 at %.1f ms: %s" % (c["p95"],c["samples"],b["p95"],d["verdict"]))' <"$WORK/perf-idle.json"
await_json "the prod run's windows" "/api/perf/checkout?env=prod" 'd["candidate"]["version"]=="2.0.0" and d["candidate"]["samples"]>=4'
curl -fs "$URL/api/perf/checkout?env=prod" >"$WORK/perf-prod-two.json"
py 'b,c=d["baseline"],d["candidate"]
assert [v["version"] for v in d["versions_seen"]]==["2.0.0","1.0.0"], d["versions_seen"]
assert c["version"]=="2.0.0" and len(c["runs"])==1 and c["attribution"]["observed"]==1, c
assert b["version"]=="1.0.0" and b["runs"]==[], b
assert d["verdict"]=="inconclusive", (d["verdict"],d["reasons"])' <"$WORK/perf-prod-two.json"
# Both environments can now answer; a request that names neither is asked to.
code=$(status_of "$URL/api/perf/checkout")
[[ "$code" == 400 ]] || fail "a request across two comparable environments should be 400, got $code"
py 'e=d["error"]; assert "more than one environment (demo, prod)" in e and "env=" in e, e' <"$WORK/last.json"
# No verdict at all is an error (1), not a verdict that failed the gate.
perf_exits 1 cli-ambiguous.out checkout --fail-on never
grep -q 'more than one environment' "$WORK/cli-ambiguous.out" || { cat "$WORK/cli-ambiguous.out"; fail "tumult perf should pass the daemon's question on"; }
kill "$PRPID"; wait "$PRPID" 2>/dev/null || true; PRPID=""
pass "each run observed the version of its own environment while both builds spoke in both windows of the first; demo improved over three runs, prod has its one run on 2.0.0; without env the answer is 400"

echo "[11] a plan the target is too slow to let finish is truncated: a deviation, and left out of the samples"
deploy 40 5 900
# A one-second window gives a request 750 ms by default; these answers are
# slow, not lost, so the run allows them the time they take.
TUMULT_OHA_REQUEST_TIMEOUT_S=2 run_expecting run-truncated checkout-truncated deviated --target-version 1.0.0
planned=$(load_field run-truncated planned_windows); done_=$(load_field run-truncated completed_windows)
[[ "$planned" == 12 ]] || fail "the plan has 12 windows, the journal says $planned"
[[ "$done_" -ge 1 && "$done_" -lt 12 ]] || fail "expected a truncated plan, got $done_ of $planned windows"
[[ "$(windows_in run-truncated)" == "$done_" ]] || fail "the journal keeps $(windows_in run-truncated) samples for $done_ completed windows"
[[ "$(load_field run-truncated error_rate)" == 0 ]] || fail "every request was answered; error_rate $(load_field run-truncated error_rate)"
[[ "$(load_field run-truncated thresholds_met)" == false ]] || fail "a truncated plan cannot meet its thresholds"
grep -q "reasons\[1\]: \"$(( planned - done_ )) of 12 windows skipped" "$WORK/run-truncated.toon" || { grep -n reasons "$WORK/run-truncated.toon"; fail "the journal does not say why the plan was truncated"; }
await_json "the truncated run in the lake" "/api/perf/checkout-truncated" 'd["candidate"]["partial"]["truncated"]==1'
curl -fs "$URL/api/perf/checkout-truncated" | py 'c=d["candidate"]
assert c["partial"]["truncated"]==1 and c["partial"]["deviated"]==1 and c["partial"]["excluded"]==1, c["partial"]
assert c["runs"]==[] and c["samples"]==0 and d["include_partial"] is False, (c["runs"],c["samples"])'
curl -fs "$URL/api/perf/checkout-truncated?include=partial" | py 'c=d["candidate"]
assert c["partial"]["excluded"]==0 and len(c["runs"])==1 and c["samples"]=='"$done_"' and d["include_partial"] is True, (c["partial"],c["runs"],c["samples"])'
pass "truncated: $done_ of $planned windows ran, no request failed, run deviated; the verdict counts the run as truncated and takes its $done_ samples only with include=partial"
deploy 2

echo "[12] a target that accepts and never answers: unmet threshold deviates, no threshold only records"
python3 "$S/target.py" "$HUNG_PORT" 0 1 0 >"$WORK/hung.log" 2>&1 &
HPID=$!
await "the hung service's port" 30 port_taken "$HUNG_PORT"
TUMULT_OHA_REQUEST_TIMEOUT_S=1 run_expecting run-hung checkout-hung deviated
[[ "$(load_field run-hung total_requests)" -gt 0 ]] || fail "the hung run should count its timed-out requests"
[[ "$(load_field run-hung error_rate)" == 1 ]] || fail "every hung request is an error, got $(load_field run-hung error_rate)"
[[ "$(load_field run-hung thresholds_met)" == false ]] || fail "an error rate of 1 cannot meet a threshold of 0.5"
# The same load with no thresholds declared, on the default request
# timeout: by the load's own clock the 2 s window ends in about twice its
# length, not after the 30 s a request may otherwise wait.
run_toon run-blackout checkout-blackout || { cat "$WORK/run-blackout.out"; fail "a blackout with no declared thresholds must not fail the run"; }
grep -q '^status: completed$' "$WORK/run-blackout.toon" || fail "the blackout run should end completed"
[[ "$(load_field run-blackout error_rate)" == 1 ]] || fail "the blackout must still be measured as an error rate of 1"
[[ "$(load_field run-blackout unanswered_windows)" == 1 ]] || fail "the journal should record the unanswered window"
took=$(load_field run-blackout duration_s)
less_than "$took" 10 || fail "a 2 s window against a hung target took $took s"
pass "all-hung target: with a threshold $(load_field run-hung total_requests) requests, error rate 1, deviated; with none completed, 1 unanswered window recorded, the load took $took s"

echo "[13] build 1.3.1 answers in 2 ms and hangs every third request: faster, and regressed on its failure share"
deploy 2 3 0
# A hung request is given up on after 300 ms here, so a window ends inside
# its slot and the plan is run whole; the default is tried below.
TUMULT_OHA_REQUEST_TIMEOUT_S=0.3 soaks run-1.3.1 --target-version 1.3.1
sent=$(load_field run-1.3.1-1 sent); timed_out=$(load_field run-1.3.1-1 timed_out)
[[ "${sent:-0}" -gt 0 && "${timed_out:-0}" -gt 0 ]] || fail "the journal should count the requests sent and given up on, got sent='$sent' timed_out='$timed_out'"
await_json "the 1.3.1 runs' windows" "/api/perf/checkout?env=demo" 'd["candidate"]["version"]=="1.3.1" and len(d["candidate"]["runs"])==3'
curl -fs "$URL/api/perf/checkout?env=demo" >"$WORK/perf-hanging.json"
py 'b,c=d["baseline"],d["candidate"]
assert b["version"]=="1.3.0" and c["version"]=="1.3.1", (b["version"],c["version"])
assert d["verdict"]=="regressed" and d["reason"]=="failure_share", (d["verdict"],d["reason"],d["reasons"])
r=c["requests"]; assert r["failed"]>0 and r["failure_share"]>0.15 and r["runs_without_counts"]==0, r
assert b["requests"]["failure_share"]<0.01, b["requests"]
f=d["comparison"]["failure_share"]; assert f["delta"]>f["margin"]>0, f
assert c["windows"]["unanswered"]==0 and c["windows"]["censored"]>0, c["windows"]
print("     candidate 1.3.1: %d of %d requests failed (%.0f%%), %d of %d windows censored; baseline 1.3.0: %d of %d" % (r["failed"],r["sent"],100*r["failure_share"],c["windows"]["censored"],c["windows"]["completed"],b["requests"]["failed"],b["requests"]["sent"]))' <"$WORK/perf-hanging.json"
# What it did answer, it answered faster: the median alone would have
# called it improved.
curl -fs "$URL/api/perf/checkout?env=demo&probe=load.latency_p50_ms&quantile=p50" >"$WORK/perf-hanging-p50.json"
py 'b,c=d["baseline"],d["candidate"]
assert c["p50"]<b["p50"], (b["p50"],c["p50"])
assert d["verdict"]=="regressed" and d["reason"]=="failure_share", (d["verdict"],d["reason"],d["reasons"])
print("     median of the answered: 1.3.1 %.1f ms against 1.3.0 %.1f ms, and still %s" % (c["p50"],b["p50"],d["verdict"]))' <"$WORK/perf-hanging-p50.json"
perf_exits 3 cli-hanging.out checkout --env demo
grep -q 'request(s) failed' "$WORK/cli-hanging.out" || { cat "$WORK/cli-hanging.out"; fail "tumult perf did not report the failed requests"; }
grep -q 'hold a request given up on at its timeout' "$WORK/cli-hanging.out" || { cat "$WORK/cli-hanging.out"; fail "tumult perf did not report the censored windows"; }
pass "a build that hangs a third of its requests and answers the rest faster is regressed (failure_share), its windows are marked censored, and tumult perf exits 3"
# On the default timeout a 1 s window waits 750 ms for a hung request,
# runs over its slot and costs the next one its place: the plan falls
# short and the run is left out of the latencies. The requests it failed
# are counted all the same.
run_expecting run-1.3.2 checkout-soak deviated --target-version 1.3.2
planned=$(load_field run-1.3.2 planned_windows); done_=$(load_field run-1.3.2 completed_windows)
[[ "$done_" -ge 1 && "$done_" -lt "$planned" ]] || fail "expected the hung requests to cut the plan short, got $done_ of $planned windows"
await_json "the 1.3.2 run" "/api/perf/checkout?env=demo" 'd["candidate"]["version"]=="1.3.2" and d["candidate"]["partial"]["excluded"]==1'
curl -fs "$URL/api/perf/checkout?env=demo&baseline=1.3.0" >"$WORK/perf-cut-short.json"
py 'b,c=d["baseline"],d["candidate"]
assert b["version"]=="1.3.0" and c["version"]=="1.3.2", (b["version"],c["version"])
assert c["runs"]==[] and c["samples"]==0 and c["partial"]["truncated"]==1, (c["runs"],c["samples"],c["partial"])
r=c["requests"]; assert r["sent"]>0 and r["failure_share"]>0.15, r
assert d["verdict"]=="regressed" and d["reason"]=="failure_share", (d["verdict"],d["reason"],d["reasons"])
print("     candidate 1.3.2: plan cut short, no run in the latencies, %d of %d requests failed (%.0f%%): %s" % (r["failed"],r["sent"],100*r["failure_share"],d["verdict"]))' <"$WORK/perf-cut-short.json"
perf_exits 3 cli-cut-short.out checkout --env demo --baseline 1.3.0
pass "the same build on the default timeout: $done_ of $planned windows ran, the run deviated and is left out of the latencies, and it is regressed on the requests it failed"
deploy 2

echo "[14] the 24 x 1 s soak against the hung build: no window is skipped, the run completes, and the build is regressed"
run_toon run-dark checkout-soak-blackout --target-version 1.4.0 || { cat "$WORK/run-dark.out"; fail "a blacked-out soak with no declared thresholds must not fail the run"; }
grep -q '^status: completed$' "$WORK/run-dark.toon" || fail "the blacked-out soak should end completed"
planned=$(load_field run-dark planned_windows); done_=$(load_field run-dark completed_windows)
lost=$(load_field run-dark lost_to_unanswered); dark=$(load_field run-dark unanswered_windows)
[[ "$planned" == 24 ]] || fail "the plan has 24 windows, the journal says $planned"
[[ $(( done_ + ${lost:-0} )) == 24 ]] || fail "the blackout truncated the plan: $done_ completed and ${lost:-0} lost of 24"
[[ "$dark" == 24 ]] || fail "all 24 windows were unanswered, the journal says $dark"
[[ "$(load_field run-dark error_rate)" == 1 ]] || fail "the blackout must be measured as an error rate of 1"
took=$(load_field run-dark duration_s)
less_than "$took" 30 || fail "the 24 s blacked-out plan drifted to $took s"
await_json "the blacked-out run in the lake" "/api/perf/checkout?env=demo" 'd["candidate"]["version"]=="1.4.0" and d["candidate"]["windows"]["unanswered"]==24'
curl -fs "$URL/api/perf/checkout?env=demo" >"$WORK/perf-dark.json"
py 'b,c=d["baseline"],d["candidate"]
assert b["version"]=="1.3.2" and c["version"]=="1.4.0", (b["version"],c["version"])
assert len(c["runs"])==1 and c["samples"]==0, (c["runs"],c["samples"])
assert c["partial"]=={"truncated":0,"aborted":0,"halted":0,"deviated":0,"excluded":0}, c["partial"]
assert c["windows"]["unanswered_share"]==1.0 and b["windows"]["unanswered"]==0, (c["windows"],b["windows"])
assert d["verdict"]=="regressed" and d["reason"]=="candidate_unanswered", (d["verdict"],d["reason"],d["reasons"])
assert "24 of 24 load windows went unanswered" in d["reasons"][0], d["reasons"]
print("     %s" % d["reasons"][0])' <"$WORK/perf-dark.json"
perf_exits 3 cli-dark.out checkout --env demo
grep -q 'perf checkout: REGRESSED' "$WORK/cli-dark.out" || { cat "$WORK/cli-dark.out"; fail "tumult perf did not call the blacked-out build regressed"; }
grep -q '24 of 24 load window(s) went unanswered' "$WORK/cli-dark.out" || { cat "$WORK/cli-dark.out"; fail "tumult perf did not report the unanswered windows"; }
# The dark build as the yardstick: nothing to measure against.
curl -fs "$URL/api/perf/checkout?env=demo&baseline=1.4.0&candidate=1.3.0" | py 'assert d["verdict"]=="inconclusive" and d["reason"]=="baseline_unanswered", (d["verdict"],d["reason"],d["reasons"])'
perf_exits 4 cli-dark-baseline.out checkout --env demo --baseline 1.4.0 --candidate 1.3.0
perf_exits 0 cli-dark-baseline-lenient.out checkout --env demo --baseline 1.4.0 --candidate 1.3.0 --fail-on regressed
curl -fs "$URL/api/perf/checkout?env=demo&candidate=1.3.0" | py 'assert d["selection"]=="explicit" and d["baseline"]["version"]=="1.2.0" and d["verdict"]=="improved", (d["selection"],d["baseline"]["version"],d["verdict"])'
pass "24 of 24 one-second windows unanswered in $took s, $done_ completed, plan not truncated, run completed; the verdict is regressed (candidate_unanswered) and tumult perf exits 3; as the baseline it gives inconclusive and exit 4"

echo "[15] a load that cannot send one request fails the run"
run_expecting run-unreachable checkout-unreachable failed
[[ "$(load_field run-unreachable total_requests)" == 0 ]] || fail "expected zero requests, got $(load_field run-unreachable total_requests)"
[[ "$(load_field run-unreachable thresholds_met)" == false ]] || fail "zero requests cannot meet thresholds"
pass "zero requests: thresholds unmet, run failed, exit non-zero"

echo "[16] SIGINT during a ten-minute soak: the load ends at once, the rollback runs"
export PROOF_ROLLBACK_MARK=$WORK/interrupt-rolled-back
daemon_alive
"$BIN/tumult" run "$WORK/toons/checkout-interrupt.toon" --journal-path "$WORK/run-interrupt.toon" --force >"$WORK/run-interrupt.out" 2>&1 &
TPID=$!
# Wait for the first window to finish: its oha is replaced by the next one.
await "the first oha window" 60 oha_running "$SVC_PORT"
first=$(oha_pids "$SVC_PORT" | head -1)
await "the second oha window" 60 oha_replaced "$SVC_PORT" "$first"
t0=$(now_ms)
kill -INT "$TPID"
rc=0; wait "$TPID" || rc=$?
took=$(( $(now_ms) - t0 ))
[[ $rc -ne 0 ]] || fail "an interrupted run must exit non-zero"
# The plan had most of 600 s left; the bound only has to tell "ended on
# the interrupt" from "waited for the load", on a busy machine too.
[[ $took -lt 120000 ]] || fail "the interrupt took ${took} ms; the load was waited for"
oha_running "$SVC_PORT" && fail "oha is still hitting the service after the interrupt"
grep -q '^status: interrupted$' "$WORK/run-interrupt.toon" || { cat "$WORK/run-interrupt.out"; fail "the run should end interrupted"; }
[[ -e "$PROOF_ROLLBACK_MARK" ]] || fail "the rollback did not run after the interrupt"
kept=$(windows_in run-interrupt)
[[ "${kept:-0}" -ge 1 ]] || fail "the windows completed before the interrupt were lost"
[[ "$(load_field run-interrupt aborted)" == true ]] || fail "the interrupted load is not marked aborted"
less_than "$(load_field run-interrupt duration_s)" 300 || fail "the interrupted load says it ran $(load_field run-interrupt duration_s) s"
pass "interrupted in ${took} ms of a 600 s plan: no oha left, rollback ran, $kept window(s) journaled, load marked aborted after $(load_field run-interrupt duration_s) s"

# A guard halt: $1 journal, $2 experiment. The load must end at once, keep
# its evidence with the aborted marker, and end before the rollback starts.
halted_run() {
    export PROOF_ROLLBACK_MARK=$WORK/$1-rolled-back PROOF_BREACH_FLAG=$WORK/$1-breach-flag
    local t0 ended
    t0=$(now_ms)
    run_expecting "$1" "$2" halted
    took=$(( $(now_ms) - t0 ))
    [[ $took -lt 300000 ]] || fail "the halted run took ${took} ms; the load was waited for"
    oha_running "$SVC_PORT" && fail "oha is still hitting the service after the halt"
    [[ -e "$PROOF_ROLLBACK_MARK" ]] || fail "the rollback did not run after the halt"
    [[ "$(load_field "$1" aborted)" == true ]] || fail "the halted load is not marked aborted"
    [[ "$(load_field "$1" total_requests)" -gt 0 ]] || fail "the halted load kept no evidence"
    ended=$(load_field "$1" ended_at_ns)
    [[ "$ended" -le "$(cat "$PROOF_ROLLBACK_MARK")" ]] || fail "the rollback ran before the load had ended"
}

echo "[17] a guard breach during a ten-minute soak halts it the same way"
halted_run run-halt checkout-halt
kept=$(windows_in run-halt)
[[ "${kept:-0}" -ge 1 ]] || fail "the windows completed before the halt were lost"
pass "halted: the whole run took ${took} ms of a 600 s plan, no oha left, $kept window(s) journaled, load aborted before the rollback"

echo "[18] the same halt on a flat load: one 600 s window, cut short, keeps what it measured"
halted_run run-halt-flat checkout-halt-flat
[[ "$(load_field run-halt-flat planned_windows)" == 1 && "$(load_field run-halt-flat completed_windows)" == 0 ]] ||
    fail "a flat load cut short completed no window: $(load_field run-halt-flat completed_windows) of $(load_field run-halt-flat planned_windows)"
[[ "$(windows_in run-halt-flat)" == 1 ]] || fail "the partial window was not journaled"
[[ "$(load_field run-halt-flat thresholds_met)" == false ]] || fail "an aborted load cannot meet its thresholds"
pass "halted flat load: ${took} ms of a 600 s window, $(load_field run-halt-flat total_requests) requests kept from the partial window, marked aborted, ended before the rollback"

echo "[19] environments: exact match, three attributes, and an honest 404"
code=$(status_of "$URL/api/perf/checkout?env=staging")
[[ "$code" == 404 ]] || fail "checkout has no staging telemetry; expected 404, got $code"
py 'e=d["error"]; assert "in environment staging" in e and "deployment.environment.name" in e, e' <"$WORK/last.json"
code=$(status_of "$URL/api/perf/checkout?env=")
[[ "$code" == 400 ]] || fail "an empty env should be 400, got $code"
emit_span ledger 3.0.0 -
await_json "service.version 3.0.0 of ledger" /api/perf/ledger '"3.0.0" in [v["version"] for v in d["versions_seen"]]'
curl -fs "$URL/api/perf/ledger" | py 'assert d["environment"] is None and d["environments_seen"]==[None], (d["environment"],d["environments_seen"])'
code=$(status_of "$URL/api/perf/ledger?env=demo")
[[ "$code" == 404 ]] || fail "an untagged service is in no environment; expected 404, got $code"
py 'e=d["error"]; assert "target ledger in environment demo" in e and "resilience.target.environment" in e, e' <"$WORK/last.json"
emit_span billing 7.0.0 demo deployment.environment.name
emit_span billing 7.1.0 demo deployment.environment
await_json "billing's two versions in demo" "/api/perf/billing?env=demo" 'len(d["versions_seen"])==2'
pass "env=staging and an untagged service are 404 with the reason; env= is 400; untagged telemetry answers as environment null; both deployment.environment spellings count"

echo "[20] a rollback: the version deployed again is the candidate, whatever the range asked for"
DAY=86400
emit_span inventory 1.0.0 demo resilience.target.environment $(( 40 * DAY ))
emit_span inventory 2.0.0 demo resilience.target.environment $(( 20 * DAY ))
emit_span inventory 1.0.0 demo resilience.target.environment $(( 5 * DAY ))
# One request of the old version is a straggler, not a rollback: the pair
# has no order the daemon will vouch for.
unorderable() {
    curl -s -o "$WORK/perf-straggler.json" -w '%{http_code}' "$URL/api/perf/inventory?env=demo" >"$WORK/perf-straggler.code"
    [[ "$(<"$WORK/perf-straggler.code")" == 400 ]] && grep -q '"reason":"unorderable"' "$WORK/perf-straggler.json"
}
await "one returned request of inventory 1.0.0 to be refused as unorderable" 60 unorderable
grep -q '1.0.0' "$WORK/perf-straggler.json" && grep -q '2.0.0' "$WORK/perf-straggler.json" || fail "the refusal does not name both versions: $(<"$WORK/perf-straggler.json")"
for i in $(seq 1 11); do
    emit_span inventory 1.0.0 demo resilience.target.environment $(( 5 * DAY - i ))
done
await_json "inventory's rollback" "/api/perf/inventory?env=demo" 'len(d["versions_seen"])==2 and d["versions_seen"][0]["redeployed"]'
for range in 30d 60d; do
    curl -fs "$URL/api/perf/inventory?env=demo&range=$range" >"$WORK/perf-rollback-$range.json"
    py 'vs=d["versions_seen"]
assert [(v["version"],v["redeployed"]) for v in vs]==[("1.0.0",True),("2.0.0",False)], vs
assert vs[0]["last_deployed_ns"]>vs[1]["last_deployed_ns"]>vs[0]["first_seen_ns"], vs
assert d["candidate"]["version"]=="1.0.0" and d["baseline"]["version"]=="2.0.0", (d["candidate"]["version"],d["baseline"]["version"])
assert d["selection"]=="latest_deployment" and "1.0.0" in d["selection_rule"], (d["selection"],d["selection_rule"])' <"$WORK/perf-rollback-$range.json"
done
pass "1.0.0 at 40 days, 2.0.0 at 20, 1.0.0 again at 5: one returned request is 400 unorderable naming both versions; with twelve, 1.0.0 is flagged redeployed and is the candidate against 2.0.0 for range=30d and range=60d alike"

echo "[21] restart the daemon with authentication: roles and scopes on the same lake"
kill "$DPID"; wait "$DPID" 2>/dev/null || true
await "the daemon's port to be released" 60 port_free "$HTTP_PORT"
ADMIN=kro_$(python3 -c 'import secrets; print(secrets.token_hex(24))')
export KRONIKA_BOOTSTRAP_ADMIN_PASSWORD=proof-admin-password KRONIKA_BOOTSTRAP_TOKEN=$ADMIN
start_daemon
admin() { curl -fs -H "Authorization: Bearer $ADMIN" -H "$CT" "$@"; }
# Create a user with a role and one environment scope, take it through the
# forced password change, and mint it a token (printed).
mint() {
    local id
    daemon_alive
    id=$(admin -X POST -d "{\"username\":\"$1\",\"password\":\"$1-first-password\",\"role\":\"$2\",\"env_scopes\":[\"$3\"]}" "$URL/api/users" | py 'print(d["id"])')
    curl -fs -c "$WORK/$1.jar" -X POST -H "$CT" -d "{\"username\":\"$1\",\"password\":\"$1-first-password\"}" "$URL/api/auth/login" >/dev/null
    curl -fs -b "$WORK/$1.jar" -X POST -H "$CT" -d "{\"current_password\":\"$1-first-password\",\"new_password\":\"$1-second-password\"}" "$URL/api/auth/change-password" >/dev/null
    admin -X POST -d "{\"name\":\"$1-proof\",\"user_id\":\"$id\"}" "$URL/api/tokens" | py 'print(d["token"])'
}
VIEWER=$(mint proof-viewer viewer demo)
OPERATOR=$(mint proof-operator operator demo)
PROD=$(mint proof-prod viewer prod)
OUTSIDER=$(mint proof-outsider viewer staging)
as() { local token=$1; shift; status_of -H "Authorization: Bearer $token" "$@"; }
code=$(status_of "$URL/api/perf/checkout")
[[ "$code" == 401 ]] || fail "an anonymous read should be 401 once users exist, got $code"
# A demo-scoped principal is answered for demo without naming it, although
# prod could answer too.
code=$(as "$VIEWER" "$URL/api/perf/checkout")
[[ "$code" == 200 ]] || fail "the demo viewer should read the verdict, got $code"
py 'assert d["environment"]=="demo" and d["environments_seen"]==["demo"], (d["environment"],d["environments_seen"])
assert d["verdict"]=="regressed" and d["candidate"]["version"]=="1.4.0", (d["verdict"],d["candidate"]["version"])
assert len(d["history"])==1 and d["history"][0]["envs"]==["demo"], d["history"]' <"$WORK/last.json"
code=$(as "$VIEWER" "$URL/api/perf/checkout?candidate=1.3.0")
[[ "$code" == 200 ]] || fail "the demo viewer should read the verdict for 1.3.0, got $code"
py 'assert d["verdict"]=="improved" and d["candidate"]["version"]=="1.3.0" and d["selection"]=="explicit", (d["verdict"],d["candidate"]["version"],d["selection"])' <"$WORK/last.json"
code=$(as "$VIEWER" "$URL/api/perf/checkout?store=true")
[[ "$code" == 400 ]] || fail "a viewer's GET ?store=true should be 400, got $code"
code=$(as "$VIEWER" -X POST "$URL/api/perf/checkout/verdicts?candidate=1.3.0")
[[ "$code" == 403 ]] || fail "a viewer storing a verdict should be 403, got $code"
rc=0; TUMULT_DAEMON_TOKEN=$VIEWER "$BIN/tumult" perf checkout --candidate 1.3.0 --store --fail-on never >"$WORK/cli-viewer.out" 2>&1 || rc=$?
[[ $rc -eq 1 ]] || { cat "$WORK/cli-viewer.out"; fail "tumult perf --store must exit 1 when nothing was stored, exited $rc"; }
grep -q '403' "$WORK/cli-viewer.out" || { cat "$WORK/cli-viewer.out"; fail "tumult perf --store should report the 403"; }
as "$VIEWER" "$URL/api/perf/checkout" >/dev/null
py 'assert len(d["history"])==1, d["history"]' <"$WORK/last.json"
daemon_alive
code=$(as "$OPERATOR" -X POST "$URL/api/perf/checkout/verdicts?env=prod")
[[ "$code" == 403 ]] || fail "an operator outside prod should be 403 there, got $code"
daemon_alive
code=$(as "$OPERATOR" -X POST "$URL/api/perf/checkout/verdicts?candidate=1.3.0")
[[ "$code" == 201 ]] || fail "the demo operator should store the verdict, got $code"
py 'assert d["stored"] is True and d["duplicate"] is False and d["candidate"]["version"]=="1.3.0", (d["stored"],d["duplicate"],d["candidate"])' <"$WORK/last.json"
# Two retries of the request that already succeeded: by hand and by CLI.
daemon_alive
code=$(as "$OPERATOR" -X POST "$URL/api/perf/checkout/verdicts?candidate=1.3.0")
[[ "$code" == 200 ]] || fail "a retried store should be 200, got $code"
py 'assert d["stored"] is True and d["duplicate"] is True, (d["stored"],d["duplicate"])' <"$WORK/last.json"
rc=0; TUMULT_DAEMON_TOKEN=$OPERATOR "$BIN/tumult" perf checkout --candidate 1.3.0 --store --fail-on never >"$WORK/cli-operator.out" 2>&1 || rc=$?
[[ $rc -eq 0 ]] || { cat "$WORK/cli-operator.out"; fail "tumult perf --store as operator should succeed"; }
grep -q 'verdict stored' "$WORK/cli-operator.out" || { cat "$WORK/cli-operator.out"; fail "tumult perf --store should say it stored"; }
grep -q 'nothing appended' "$WORK/cli-operator.out" || { cat "$WORK/cli-operator.out"; fail "tumult perf --store should say the verdict was already there"; }
grep -q 'versions: the request named the pair in environment demo' "$WORK/cli-operator.out" || { cat "$WORK/cli-operator.out"; fail "tumult perf should say the versions were named"; }
as "$VIEWER" "$URL/api/perf/checkout" >/dev/null
py 'h=d["history"]; assert len(h)==2 and all(r["envs"]==["demo"] for r in h), h
assert h[0]["candidate_version"]=="1.3.0" and h[1]["candidate_version"]=="1.1.0", h' <"$WORK/last.json"
code=$(as "$PROD" "$URL/api/perf/checkout")
[[ "$code" == 200 ]] || fail "the prod viewer should read prod's own telemetry, got $code"
py 'assert d["environment"]=="prod" and [v["version"] for v in d["versions_seen"]]==["2.0.0","1.0.0"], (d["environment"],d["versions_seen"])
assert d["baseline"]["runs"]==[] and len(d["candidate"]["runs"])==1 and d["history"]==[], (d["baseline"],d["candidate"],d["history"])' <"$WORK/last.json"
code=$(as "$OUTSIDER" "$URL/api/perf/checkout")
[[ "$code" == 404 ]] || fail "a staging-scoped viewer must not see the target, got $code"
py 'e=d["error"]; assert "in environment staging" in e, e' <"$WORK/last.json"
code=$(as "$OUTSIDER" "$URL/api/perf/checkout?env=demo")
[[ "$code" == 403 ]] || fail "a staging-scoped viewer asking for demo should be 403, got $code"
pass "401 anonymous; a demo viewer is answered for demo alone, 400 on GET ?store, 403 storing; operator stores in demo only and two retries append nothing (2 rows, none identical); prod sees only its own run and versions; staging sees nothing"
echo "ALL PERFORMANCE-VERDICT PROOF CHECKS PASSED (run $RUN)"
