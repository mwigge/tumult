#!/usr/bin/env bash
# Star Map live proof: the eight demo experiments run through the CLI into a
# fresh daemon that loads the demo org.yaml and workload manifests; the map
# at /api/starmap answers from the lake. No mocks. Exit non-zero on any
# failed expectation. Run it three times in a row before a release.
#
#   cargo build -p tumultd -p tumult-cli && (cd web && npm ci && npm run build)
#   bash scripts/proof/starmap-step1/proof.sh [run-number]
#
# Ports 44317/44318 must be free. Work files land under $PROOF_WORK
# (default: a temp dir); the experiments' scratch files are redirected there
# too, so nothing is written outside it.
set -euo pipefail
S="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$S/../../.." && pwd)"
BIN=${TUMULT_BIN_DIR:-$REPO/target/debug}
RUN=${1:-1}
WORK=${PROOF_WORK:-$(mktemp -d)}/run-$RUN; rm -rf "$WORK"; mkdir -p "$WORK/experiments"
export TUMULT_LAKE_PATH=$WORK/lake.duckdb
export KRONIKA_OTLP_GRPC_ADDR=127.0.0.1:44317
export KRONIKA_OTLP_HTTP_ADDR=127.0.0.1:44318
export KRONIKA_METRICS_DIR=$REPO/metrics
export KRONIKA_MANIFESTS_DIR=${KRONIKA_MANIFESTS_DIR:-$REPO/demo/kronika/manifests}
export KRONIKA_ORG_FILE=${KRONIKA_ORG_FILE:-$REPO/demo/kronika/org.yaml}
URL=http://127.0.0.1:44318
MAP="$URL/api/starmap?env=demo"

pass() { echo "  ok   $1"; }
fail() { echo "  FAIL $1"; exit 1; }
py() { python3 -c "import json,sys; d=json.load(sys.stdin); $1"; }

echo "[1] start tumultd on a fresh lake with the demo org and manifests"
RUST_LOG=warn "$BIN/tumultd" serve >"$WORK/tumultd.log" 2>&1 &
DPID=$!
trap 'kill $DPID 2>/dev/null || true' EXIT
for i in $(seq 1 60); do curl -fs $URL/healthz >/dev/null 2>&1 && break; sleep 0.5; done
curl -fs $URL/healthz >/dev/null || { cat "$WORK/tumultd.log"; fail "daemon did not come up"; }
pass "daemon healthy"

echo "[2] the map before any run: organisation, entities and workloads, no evidence"
curl -fs "$MAP" >"$WORK/map0.json"
py 'n={x["id"]:x for x in d["nodes"]}
assert n["org:"]["kind"]=="org", "no organisation root"
teams=sorted(x["label"] for x in d["nodes"] if x["kind"]=="org" and x["org_kind"]=="team")
assert teams==["data-team","edge-team","runtime-team"], teams
wl=[x for x in d["nodes"] if x["kind"]=="workload"]; assert len(wl)==5, [x["id"] for x in wl]
assert all(x["evidence"]["runs"]==0 for x in d["nodes"]), "a fresh lake has no runs"
assert not [x for x in d["nodes"] if x["kind"]=="def"], "no definitions before a run"
assert d["sources"]["slo"]==d["sources"]["dora"]==d["sources"]["maturity"]=="unavailable", d["sources"]' <"$WORK/map0.json"
pass "three teams under the organisation, five demo workloads, nothing tested yet"

export TUMULT_DAEMON_URL=$URL
export TUMULT_OTEL_ENABLED=true
export OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:44317

echo "[3] run the eight demo experiments through the CLI"
COUNT=0
for f in "$REPO"/demo/kronika/experiments/*.toon; do
  n=$(basename "$f" .toon)
  sed "s#/tmp/kd-demo#$WORK/kd-demo#g" "$f" >"$WORK/experiments/$n.toon"
  "$BIN/tumult" run "$WORK/experiments/$n.toon" --journal-path "$WORK/journal-$n.toon" --force >"$WORK/$n.out" 2>&1 || true
  [[ -s "$WORK/journal-$n.toon" ]] || { cat "$WORK/$n.out"; fail "$n left no journal"; }
  COUNT=$((COUNT + 1))
done
[[ "$COUNT" == 8 ]] || fail "expected 8 demo experiments, ran $COUNT"
grep -q 'status: completed' "$WORK/journal-api-worker-freeze.toon" || fail "api-worker-freeze should complete"
grep -q 'status: completed' "$WORK/journal-queue-dependency-loss.toon" && fail "queue-dependency-loss is designed to fail"
pass "eight real runs journaled to the daemon; queue-dependency-loss did not pass"
sleep 3

echo "[4] the map after the runs"
curl -fs "$MAP" >"$WORK/map1.json"
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
assert all(not x["id"].startswith("tgt:") for x in d["nodes"]), "retired tgt: ids"
assert all(not (x["from"].startswith("tgt:") or x["to"].startswith("tgt:")) for x in e)
assert all(x["from"] in n and x["to"] in n for x in e), "an edge points at a missing node"
assert all(x["env"] in (None,"demo") for x in d["nodes"]), "a node from another environment"
assert "sys:production/cdn" not in n and "wl:edge/deployment/cdn-edge" not in n
assert d["counts"]["nodes"]==len(d["nodes"]) and d["counts"]["edges"]==len(e)
print("     nodes", {k:len(v) for k,v in sorted(kinds.items())}, "edges", len(e))' <"$WORK/map1.json"
pass "organisation, 3 teams, >=4 workloads with runs_on edges, 8 definitions referenced, no tgt: ids, demo only"

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

echo "[6] gap and failed evidence"
py 'n={x["id"]:x for x in d["nodes"]}
g=n["sys:demo/session-cache"]; assert g["gap"]=="missing" and g["evidence"]["runs"]==0, g
assert n["ent:cif-0005"]["gap"]=="missing", n["ent:cif-0005"]
assert d["counts"]["gaps"]>=1 and d["counts"]["gaps"]==sum(1 for x in d["nodes"] if x["gap"])
q=n["sys:demo/message-queue"]; assert q["evidence"]["failed"]>=1 and q["evidence"]["runs"]>=1, q["evidence"]
assert q["gap"] is None, "a tested system is not a gap"
a=n["sys:demo/api-worker"]; assert a["evidence"]["runs"]>=1 and a["evidence"]["failed"]==0, a["evidence"]
assert n["ent:cif-0004"]["evidence"]["failed"]>=1, "the failure rolls up to the entity"
print("     gaps", sorted(x["id"] for x in d["nodes"] if x["gap"]))
print("     message-queue evidence", q["evidence"])' <"$WORK/map1.json"
pass "session-cache is a gap node; message-queue carries the failed run"

echo "[7] suppress one finding on api-worker with an expiry; the map keeps it visible"
curl -fs "$URL/api/risks?env=demo" >"$WORK/risks.json"
RID=$(py 'r=[x for x in d["risks"] if x["target"]=="api-worker" and x["rule"]=="single_replica"]; assert r, [(x["target"],x["rule"]) for x in d["risks"]]; print(r[0]["id"])' <"$WORK/risks.json")
OPEN=$(py 'print(d["counts"]["risks_open"])' <"$WORK/map1.json")
[[ "$(py 'print(d["counts"]["risks_suppressed"])' <"$WORK/map1.json")" == 0 ]] || fail "nothing is suppressed yet"
curl -fs -X POST -H 'content-type: application/json' -d '{"reason":"proof: second replica lands next sprint","expires_in_days":7,"actor":"proof"}' "$URL/api/risks/$RID/suppress" >"$WORK/sup.json"
curl -fs "$MAP" >"$WORK/map2.json"
py "c=d['counts']; assert c['risks_suppressed']==1 and c['risks_open']==$OPEN-1, c
s=[x for x in d['nodes'] if x['kind']!='entity' and x['risks']['suppressed']>0]; assert len(s)==1, [x['id'] for x in s]
r=s[0]['risks']; assert r['suppressed_by']=='proof' and r['suppression_reason']=='proof: second replica lands next sprint', r
days=(r['suppression_expires_ns']-d['as_of_ns'])/86400e9; assert 6.9<days<=7.0, days
print('     suppressed on', s[0]['id'], 'expires in', round(days,2), 'days; open', c['risks_open'], 'suppressed', c['risks_suppressed'])" <"$WORK/map2.json"
pass "the suppressed finding is counted apart, with actor, reason and expiry on its node"

echo "[8] focus, range and parameter checks"
curl -fs "$MAP&focus=sys:demo/message-queue&depth=1" >"$WORK/focus.json"
py 'ids={x["id"] for x in d["nodes"]}; assert "sys:demo/message-queue" in ids and "sys:demo/api-worker" not in ids, ids
assert d["focus"]=="sys:demo/message-queue" and d["depth"]==1 and d["counts"]["pruned"]>0, d["counts"]' <"$WORK/focus.json"
curl -fs "$URL/api/starmap?env=demo&range=30d&include=runs" >"$WORK/runs.json"
py 'r=[x for x in d["nodes"] if x["kind"]=="run"]; assert len(r)==8, len(r); assert d["range"]=="30d"
assert any(x["rel"]=="broke" for x in d["edges"]), "the failed run broke nothing"' <"$WORK/runs.json"
for bad in "range=1y" "depth=9&focus=org:" "budget=0" "kinds=planet" "include=all"; do
  code=$(curl -s -o /dev/null -w '%{http_code}' "$URL/api/starmap?$bad")
  [[ "$code" == 400 ]] || fail "$bad should be 400, got $code"
done
curl -fs "$MAP" >"$WORK/map3.json"
python3 -c "
import json
a=json.load(open('$WORK/map2.json')); b=json.load(open('$WORK/map3.json'))
# The read time and the posture score (evidence freshness decays by the
# second) are the only things allowed to move between two reads.
for m in (a,b):
    m.pop('as_of_ns')
    for n in m['nodes']:
        if n['posture']: n['posture']['score']=round(n['posture']['score'])
assert a==b, 'two reads of the same lake differ'"
pass "focus prunes to the neighbourhood, include=runs adds the eight runs, bad parameters are 400, reads are stable"

echo "[9] the compatibility view and the page"
curl -fs "$URL/api/topology" >"$WORK/topology.json"
py 'ids=[x["id"] for x in d["nodes"]]; assert ids and all(not i.startswith("tgt:") for i in ids), ids
assert "sys:demo/message-queue" in ids, ids
assert all(not x["to_id"].startswith("tgt:") for x in d["edges"])' <"$WORK/topology.json"
code=$(curl -s -o "$WORK/starmap.html" -w '%{http_code}' "$URL/starmap")
[[ "$code" == 200 ]] || fail "/starmap returned $code"
grep -q '_app/immutable' "$WORK/starmap.html" || fail "/starmap is not the SPA shell"
cmp -s "$WORK/starmap.html" "$REPO/web/build/200.html" || fail "/starmap is not the SPA shell the daemon embeds"
curl -fs "$URL/starmap.html" >"$WORK/starmap-static.html"
cmp -s "$WORK/starmap-static.html" "$REPO/web/build/starmap.html" || fail "/starmap.html differs from the built page"
pass "/api/topology has no tgt: ids; /starmap served by the daemon"
echo "ALL STAR MAP STEP 1 PROOF CHECKS PASSED (run $RUN)"
