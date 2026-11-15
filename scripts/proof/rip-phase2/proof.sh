#!/usr/bin/env bash
# Posture-and-risks live proof: real experiments run through the CLI into a
# fresh daemon; the passive risk scan, the posture score, the suppression
# chain and the /posture page answer from the lake. No mocks. Exit non-zero
# on any failed expectation. Run it three times in a row before a release.
#
#   cargo build -p tumultd -p tumult-cli && (cd web && npm ci && npm run build)
#   bash scripts/proof/rip-phase2/proof.sh [run-number]
#
# Ports 44317/44318 must be free. Work files land under $PROOF_WORK
# (default: a temp dir).
set -euo pipefail
S="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$S/../../.." && pwd)"
BIN=${TUMULT_BIN_DIR:-$REPO/target/debug}
RUN=${1:-1}
WORK=${PROOF_WORK:-$(mktemp -d)}/run-$RUN; rm -rf "$WORK"; mkdir -p "$WORK"
export TUMULT_LAKE_PATH=$WORK/lake.duckdb
export KRONIKA_OTLP_GRPC_ADDR=127.0.0.1:44317
export KRONIKA_OTLP_HTTP_ADDR=127.0.0.1:44318
export KRONIKA_METRICS_DIR=$REPO/metrics
export KRONIKA_MANIFESTS_DIR=$S/manifests
URL=http://127.0.0.1:44318

pass() { echo "  ok   $1"; }
fail() { echo "  FAIL $1"; exit 1; }
py() { python3 -c "import json,sys; d=json.load(sys.stdin); $1"; }

echo "[1] start tumultd on a fresh lake with a manifests dir"
RUST_LOG=warn "$BIN/tumultd" serve >"$WORK/tumultd.log" 2>&1 &
DPID=$!
trap 'kill $DPID 2>/dev/null || true' EXIT
for i in $(seq 1 60); do curl -fs $URL/healthz >/dev/null 2>&1 && break; sleep 0.5; done
curl -fs $URL/healthz >/dev/null || { cat "$WORK/tumultd.log"; fail "daemon did not come up"; }
pass "daemon healthy"

echo "[2] scan before any run: only manifest findings"
curl -fs "$URL/api/risks" >"$WORK/risks0.json"
py 'rules={r["rule"] for r in d["risks"]}; assert "single_replica" in rules, rules; assert "no_resource_limits" in rules, rules; assert "no_pod_disruption_budget" in rules, rules; assert "no_readiness_probe" in rules, rules; assert not any(r["subject"]=="demo/proof-db" for r in d["risks"]), "proof-db is well configured"; assert d["coverage"]["targets"]==[]' <"$WORK/risks0.json"
pass "manifest rules flag proof-service (1 replica, no limits, no PDB, no readiness) and not proof-db"

export TUMULT_DAEMON_URL=$URL
export TUMULT_OTEL_ENABLED=true
export OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:44317

echo "[3] run the DR experiment (Completed) and the blocking-breach variant (Deviated)"
"$BIN/tumult" run "$S/dr-objectives.toon" --journal-path "$WORK/good.toon" --force >"$WORK/good.out" 2>&1 || { cat "$WORK/good.out"; fail "good run errored"; }
grep -q 'status: completed' "$WORK/good.toon" || fail "good run not completed"
"$BIN/tumult" run "$S/dr-breach.toon" --journal-path "$WORK/breach.toon" --force >"$WORK/breach.out" 2>&1 || true
grep -q 'status: deviated' "$WORK/breach.toon" || fail "breach run should be deviated"
pass "two real runs journaled to the daemon"
sleep 3

echo "[4] scan after the runs: evidence-driven findings for proof-service"
curl -fs "$URL/api/risks" >"$WORK/risks1.json"
py 'rs=[r for r in d["risks"] if r["target"]=="proof-service"]; rules={r["rule"] for r in rs}
assert "objective_breached" in rules, rules
assert "latest_run_failed" not in rules, "a breached objective explains the deviation; no double count"
assert "discipline_gap" in rules, rules
ranks=[r["rank"] for r in d["risks"]]; assert ranks==sorted(ranks, reverse=True)
t=[c for c in d["coverage"]["targets"] if c["target"]=="proof-service"][0]
cells={c["kind"]:c["status"] for c in t["cells"]}; assert cells["dr"]=="covered", cells; assert cells["load"]=="missing", cells
assert d["summary"]["total"]==len(d["risks"]) and d["summary"]["suppressed"]==0' <"$WORK/risks1.json"
pass "breached objective (explains the deviation) and discipline gaps ranked; dr covered, load missing"

echo "[5] posture score"
curl -fs "$URL/api/posture" >"$WORK/posture.json"
py 'p=d["posture"]; assert 0<=p["score"]<=100 and p["band"] in ("good","fair","poor"), p
t=[x for x in p["targets"] if x["target"]=="proof-service"][0]
assert t["components"]["nfr"] is not None and t["components"]["coverage"]>0, t
assert 1<=len(d["top_risks"])<=5 and d["top_risks"][0]["id"]==max(d["top_risks"], key=lambda r:r["rank"])["id"]
nf=[n for n in d["nfrs"] if n["target"]=="proof-service"]; assert nf, "no objective cells"
assert {n["state"] for n in nf} >= {"met"}, nf; assert all(n["nfr_id"] and n["experiment"] for n in nf)
print("     posture", round(p["score"],1), p["band"], "components", t["components"], "objective cells", len(nf))' <"$WORK/posture.json"
pass "posture bounded with per-target components, objective cells and the top findings"

echo "[6] suppress the top finding, verify it moves aside and the chain links"
RID=$(py 'print(d["risks"][0]["id"])' <"$WORK/risks1.json")
TOTAL=$(py 'print(d["summary"]["total"])' <"$WORK/risks1.json")
curl -fs -X POST -H 'content-type: application/json' -d '{"reason":"proof: accepted for a day","expires_in_days":1,"actor":"proof"}' "$URL/api/risks/$RID/suppress" >"$WORK/sup1.json"
py 'assert d["prev_hash"] is None and d["new_hash"] and d["expires_at_ns"]>d["at_ns"]' <"$WORK/sup1.json"
curl -fs -X POST -H 'content-type: application/json' -d '{"reason":"proof: extended","actor":"proof"}' "$URL/api/risks/$RID/suppress" >"$WORK/sup2.json"
python3 -c "import json; a=json.load(open('$WORK/sup1.json')); b=json.load(open('$WORK/sup2.json')); assert b['prev_hash']==a['new_hash'], (a,b)"
curl -fs "$URL/api/risks" >"$WORK/risks2.json"
py "assert d['summary']['total']==$TOTAL-1 and d['summary']['suppressed']==1
assert all(r['id']!='$RID' for r in d['risks']); s=d['suppressed'][0]; assert s['id']=='$RID' and s['suppression']['reason']=='proof: extended', s" <"$WORK/risks2.json"
code=$(curl -s -o /dev/null -w '%{http_code}' -X POST -H 'content-type: application/json' -d '{"reason":"x","actor":"p"}' "$URL/api/risks/r_nope/suppress")
[[ "$code" == 404 ]] || fail "unknown id should be 404, got $code"
pass "suppression chained (prev_hash = first new_hash), finding moved aside, unknown id 404"

echo "[7] the chain survives a daemon restart (it lives in the lake, not in memory)"
kill $DPID; wait $DPID 2>/dev/null || true
RUST_LOG=warn "$BIN/tumultd" serve >>"$WORK/tumultd.log" 2>&1 &
DPID=$!
for i in $(seq 1 60); do curl -fs $URL/healthz >/dev/null 2>&1 && break; sleep 0.5; done
curl -fs "$URL/api/risks" >"$WORK/risks3.json"
py "assert d['summary']['suppressed']==1 and d['suppressed'][0]['id']=='$RID' and d['suppressed'][0]['suppression']['reason']=='proof: extended'" <"$WORK/risks3.json"
curl -fs -X POST -H 'content-type: application/json' -d '{"reason":"proof: third","actor":"proof"}' "$URL/api/risks/$RID/suppress" >"$WORK/sup3.json"
python3 -c "import json; b=json.load(open('$WORK/sup2.json')); c=json.load(open('$WORK/sup3.json')); assert c['prev_hash']==b['new_hash'], (b,c)"
pass "suppression persisted across restart and the third row chains onto the second"

echo "[8] the posture page is served"
code=$(curl -s -o "$WORK/posture.html" -w '%{http_code}' "$URL/posture")
[[ "$code" == 200 ]] || fail "/posture returned $code"
grep -q '_app/immutable' "$WORK/posture.html" || fail "/posture is not the SPA shell"
cmp -s "$WORK/posture.html" "$REPO/web/build/200.html" || fail "/posture is not the SPA shell the daemon embeds"
curl -fs "$URL/posture.html" >"$WORK/posture-static.html"
cmp -s "$WORK/posture-static.html" "$REPO/web/build/posture.html" || fail "/posture.html differs from the built page"
# The route chunk is the one the page names and the bare shell does not; its
# number moves whenever a page is added, so it is found, not written here.
chunks() { grep -o '_app/immutable/nodes/[0-9]*\.[A-Za-z0-9_-]*\.js' "$1" | sort -u; }
chunk=$(comm -23 <(chunks "$WORK/posture-static.html") <(chunks "$REPO/web/build/200.html") | head -n 1)
[[ -n "$chunk" ]] || fail "built posture page lacks its route chunk"
curl -fs -o /dev/null "$URL/$chunk" || fail "the daemon does not serve the posture route chunk $chunk"
pass "/posture served by the daemon"
echo "ALL POSTURE-AND-RISKS PROOF CHECKS PASSED (run $RUN)"
