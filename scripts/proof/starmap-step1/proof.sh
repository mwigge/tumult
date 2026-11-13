#!/usr/bin/env bash
# Star Map live proof: the eight demo experiments run through the CLI into a
# fresh daemon that loads the demo org.yaml and workload manifests; the map
# at /api/starmap answers from the lake. No mocks. Exit non-zero on any
# failed expectation. Run it three times in a row before a release.
#
#   cargo build -p tumultd -p tumult-cli && (cd web && npm ci && npm run build)
#   bash scripts/proof/starmap-step1/proof.sh [run-number]
#
# The daemon listens on two loopback ports that are free when the proof
# starts. Free is not reserved: another process can take a port before the
# daemon binds it, and would then answer in its place. So the proof asks the
# kernel (`ss -ltnp`) which process listens on each port and goes on only
# when that is the daemon it started, once at the start and again before
# every write step; it cannot attest another daemon. Work files (lake,
# journals, the experiments' scratch files) land under $PROOF_WORK/run-N
# (default: a temp dir) and are removed on exit, as is the daemon;
# PROOF_KEEP=1 keeps the files of a run for a post-mortem.
set -euo pipefail
S="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$S/../../.." && pwd)"
BIN=${TUMULT_BIN_DIR:-$REPO/target/debug}
RUN=${1:-1}
TEMP_ROOT=""
[[ -n "${PROOF_WORK:-}" ]] || TEMP_ROOT=$(mktemp -d)
WORK=${PROOF_WORK:-$TEMP_ROOT}/run-$RUN
DPID=""

pass() { echo "  ok   $1"; }
fail() { echo "  FAIL $1"; exit 1; }
# py <python> [argument...]: run <python> with the JSON on stdin as `d` and
# the arguments as `a`. Paths and numbers of this run are passed as
# arguments, never pasted into the source.
py() {
  local code=$1
  shift
  python3 -c "import json,sys; a=sys.argv[1:]; d=json.load(sys.stdin); $code" "$@"
}

# Nothing in here may end the script: under `set -e` a failing command in
# the EXIT trap would replace the exit code of the proof.
cleanup() {
  if [[ -n "$DPID" ]]; then
    kill "$DPID" 2>/dev/null || true # already gone is fine here
    wait "$DPID" 2>/dev/null || true
  fi
  if [[ "${PROOF_KEEP:-0}" == 1 ]]; then
    echo "  kept $WORK"
    return 0
  fi
  rm -rf "$WORK" || echo "  could not remove $WORK"
  [[ -n "$TEMP_ROOT" ]] || return 0
  rmdir "$TEMP_ROOT" 2>/dev/null || echo "  left $TEMP_ROOT: it is not empty"
}
trap cleanup EXIT

# The pid of the process listening on a loopback TCP port, from the kernel.
listener_pid() {
  ss -ltnpH "sport = :$1" | sed -n 's/.*pid=\([0-9]*\),.*/\1/p' | head -1
}

# Whether both ports are served by the daemon this proof started.
ours() {
  [[ "$(listener_pid "$HTTP_PORT")" == "$DPID" && "$(listener_pid "$GRPC_PORT")" == "$DPID" ]]
}

# The daemon this proof started must still be running, and it must be the
# process that listens on both ports: what answers otherwise is not ours.
alive() {
  if ! kill -0 "$DPID" 2>/dev/null; then
    tail -20 "$WORK/tumultd.log" || true
    fail "the daemon this proof started (pid $DPID) is gone"
  fi
  ours && return 0
  ss -ltnp "sport = :$HTTP_PORT or sport = :$GRPC_PORT" || true
  fail "ports $HTTP_PORT and $GRPC_PORT are not both served by the daemon this proof started (pid $DPID)"
}

# Retry a check until it holds: wait_for <what> <command...>.
wait_for() {
  local what=$1 deadline=$((SECONDS + 30))
  shift
  until "$@"; do
    kill -0 "$DPID" 2>/dev/null || alive
    ((SECONDS < deadline)) || fail "timed out waiting for $what"
    sleep 0.5
  done
}

# A read past the 30 s cache of the map. The daemon honours it at most
# every 2 s per map, so the answer is at most that old.
FLOOR_S=2.2
fresh() { curl -fsS -H 'Cache-Control: no-cache' "$@"; }

command -v ss >/dev/null || fail "ss (iproute2) is needed to identify the listening process"
rm -rf "$WORK"
mkdir -p "$WORK/experiments"
read -r GRPC_PORT HTTP_PORT < <(python3 -c 'import socket
held = [socket.socket() for _ in range(2)]
for s in held: s.bind(("127.0.0.1", 0))
print(*[s.getsockname()[1] for s in held])')
[[ -n "$GRPC_PORT" && -n "$HTTP_PORT" ]] || fail "no free loopback ports"
export TUMULT_LAKE_PATH=$WORK/lake.duckdb
export KRONIKA_OTLP_GRPC_ADDR=127.0.0.1:$GRPC_PORT
export KRONIKA_OTLP_HTTP_ADDR=127.0.0.1:$HTTP_PORT
export KRONIKA_METRICS_DIR=$REPO/metrics
export KRONIKA_MANIFESTS_DIR=${KRONIKA_MANIFESTS_DIR:-$REPO/demo/kronika/manifests}
export KRONIKA_ORG_FILE=${KRONIKA_ORG_FILE:-$REPO/demo/kronika/org.yaml}
URL=http://127.0.0.1:$HTTP_PORT
MAP="$URL/api/starmap?env=demo"

echo "[1] start tumultd on a fresh lake with the demo org and manifests"
RUST_LOG=warn "$BIN/tumultd" serve >"$WORK/tumultd.log" 2>&1 &
DPID=$!
healthy() { ours && curl -fs "$URL/healthz" >/dev/null 2>&1; }
wait_for "the daemon to listen on both ports and answer /healthz" healthy
alive
[[ -s "$TUMULT_LAKE_PATH" ]] || fail "the daemon did not open the private lake $TUMULT_LAKE_PATH"
pass "daemon healthy: pid $DPID is the listener on OTLP gRPC :$GRPC_PORT and HTTP :$HTTP_PORT, private lake"

echo "[2] the map before any run: organisation, entities and workloads, no evidence"
fresh "$MAP" >"$WORK/map0.json"
py 'n={x["id"]:x for x in d["nodes"]}
assert n["org:"]["kind"]=="org", "no organisation root"
teams=sorted(x["label"] for x in d["nodes"] if x["kind"]=="org" and x["org_kind"]=="team")
assert teams==["data-team","edge-team","runtime-team"], teams
wl=[x for x in d["nodes"] if x["kind"]=="workload"]; assert len(wl)==5, [x["id"] for x in wl]
assert all(x["evidence"]["runs"]==0 for x in d["nodes"]), "a fresh lake has no runs"
assert not [x for x in d["nodes"] if x["kind"]=="def"], "no definitions before a run"
assert d["sources"]["slo"]==d["sources"]["dora"]==d["sources"]["maturity"]=="unavailable", d["sources"]' <"$WORK/map0.json"
if python3 "$REPO/scripts/starmap-check.py" "$URL" --wait 0 >"$WORK/check0.out" 2>&1; then
  cat "$WORK/check0.out"
  fail "the demo-check assertions passed on a map with no runs"
fi
grep -q 'FAIL star map: 0 definitions' "$WORK/check0.out" || { cat "$WORK/check0.out"; fail "the demo-check assertions failed for another reason"; }
pass "three teams under the organisation, five demo workloads, nothing tested yet; the demo-check assertions reject it"

export TUMULT_DAEMON_URL=$URL
export TUMULT_OTEL_ENABLED=true
export OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:$GRPC_PORT

echo "[3] run the eight demo experiments through the CLI"
# What each experiment must do: the exit code of `tumult run` and the
# status in its journal. Two do not pass by design.
declare -A WANT=(
  [api-worker-freeze]="0 completed"
  [batch-worker-pool]="0 completed"
  [config-corruption]="1 deviated"
  [disk-pressure]="0 completed"
  [host-cpu-burn]="0 completed"
  [queue-dependency-loss]="1 failed"
  [service-partition]="0 completed"
  [worker-process-kill]="0 completed"
)
COUNT=0
for f in "$REPO"/demo/kronika/experiments/*.toon; do
  n=$(basename "$f" .toon)
  [[ -n "${WANT[$n]:-}" ]] || fail "$n has no expected outcome in this proof"
  python3 -c 'import sys
source, target, old, new = sys.argv[1:]
open(target, "w").write(open(source).read().replace(old, new))' \
    "$f" "$WORK/experiments/$n.toon" /tmp/kd-demo "$WORK/kd-demo"
  alive
  code=0
  "$BIN/tumult" run "$WORK/experiments/$n.toon" --journal-path "$WORK/journal-$n.toon" --force >"$WORK/$n.out" 2>&1 || code=$?
  [[ -s "$WORK/journal-$n.toon" ]] || { cat "$WORK/$n.out"; fail "$n left no journal (exit $code)"; }
  status=$(sed -n 's/^status: //p' "$WORK/journal-$n.toon" | head -1)
  echo "     $n: exit $code, $status"
  [[ "$code $status" == "${WANT[$n]}" ]] || { tail -20 "$WORK/$n.out"; fail "$n: expected ${WANT[$n]}, got $code $status"; }
  COUNT=$((COUNT + 1))
done
[[ "$COUNT" == "${#WANT[@]}" ]] || fail "expected ${#WANT[@]} demo experiments, ran $COUNT"
[[ ! -e "$WORK/kd-demo/worker.pid" ]] || fail "worker-process-kill left its pid file behind"
pass "eight real runs journaled to the daemon, each with its expected exit code and status"

evidence_landed() {
  fresh "$URL/api/starmap?env=demo&range=30d&include=runs" >"$WORK/runs.json" || return 1
  python3 -c 'import json,sys
d=json.load(open(sys.argv[1]))
tested={e[k] for e in d["edges"] if e["rel"]=="tested_by" for k in ("from","to")}
runs=[x for x in d["nodes"] if x["kind"]=="run"]
defs=[x for x in d["nodes"] if x["kind"]=="def" and x["id"] in tested]
broke=[e for e in d["edges"] if e["rel"]=="broke"]
sys.exit(0 if len(runs)==8 and len(defs)==8 and len(broke)>=2 else 1)' "$WORK/runs.json"
}
wait_for "the telemetry of the eight runs to land" evidence_landed

echo "[4] the map after the runs"
fresh "$MAP" >"$WORK/map1.json"
py 'n={x["id"]:x for x in d["nodes"]}; e=d["edges"]
kinds={}
for x in d["nodes"]: kinds.setdefault(x["kind"],[]).append(x["id"])
assert d["env"]=="demo" and d["range"]=="7d", (d["env"], d["range"])
assert n["org:"]["kind"]=="org"
teams=[x for x in d["nodes"] if x["kind"]=="org" and x["org_kind"]=="team"]; assert len(teams)==3, teams
hosted={x["to"] for x in e if x["rel"]=="runs_on"}
wl=[w for w in kinds["workload"] if w in hosted]; assert len(wl)>=4, (kinds["workload"], hosted)
tested={x["to"] for x in e if x["rel"]=="tested_by"}|{x["from"] for x in e if x["rel"]=="tested_by"}
defs=[x for x in kinds.get("def",[]) if x in tested]; assert len(defs)==8, kinds.get("def")
assert all(x.startswith("def:name/") for x in defs), "a definition known by name only is def:name/<name>"
assert all(not x["id"].startswith("tgt:") for x in d["nodes"]), "retired tgt: ids"
assert all(not (x["from"].startswith("tgt:") or x["to"].startswith("tgt:")) for x in e)
assert all(x["from"] in n and x["to"] in n for x in e), "an edge points at a missing node"
assert all(x["env"] in (None,"demo") for x in d["nodes"]), "a node from another environment"
assert "sys:production/cdn" not in n and "wl:edge/deployment/cdn-edge" not in n
assert d["counts"]["nodes"]==len(d["nodes"]) and d["counts"]["edges"]==len(e)
assert all(x["flags"]==[] for x in d["nodes"]), [(x["id"],x["flags"]) for x in d["nodes"] if x["flags"]]
assert d["focus_found"] is None, "no focus was asked for"
assert set(d["sources"].values())<={"ok","unavailable"}, d["sources"]
print("     nodes", {k:len(v) for k,v in sorted(kinds.items())}, "edges", len(e))' <"$WORK/map1.json"
pass "organisation, 3 teams, >=4 workloads with runs_on edges, 8 definitions referenced, no tgt: ids, no flags, demo only"

echo "[5] ownership: entities under their teams, the unassigned one under (unassigned)"
py 'n={x["id"]:x for x in d["nodes"]}
want={"ent:cif-0001":"runtime-team","ent:cif-0003":"runtime-team","ent:cif-0002":"data-team","ent:cif-0004":"data-team"}
for ent,team in want.items():
    p=n[n[ent]["parent"]]; assert p["kind"]=="org" and p["label"]==team, (ent, p["id"])
assert n["ent:cif-0005"]["parent"]=="org:(unassigned)", n["ent:cif-0005"]["parent"]
assert n["sys:demo/message-queue"]["parent"]=="ent:cif-0004", n["sys:demo/message-queue"]["parent"]
owns={(x["from"],x["to"]) for x in d["edges"] if x["rel"]=="owns"}
assert (n["ent:cif-0004"]["parent"],"ent:cif-0004") in owns, "no owns edge for cif-0004"' <"$WORK/map1.json"
pass "cif-0001..0004 owned by runtime-team and data-team; cif-0005 unassigned"

echo "[6] gaps, failed evidence and what rolls up"
py 'n={x["id"]:x for x in d["nodes"]}; c=d["counts"]
g=n["sys:demo/session-cache"]; assert g["gap"]=="missing" and g["evidence"]["runs"]==0, g
assert n["ent:cif-0005"]["gap"]=="missing", n["ent:cif-0005"]
assert c["gaps"]>=1 and c["gaps"]==sum(1 for x in d["nodes"] if x["gap"])
q=n["sys:demo/message-queue"]; assert q["evidence"]["failed"]>=1 and q["evidence"]["runs"]>=1, q["evidence"]
assert q["gap"] is None, "a tested system is not a gap"
a=n["sys:demo/api-worker"]; assert a["evidence"]["runs"]>=1 and a["evidence"]["failed"]==0, a["evidence"]
assert n["ent:cif-0004"]["evidence"]["failed"]>=1, "the failure rolls up to the entity"
assert all((x["rollup"] is not None)==(x["kind"] in ("entity","org")) for x in d["nodes"]), "roll-ups sit on entities and org units only"
r=n["ent:cif-0005"]["rollup"]; assert (r["systems"],r["untested_systems"],r["gaps"])==(1,1,1), r
r=n["ent:cif-0004"]["rollup"]; assert (r["systems"],r["untested_systems"],r["failed"])==(1,0,1), r
root=n["org:"]["rollup"]
assert root["systems"]==len([x for x in d["nodes"] if x["kind"]=="system"]), root
assert root["untested_systems"]==1 and root["gaps"]==1 and root["failed"]==2, root
assert c["risks_unplaced"]==0 and root["risks_open"]==c["risks_open"], (root, c)
assert n["org:platform/data/data-team"]["rollup"]["failed"]==2, "both failed runs sit under data-team"
print("     gaps", sorted(x["id"] for x in d["nodes"] if x["gap"]))
print("     message-queue evidence", q["evidence"])
print("     organisation roll-up", root)' <"$WORK/map1.json"
fresh "$MAP&budget=3" >"$WORK/small.json"
python3 -c 'import json, sys
full, small = (json.load(open(path))["counts"] for path in sys.argv[1:])
assert small["clustered"]>0 and small["nodes"]<full["nodes"], small
for k in ("gaps","risks_open","risks_suppressed","risks_unplaced"): assert small[k]==full[k], (k, small, full)
print("     budget 3 clusters", small["clustered"], "nodes; gaps stay at", small["gaps"])' \
  "$WORK/map1.json" "$WORK/small.json"
pass "session-cache is a gap node; message-queue carries the failed run; counts roll up to the organisation and do not move with the budget"

echo "[6b] the demo-check assertions without --write only read"
alive
python3 "$REPO/scripts/starmap-check.py" "$URL" --wait 30 >"$WORK/check-read.out" 2>&1 || { cat "$WORK/check-read.out"; fail "the read-only assertions failed"; }
[[ "$(grep -c '^ok   star map: ' "$WORK/check-read.out")" == 7 ]] || { cat "$WORK/check-read.out"; fail "expected seven passing lines"; }
grep -q '^skip star map: suppressed risk not checked' "$WORK/check-read.out" || { cat "$WORK/check-read.out"; fail "the skipped suppression check is not reported"; }
sleep "$FLOOR_S"
fresh "$MAP" | py 'assert d["counts"]["risks_suppressed"]==0, d["counts"]'
pass "seven assertions hold, the suppression check is reported as skipped, and nothing was suppressed"

echo "[7] suppress one finding on api-worker with an expiry; the map keeps it visible"
curl -fsS "$URL/api/risks?env=demo" >"$WORK/risks.json"
RID=$(py 'r=[x for x in d["risks"] if x["target"]=="api-worker" and x["rule"]=="single_replica"]; assert r, [(x["target"],x["rule"]) for x in d["risks"]]; print(r[0]["id"])' <"$WORK/risks.json")
OPEN=$(py 'print(d["counts"]["risks_open"])' <"$WORK/map1.json")
[[ "$(py 'print(d["counts"]["risks_suppressed"])' <"$WORK/map1.json")" == 0 ]] || fail "nothing is suppressed yet"
curl -fsS "$MAP" >"$WORK/cached.json" # fills the cache with the map before the write
alive
curl -fsS -X POST -H 'content-type: application/json' -d '{"reason":"proof: second replica lands next sprint","expires_in_days":7,"actor":"proof"}' "$URL/api/risks/$RID/suppress" >"$WORK/sup.json"
# No `Cache-Control: no-cache` here: the write itself must drop the cached map.
curl -fsS "$MAP" >"$WORK/map2.json"
py 'was_open=int(a[0]); c=d["counts"]; assert c["risks_suppressed"]==1 and c["risks_open"]==was_open-1, c
s=[x for x in d["nodes"] if x["kind"] not in ("entity","org") and x["risks"]["suppressed"]>0]; assert len(s)==1, [x["id"] for x in s]
r=s[0]["risks"]; assert r["suppressed_by"]=="proof" and r["suppression_reason"]=="proof: second replica lands next sprint", r
days=(r["suppression_expires_ns"]-d["as_of_ns"])/86400e9; assert 6.9<days<=7.0, days
root=[x for x in d["nodes"] if x["id"]=="org:"][0]["rollup"]; assert root["risks_suppressed"]==1 and root["risks_open"]==was_open-1, root
print("     suppressed on", s[0]["id"], "expires in", round(days,2), "days; open", c["risks_open"], "suppressed", c["risks_suppressed"])' "$OPEN" <"$WORK/map2.json"
pass "the suppressed finding is counted apart, with actor, reason and expiry on its node, on a plain read right after the write"

echo "[8] focus, range and parameter checks"
fresh "$MAP&focus=sys:demo/message-queue&depth=1" >"$WORK/focus.json"
py 'ids={x["id"] for x in d["nodes"]}; assert "sys:demo/message-queue" in ids and "sys:demo/api-worker" not in ids, ids
assert d["focus"]=="sys:demo/message-queue" and d["focus_found"] is True and d["depth"]==1 and d["counts"]["pruned"]>0, d["counts"]' <"$WORK/focus.json"
fresh "$MAP&focus=sys:demo/no-such-system" >"$WORK/nofocus.json"
py 'assert d["focus_found"] is False and d["nodes"]==[], (d["focus_found"], len(d["nodes"]))' <"$WORK/nofocus.json"
py 'r=[x for x in d["nodes"] if x["kind"]=="run"]; assert len(r)==8, len(r); assert d["range"]=="30d"
assert len([x for x in d["edges"] if x["rel"]=="broke"])>=2, "the two runs that did not pass broke nothing"' <"$WORK/runs.json"
for bad in "range=1y" "depth=9&focus=org:" "budget=0" "kinds=planet" "include=all"; do
  code=$(curl -s -o /dev/null -w '%{http_code}' "$URL/api/starmap?$bad")
  [[ "$code" == 400 ]] || fail "$bad should be 400, got $code"
done
# A forced read is honoured once the kept map is 2 s old; inside those 2 s
# it is answered from the kept map, so no client can make the daemon read
# the lake in a loop. A machine that stalls between the two reads gets
# another try.
forced_twice() {
  sleep "$FLOOR_S"
  fresh "$MAP" >"$WORK/map3.json" || return 1
  fresh "$MAP" >"$WORK/map3b.json" || return 1
  cmp -s "$WORK/map3.json" "$WORK/map3b.json"
}
wait_for "two forced reads inside 2 s to be one read of the lake" forced_twice
sleep "$FLOOR_S"
fresh "$MAP" >"$WORK/map4.json"
python3 -c 'import json, sys
a, b = (json.load(open(path)) for path in sys.argv[1:])
assert b["as_of_ns"]>a["as_of_ns"], "a no-cache read after the floor must read the lake again"
# The read time and the posture score (evidence freshness decays by the
# second) are the only things allowed to move between two reads.
for m in (a,b):
    m.pop("as_of_ns")
    for n in m["nodes"]:
        if n["posture"]: n["posture"]["score"]=round(n["posture"]["score"])
assert a==b, "two reads of the same lake differ"' "$WORK/map3.json" "$WORK/map4.json"
pass "focus prunes to the neighbourhood, an unknown focus says so, include=runs adds the eight runs, bad parameters are 400, a forced read is rate-limited to one per 2 s, reads are stable"

echo "[9] the 30 s cache and its validator"
curl -fsS -D "$WORK/h1" "$MAP" >"$WORK/c1.json"
curl -fsS -D "$WORK/h2" "$MAP" >"$WORK/c2.json"
cmp -s "$WORK/c1.json" "$WORK/c2.json" || fail "two plain reads inside 30 s must be the same cached body"
ETAG=$(tr -d '\r' <"$WORK/h1" | sed -n 's/^etag: //ip')
[[ "$ETAG" =~ ^\"[0-9a-f]{32}\"$ ]] || fail "no quoted ETag on the map: '$ETAG'"
[[ "$ETAG" == "$(tr -d '\r' <"$WORK/h2" | sed -n 's/^etag: //ip')" ]] || fail "the cached read changed its ETag"
tr -d '\r' <"$WORK/h1" | grep -qi '^cache-control: private, no-cache$' || fail "the map must be private and revalidated"
code=$(curl -s -o "$WORK/c304" -w '%{http_code}' -H "If-None-Match: $ETAG" "$MAP")
[[ "$code" == 304 && ! -s "$WORK/c304" ]] || fail "If-None-Match with the current ETag should be an empty 304, got $code"
code=$(curl -s -o /dev/null -w '%{http_code}' -H 'If-None-Match: "0000"' "$MAP")
[[ "$code" == 200 ]] || fail "a stale validator should get the map, got $code"
pass "plain reads share one cached body and ETag $ETAG; the validator answers 304"

echo "[10] the compatibility view and the page"
curl -fsS "$URL/api/topology" >"$WORK/topology.json"
py 'ids=[x["id"] for x in d["nodes"]]; assert ids and all(not i.startswith("tgt:") for i in ids), ids
assert "sys:demo/message-queue" in ids, ids
assert any(i.startswith("sys:-/") for i in ids), "a target known by name only maps to a sys:-/ node"
assert all(not x["to_id"].startswith("tgt:") for x in d["edges"])' <"$WORK/topology.json"
code=$(curl -s -o /dev/null -w '%{http_code}' "$URL/api/topology?range=1y")
[[ "$code" == 400 ]] || fail "/api/topology?range=1y should be 400, got $code"
code=$(curl -s -o "$WORK/starmap.html" -w '%{http_code}' "$URL/starmap")
[[ "$code" == 200 ]] || fail "/starmap returned $code"
grep -q '_app/immutable' "$WORK/starmap.html" || fail "/starmap is not the SPA shell"
cmp -s "$WORK/starmap.html" "$REPO/web/build/200.html" || fail "/starmap is not the SPA shell the daemon embeds"
curl -fsS "$URL/starmap.html" >"$WORK/starmap-static.html"
cmp -s "$WORK/starmap-static.html" "$REPO/web/build/starmap.html" || fail "/starmap.html differs from the built page"
pass "/api/topology has no tgt: ids, keeps name-only targets and rejects an unknown range; /starmap served by the daemon"

echo "[11] the demo-check assertions against this daemon"
alive
# With STARMAP_URL demo-check only reads; the risk step [7] suppressed is
# what its eighth line finds.
STARMAP_URL=$URL bash "$REPO/scripts/demo-check.sh" --starmap-only >"$WORK/check1.out" 2>&1 || { cat "$WORK/check1.out"; fail "the demo-check Star Map assertions failed"; }
[[ "$(grep -c 'PASS.*star map: ' "$WORK/check1.out")" == 8 ]] || { cat "$WORK/check1.out"; fail "expected eight passing Star Map lines"; }
# This daemon is thrown away, so the check may write: with a risk suppressed
# already it has nothing to add.
python3 "$REPO/scripts/starmap-check.py" "$URL" --wait 30 --write >"$WORK/check-write.out" 2>&1 || { cat "$WORK/check-write.out"; fail "the assertions with --write failed"; }
[[ "$(grep -c '^ok   star map: ' "$WORK/check-write.out")" == 8 ]] || { cat "$WORK/check-write.out"; fail "expected eight passing lines with --write"; }
sleep "$FLOOR_S"
fresh "$MAP" | py 'assert d["counts"]["risks_suppressed"]==1, d["counts"]'
alive
pass "scripts/demo-check.sh --starmap-only reads and eight assertions hold; --write adds no second suppression"
echo "ALL STAR MAP STEP 1 PROOF CHECKS PASSED (run $RUN)"
