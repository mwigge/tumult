#!/usr/bin/env bash
# DORA coverage and maturity live proof: a fresh daemon with a function
# register, telemetry posted over OTLP the way services export it, and
# backdated test runs. The registry profile, the register codes, the
# unknown state of a declared feed, the 180-day operating rule, the cap by an
# uninstrumented critical service and its lifting are all read back from
# the API. The worked examples of the model (level 4 at index 4.42, level 2
# at index 3.60) are fixture inputs, so they are asserted by running their
# tests. No mocks. Exit non-zero on any failed expectation. Run it three
# times in a row before a release.
#
#   (cd web && npm ci && npm run build) && cargo build -p tumultd
#   bash scripts/proof/dora-score/proof.sh [run-number]
#
# Ports 48417/48418 must be free. Work files land under $PROOF_WORK
# (default: a temp dir). PROOF_CARGO names the cargo command for the
# worked-example tests (default: cargo).
set -euo pipefail
S="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$S/../../.." && pwd)"
BIN=${TUMULT_BIN_DIR:-$REPO/target/debug}
RUN=${1:-1}
WORK=${PROOF_WORK:-$(mktemp -d)}/run-$RUN; rm -rf "$WORK"; mkdir -p "$WORK"
export TUMULT_LAKE_PATH=$WORK/lake.duckdb
export KRONIKA_OTLP_GRPC_ADDR=127.0.0.1:48417
export KRONIKA_OTLP_HTTP_ADDR=127.0.0.1:48418
export KRONIKA_METRICS_DIR=$REPO/metrics
export KRONIKA_MANIFESTS_DIR=$S/manifests
export KRONIKA_ORG_FILE=$WORK/org.yaml
URL=http://127.0.0.1:48418
ENV=proof

pass() { echo "  ok   $1"; }
fail() { echo "  FAIL $1"; exit 1; }
py() { python3 -c "import json,sys; d=json.load(sys.stdin); $1"; }
post() { curl -fsS --data-binary @- -H 'content-type: application/x-protobuf' "$URL/v1/traces" >/dev/null; }

DPID=
start_daemon() {
    RUST_LOG=warn "$BIN/tumultd" serve >>"$WORK/tumultd.log" 2>&1 &
    DPID=$!
    for _ in $(seq 1 60); do curl -fs $URL/healthz >/dev/null 2>&1 && break; sleep 0.5; done
    curl -fs $URL/healthz >/dev/null || { cat "$WORK/tumultd.log"; fail "daemon did not come up"; }
}
stop_daemon() { kill "$DPID"; wait "$DPID" 2>/dev/null || true; }
trap 'kill $DPID 2>/dev/null || true' EXIT

# Telemetry of one service as its SDK would export it: the ICT spine and
# the release identity on the resource (conformance level 2).
service_spans() { # <service.name> <ict.entity.id>
    python3 "$S/otlp.py" --service "$1" --count 3 \
        --resource "resilience.target.system=$1" \
        --resource resilience.target.technology=rust \
        --resource "resilience.target.environment=$ENV" \
        --resource resilience.target.component=api \
        --resource service.version=1.4.2 \
        --resource ict.schema.version=1 \
        --resource "ict.entity.id=$2" \
        --resource ict.entity.tier=critical \
        --resource ict.entity.owner=proof | post
}

# The root span of a completed restore test against a system, backdated.
restore_run() { # <target system> <ict.entity.id> <days ago>
    python3 "$S/otlp.py" --service tumult --name resilience.experiment --days-ago "$3" \
        --attr "resilience.experiment.id=proof-$1-$3" \
        --attr "resilience.experiment.name=$1 restore from backup" \
        --attr "resilience.target.system=$1" \
        --attr "resilience.target.environment=$ENV" \
        --attr resilience.test.kind=dr \
        --attr resilience.outcome.status=completed \
        --attr "ict.entity.id=$2" \
        --num resilience.outcome.recovery_time_s=120 | post
}

# Poll until the maturity document knows the service as a non-unknown node.
wait_for_node() { # <node id>
    for _ in $(seq 1 40); do
        curl -fs "$URL/api/maturity?by=node" >"$WORK/wait.json"
        py "import sys; sys.exit(0 if any(n['id']=='$1' and not n['unknown'] for n in d['nodes']) else 1)" <"$WORK/wait.json" && return 0
        sleep 0.5
    done
    fail "$1 never became a known node"
}

echo "[1] start tumultd on a fresh lake with a function register (no feed connected)"
sed 's/FEEDS/[]/' "$S/org.yaml" >"$WORK/org.yaml"
start_daemon
pass "daemon healthy"

echo "[2] the register: three functions with register codes"
for _ in $(seq 1 40); do
    curl -fs "$URL/api/dora" >"$WORK/dora0.json"
    py 'import sys; sys.exit(0 if len(d["functions"])==3 and all(f["registered"] for f in d["functions"]) else 1)' <"$WORK/dora0.json" && break
    sleep 0.5
done
py 'fs={f["entity_id"]:f for f in d["functions"]}
assert sorted(fs)==["cif-0001","cif-0002","cif-0003"], sorted(fs)
assert all(f["registered"] for f in fs.values()), fs
assert [fs[k]["register_codes"] for k in sorted(fs)]==[["F1"],["F2"],["F3"]], fs
assert fs["cif-0001"]["tier"]=="critical" and fs["cif-0001"]["org_unit"]=="firm/payments", fs["cif-0001"]
assert d["profile"]=="simplified" and d["readiness"]["functions"]==3 and d["readiness"]["registered"]==3, d["readiness"]' <"$WORK/dora0.json"
pass "cif-0001..3 registered as F1..F3 under the simplified profile"

echo "[3] the registry follows the profile"
py 'arts=[a["article"] for a in d["articles"]]
full=[a for a in arts if a.isdigit() and 5<=int(a)<=15]
assert not full, f"simplified profile must carry no Art. 5-15 rows: {full}"
assert "16" in arts and "rts1774" in arts, arts
assert d["na"]["count"]>0 and all(r["id"] for r in d["na"]["rows"])
assert "compliant" not in json.dumps(d).lower().replace("non-compliant","")' <"$WORK/dora0.json"
curl -fs "$URL/api/dora?profile=simplified" >"$WORK/dora-simplified.json"
py 'assert not [a["article"] for a in d["articles"] if a["article"].isdigit() and 5<=int(a["article"])<=15]' <"$WORK/dora-simplified.json"
curl -fs "$URL/api/dora?profile=full" >"$WORK/dora-full.json"
py 'arts=[a["article"] for a in d["articles"]]
assert d["profile"]=="full" and {"5","9","11","12"} <= set(arts) and "16" not in arts, arts' <"$WORK/dora-full.json"
code=$(curl -s -o /dev/null -w '%{http_code}' "$URL/api/dora?profile=gold")
[[ "$code" == 400 ]] || fail "unknown profile should be 400, got $code"
pass "simplified: no Art. 5-15 rows, Art. 16 and RTS 2024/1774 rows instead; full: the reverse; unknown profile 400"

echo "[4] telemetry from three services and three backdated restore tests"
service_spans payments-api cif-0001
service_spans reporting-api cif-0002
service_spans onboarding-api cif-0003
restore_run payments-api cif-0001 10
restore_run reporting-api cif-0002 180
restore_run onboarding-api cif-0003 179
wait_for_node svc:onboarding-api
for _ in $(seq 1 40); do
    curl -fs "$URL/api/dora/rts1774" >"$WORK/rts.json"
    py 'import sys; r=[r for r in d["requirements"] if r["id"]=="rts1774.art40"][0]; sys.exit(0 if all(e["state"]=="known" and e["status"] in ("existing","operating") for e in r["entities"]) else 1)' <"$WORK/rts.json" && break
    sleep 0.5
done
pass "spans and runs ingested over OTLP/HTTP"

echo "[5] evidence status: 180 unbroken days is Operating, 179 is Existing"
py 'r=[r for r in d["requirements"] if r["id"]=="rts1774.art40"][0]
e={x["entity_id"]:x for x in r["entities"]}
assert e["cif-0002"]["status"]=="operating" and e["cif-0002"]["streak_days"]==180, e["cif-0002"]
assert e["cif-0003"]["status"]=="existing" and e["cif-0003"]["streak_days"]==179 and e["cif-0003"]["days_until_operating"]==1, e["cif-0003"]
assert e["cif-0001"]["status"]=="existing" and e["cif-0001"]["days_until_operating"]==170, e["cif-0001"]
assert all(x["evidence"] for x in e.values()), "every cell lists its evidence rows"
print("     rts1774.art40:", {k:(v["status"], v["streak_days"]) for k,v in sorted(e.items())})' <"$WORK/rts.json"
pass "Reporting (run 180 days ago) is Operating; Onboarding (179 days ago) is Existing with 1 day to go"

echo "[6] no incident feed: Art. 19 is unknown, and every number carries its unknown share"
curl -fs "$URL/api/dora" >"$WORK/dora1.json"
py 'cells=[c for c in d["cells"] if c["article"]=="19"]
assert len(cells)==3 and all(c["state"]=="unknown" and c["coverage"] is None and c["unknown"]==1.0 for c in cells), cells
a=[a for a in d["articles"] if a["article"]=="19"][0]
assert a["coverage"] is None and a["unknown"]==1.0, a
assert 0<d["portfolio"]["unknown"]<1 and d["portfolio"]["coverage"] is not None, d["portfolio"]
assert all("unknown" in a for a in d["articles"]) and all("unknown" in f for f in d["functions"])' <"$WORK/dora1.json"
pass "Art. 19 cells are unknown (hatched on the page), not zero"

echo "[7] an uninstrumented critical service caps the level"
curl -fs "$URL/api/maturity?by=node" >"$WORK/maturity0.json"
py 'assert d["rollup_headline"]=="level 1, capped by svc:ledger-archive", d["rollup_headline"]
assert d["headline"]=="level 1 of target 3 (simplified)", d["headline"]
assert d["level"]==1 and d["flat_level"]==2 and d["capped"] is True, (d["level"], d["flat_level"])
c=d["cap"]; assert c["rule"]=="weakest_critical_child" and c["origin"]==["svc:ledger-archive"] and c["level"]==1, c
assert c["reason"]=="capped by svc:ledger-archive (critical, no telemetry and no test)", c
n={x["id"]:x for x in d["nodes"]}
a=n["svc:ledger-archive"]; assert a["unknown"] and a["critical"] and a["index"]==1.0 and a["level"]==1, a
chain=["ent:cif-0001","org:firm/payments","org:firm","org:"]
assert all(n[i]["cap"]["rule"]=="weakest_critical_child" and n[i]["cap"]["origin"]==["svc:ledger-archive"] and n[i]["level"]==1 for i in chain), [n[i]["cap"] for i in chain]
assert n["org:firm/back-office"]["cap"]["rule"]=="none" and n["org:firm/back-office"]["level"]>=2, n["org:firm/back-office"]
assert n["wl:proof/deployment/payments-api"]["parent"]=="svc:payments-api", n["wl:proof/deployment/payments-api"]
assert 0<d["rollup"]["unknown_share"]<1, d["rollup"]["unknown_share"]
print("     ", d["headline"], "|", d["rollup_headline"], "| index", d["index"], "pillars alone", d["flat_index"])' <"$WORK/maturity0.json"
for by in entity org; do
    curl -fs "$URL/api/maturity?by=$by" >"$WORK/maturity-$by.json"
    py "assert d['by']=='$by' and d['nodes'] and d['level']==1, d['by']" <"$WORK/maturity-$by.json"
done
pass "level 1, capped by svc:ledger-archive; the chain names it at every level up to the organisation"

echo "[8] instrumenting the service lifts the cap"
service_spans ledger-archive cif-0001
wait_for_node svc:ledger-archive
curl -fs "$URL/api/maturity?by=node" >"$WORK/maturity1.json"
py 'assert d["headline"]=="level 2 of target 3 (simplified)", d["headline"]
assert d["level"]==2 and d["target"]==3 and d["profile"]=="simplified", d["level"]
c=d["cap"]; assert c["rule"]=="gate" and c["level"]==2 and c["blocked_level"]==3, c
assert c["reason"].startswith("level 3 blocked by: "), c
assert d["rollup"]["cap"]["rule"]!="weakest_critical_child", d["rollup"]["cap"]
n={x["id"]:x for x in d["nodes"]}
a=n["svc:ledger-archive"]; assert not a["unknown"] and a["level"]>=2, a
assert d["components"]["slo"] is None and d["component_states"]["slo"]["state"]=="not_declared", d["component_states"]
assert d["component_states"]["loop"]["state"]=="unknown", d["component_states"]["loop"]
assert d["scale"]["internal"] is True
h=d["history"]; assert len(h)>=2 and h[-1]["level"]==d["level"] and h[-1]["index"]==d["index"], h
print("     ", d["headline"], "|", c["reason"], "| index", d["index"])' <"$WORK/maturity1.json"
python3 -c "import json; a=json.load(open('$WORK/maturity0.json')); b=json.load(open('$WORK/maturity1.json')); assert b['index']>a['index'] and b['level']>a['level'], (a['index'], b['index'])"
pass "level 2 of target 3 (simplified), now held by a gate with its reason"

echo "[9] ict.entity.id form: none malformed, then CF-0042 is flagged as received"
curl -fs "$URL/api/otel/conformance" >"$WORK/conf0.json"
py 'assert d["malformed_entity_ids"]==[], d["malformed_entity_ids"]
assert d["entity_id_pattern"]=="^[a-z][a-z0-9]*(-[a-z0-9]+)+$", d["entity_id_pattern"]' <"$WORK/conf0.json"
python3 "$S/otlp.py" --service legacy-batch --resource "resilience.target.system=legacy-batch" \
    --resource "resilience.target.environment=$ENV" --resource ict.entity.id=CF-0042 | post
for _ in $(seq 1 40); do
    curl -fs "$URL/api/otel/conformance" >"$WORK/conf1.json"
    py 'import sys; sys.exit(0 if d["malformed_entity_ids"] else 1)' <"$WORK/conf1.json" && break
    sleep 0.5
done
py 'm=d["malformed_entity_ids"]; assert len(m)==1 and m[0]["entity_id"]=="CF-0042" and m[0]["service"]=="legacy-batch", m
assert "cf-0042" not in json.dumps(d), "the id is flagged, never lower-cased"' <"$WORK/conf1.json"
curl -fs "$URL/api/dora" >"$WORK/dora2.json"
py 'assert sorted(f["entity_id"] for f in d["functions"])==["cif-0001","cif-0002","cif-0003"], "a malformed id must not create a function"' <"$WORK/dora2.json"
pass "zero malformed ids on the seeded lake; CF-0042 listed under its service, unchanged"

echo "[10] declare every feed and restart: nothing is ingested, so nothing changes"
curl -fs "$URL/api/maturity" >"$WORK/maturity-before-feeds.json"
stop_daemon
sed 's/FEEDS/[incident, slo, register, alert_rules, forecast]/' "$S/org.yaml" >"$WORK/org.yaml"
start_daemon
for _ in $(seq 1 40); do
    curl -fs "$URL/api/dora" >"$WORK/dora3.json"
    py 'import sys; sys.exit(0 if all(f["declared"] for f in d["feeds"]) else 1)' <"$WORK/dora3.json" && break
    sleep 0.5
done
py 'assert [f["name"] for f in d["feeds"]]==["incident","slo","register","alert_rules","forecast"], d["feeds"]
assert all(f["declared"] and not f["connected"] and "no ingest path" in f["reason"] for f in d["feeds"]), d["feeds"]
cells=[c for c in d["cells"] if c["article"]=="19"]
assert len(cells)==3 and all(c["state"]=="unknown" and c["coverage"] is None and c["unknown"]==1.0 for c in cells), cells
assert not [c for c in d["cells"] if c["state"]=="no_events"], "a declared feed must not read as connected"
fs={f["entity_id"]:f["register_codes"] for f in d["functions"]}
assert fs=={"cif-0001":["F1"],"cif-0002":["F2"],"cif-0003":["F3"]}, fs' <"$WORK/dora3.json"
python3 -c "import json; a=json.load(open('$WORK/dora1.json')); b=json.load(open('$WORK/dora3.json')); assert b['portfolio']['unknown']>=a['portfolio']['unknown'], (a['portfolio'], b['portfolio'])"
curl -fs "$URL/api/maturity" >"$WORK/maturity2.json"
py 'assert d["component_states"]["loop"]["state"]=="unknown", d["component_states"]["loop"]
g={x["level"]:x for x in d["gates"]}
assert not g[4]["passed"] and any("alert rules not exported" in f for f in g[4]["failing"]), g[4]
assert not g[5]["passed"] and any("no forecast with provenance" in f for f in g[5]["failing"]), g[5]
assert any("loop closure (L) is unknown" in f for f in g[4]["failing"]), g[4]' <"$WORK/maturity2.json"
python3 -c "import json; a=json.load(open('$WORK/maturity-before-feeds.json')); b=json.load(open('$WORK/maturity2.json'))
for k in ('level','index','flat_level','flat_index','headline','components','component_states'): assert a[k]==b[k], (k, a[k], b[k])"
pass "five feeds declared, none connected: Art. 19 still unknown, level, index and pillars unchanged, the level 4 and 5 gates stay shut; F1..F3 unchanged"

echo "[11] the score page is served"
code=$(curl -s -o "$WORK/scores.html" -w '%{http_code}' "$URL/scores?view=dora")
[[ "$code" == 200 ]] || fail "/scores returned $code"
grep -q '_app/immutable' "$WORK/scores.html" || fail "/scores is not the SPA shell"
cmp -s "$WORK/scores.html" "$REPO/web/build/200.html" || fail "/scores is not the SPA shell the daemon embeds"
curl -fs "$URL/scores.html" >"$WORK/scores-static.html"
cmp -s "$WORK/scores-static.html" "$REPO/web/build/scores.html" || fail "/scores.html differs from the built page"
grep -rq 'dora-heatmap' "$REPO/web/build/_app/immutable" || fail "the built bundle lacks the DORA view"
pass "/scores served by the daemon with the DORA view in the bundle"

echo "[12] the worked examples of the model (fixture inputs)"
(cd "$REPO" && ${PROOF_CARGO:-cargo} test -p tumult-compliance --lib worked_example >"$WORK/worked.out" 2>&1) || { tail -30 "$WORK/worked.out"; fail "worked-example tests failed"; }
for t in \
    'dora::maturity::tests::worked_example_a_is_level_4_with_index_4_42 ... ok' \
    'dora::maturity::tests::worked_example_b_is_level_2_with_index_3_60 ... ok' \
    'dora::report::tests::worked_example_a_reads_level_4_index_4_42 ... ok' \
    'dora::report::tests::worked_example_b_reads_level_2_of_target_3_with_its_cap ... ok'; do
    grep -qF "$t" "$WORK/worked.out" || { grep 'worked_example' "$WORK/worked.out" || true; fail "missing: $t"; }
done
pass "example A: level 4 at index 4.42; example B: level 2 at index 3.60, level 2 of target 3 (simplified)"
echo "ALL DORA-SCORE PROOF CHECKS PASSED (run $RUN)"
