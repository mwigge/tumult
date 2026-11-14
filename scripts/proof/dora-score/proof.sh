#!/usr/bin/env bash
# DORA coverage and maturity live proof: a fresh daemon with a function
# register, telemetry posted over OTLP the way services export it, and
# backdated test runs. The registry profile, the register codes, the
# unknown state of a declared feed, the 180-day operating rule over the
# days the daily job stored, the cap by an uninstrumented critical service
# and its lifting, the house parameters in force, the reload of the org
# file, the parameters held from the last valid file and the day that is
# not stored under stand-in defaults are all read back from the API. The worked examples of the model
# (level 4 at index 4.42, level 2 at index 3.60) are fixture inputs, so
# they are asserted by running their tests. No mocks. Exit non-zero on any
# failed expectation. Run it three times in a row before a release.
#
#   (cd web && npm ci && npm run build)
#   cargo build -p tumultd && cargo build -p tumult-api --example dora_store_status_days
#   bash scripts/proof/dora-score/proof.sh [run-number]
#
# The daemon listens on two ports that are free when the proof starts, and
# every write is preceded by a check that the listener on the HTTP port is
# the daemon this script started: the proof never posts into another
# daemon. Work files land under $PROOF_WORK/run-<n> (default: a new temp
# dir). PROOF_CARGO names the cargo command for the worked-example tests
# (default: cargo).
#
# The Operating status counts stored days, and a proof cannot wait 180
# days. Step 5 therefore stops the daemon and stores past days with
# dora_store_status_days, a test-only tool that runs the daemon's own
# daily job with the clock set to each day, against this proof's private
# lake. No endpoint does that, and the daemon never fills in a past day.
# The tool marks its rows with the source `proof`. A daemon leaves such
# days out; step 5 shows that first, and only then starts the daemon with
# KRONIKA_SCORE_PROOF_DAYS=1, after which every score response says that
# proof days are in use.
set -euo pipefail
S="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$S/../../.." && pwd)"
BIN=${TUMULT_BIN_DIR:-$REPO/target/debug}
STORE_DAYS=${PROOF_STORE_DAYS:-$BIN/examples/dora_store_status_days}

pass() { echo "  ok   $1"; }
fail() { echo "  FAIL $1"; exit 1; }

RUN=${1:-1}
[[ "$RUN" =~ ^[0-9]{1,6}$ ]] || { echo "  FAIL run number must be digits, got '$RUN'"; exit 1; }
if [[ -n "${PROOF_WORK:-}" ]]; then
    mkdir -p -- "$PROOF_WORK"
    ROOT=$(cd -- "$PROOF_WORK" && pwd -P)
else
    ROOT=$(mktemp -d -t dora-score-proof.XXXXXX)
    ROOT=$(cd -- "$ROOT" && pwd -P)
fi
# The work dir is removed and made again: it must be a run-<n> directory
# directly under a root that is neither /, the home directory nor the
# repository.
case "$ROOT" in
    /|"$HOME"|"$REPO"|"$REPO"/*) echo "  FAIL refusing to work under '$ROOT'"; exit 1 ;;
    /*) ;;
    *) echo "  FAIL the work root must be an absolute path, got '$ROOT'"; exit 1 ;;
esac
WORK=$ROOT/run-$RUN
[[ "$WORK" == "$ROOT/run-$RUN" && "$(dirname -- "$WORK")" == "$ROOT" ]] || fail "unexpected work dir '$WORK'"
[[ ! -L "$WORK" ]] || fail "'$WORK' is a symbolic link"
rm -rf -- "$WORK"
mkdir -p -- "$WORK"

[[ -x "$BIN/tumultd" ]] || fail "no tumultd in $BIN (cargo build -p tumultd)"
[[ -x "$STORE_DAYS" ]] || fail "no $STORE_DAYS (cargo build -p tumult-api --example dora_store_status_days)"
command -v ss >/dev/null || fail "ss (iproute2) is needed to check who listens on the proof port"

# Two ports that are free now. The daemon binding them is checked below.
read -r GRPC_PORT HTTP_PORT < <(python3 -c '
import socket
socks = [socket.socket() for _ in range(2)]
for s in socks: s.bind(("127.0.0.1", 0))
print(*[s.getsockname()[1] for s in socks])')
[[ "$GRPC_PORT" =~ ^[0-9]+$ && "$HTTP_PORT" =~ ^[0-9]+$ ]] || fail "could not find two free ports"

export TUMULT_LAKE_PATH=$WORK/lake.duckdb
export KRONIKA_OTLP_GRPC_ADDR=127.0.0.1:$GRPC_PORT
export KRONIKA_OTLP_HTTP_ADDR=127.0.0.1:$HTTP_PORT
export KRONIKA_METRICS_DIR=$REPO/metrics
export KRONIKA_MANIFESTS_DIR=$S/manifests
export KRONIKA_ORG_FILE=$WORK/org.yaml
# The org file is read again every second, so a change to it is in the
# lake within the polling of a step.
export KRONIKA_REGISTER_RELOAD_INTERVAL=1s
URL=http://127.0.0.1:$HTTP_PORT
ENV=proof

py() { python3 -c "import json,sys; d=json.load(sys.stdin); $1"; }

DPID=
# The daemon this script started is alive and is the listener on the HTTP
# port. Anything else aborts the proof before a byte is written.
own_daemon() {
    [[ -n "$DPID" ]] || fail "no daemon started"
    kill -0 "$DPID" 2>/dev/null || { tail -20 "$WORK/tumultd.log"; fail "the proof daemon (pid $DPID) is not running"; }
    local listeners
    listeners=$(ss -H -ltnp "sport = :$HTTP_PORT")
    [[ -n "$listeners" ]] || fail "nothing listens on port $HTTP_PORT"
    if grep -v "pid=$DPID," <<<"$listeners" | grep -q .; then
        fail "port $HTTP_PORT is held by another process: $listeners"
    fi
}
post() { own_daemon; curl -fsS --data-binary @- -H 'content-type: application/x-protobuf' "$URL/v1/traces" >/dev/null; }

start_daemon() {
    if [[ -n "$(ss -H -ltn "sport = :$HTTP_PORT")" || -n "$(ss -H -ltn "sport = :$GRPC_PORT")" ]]; then
        fail "port $HTTP_PORT or $GRPC_PORT was taken before the daemon started"
    fi
    RUST_LOG=warn "$BIN/tumultd" serve >>"$WORK/tumultd.log" 2>&1 &
    DPID=$!
    for _ in $(seq 1 60); do
        kill -0 "$DPID" 2>/dev/null || { cat "$WORK/tumultd.log"; fail "the daemon exited at start"; }
        curl -fs "$URL/healthz" >/dev/null 2>&1 && break
        sleep 0.5
    done
    curl -fs "$URL/healthz" >/dev/null || { cat "$WORK/tumultd.log"; fail "daemon did not come up"; }
    own_daemon
}
stop_daemon() {
    [[ -n "$DPID" ]] || return 0
    kill "$DPID" 2>/dev/null || true
    wait "$DPID" 2>/dev/null || true
    DPID=
}
trap stop_daemon EXIT

# A document computed after this call was made. The score endpoints reuse
# a response for a few seconds; a check that follows a write must not read
# one built before it.
fresh() { # <path> <file>
    local since
    since=$(python3 -c 'import time; print(time.time_ns())')
    for _ in $(seq 1 60); do
        curl -fs "$URL$1" >"$2"
        py "import sys; sys.exit(0 if d['as_of_ns']>=$since else 1)" <"$2" && return 0
        sleep 0.5
    done
    fail "$1 was never computed again"
}

# Poll a document until the expression holds for it.
until_true() { # <path> <file> <python expression over d>
    for _ in $(seq 1 60); do
        curl -fs "$URL$1" >"$2"
        py "import sys; sys.exit(0 if ($3) else 1)" <"$2" && return 0
        sleep 0.5
    done
    fail "$1 never satisfied: $3"
}

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

# Store the daily snapshot of past days with the daemon's own job. The
# daemon is stopped: a lake has one writer.
store_days() { # <from days ago> <to days ago> [skip days ago]
    [[ -z "$DPID" ]] || fail "stop the daemon before storing past days"
    [[ "$TUMULT_LAKE_PATH" == "$WORK/lake.duckdb" ]] || fail "the lake is not the proof's own"
    DORA_PROOF_PRIVATE_LAKE=$TUMULT_LAKE_PATH "$STORE_DAYS" "$TUMULT_LAKE_PATH" "$@" >"$WORK/store-days.out" 2>&1 \
        || { cat "$WORK/store-days.out"; fail "storing days $* failed"; }
}

# The restore-test row of every function: (status, streak, days to go).
RESTORE_ROW='r=[r for r in d["requirements"] if r["id"]=="rts1774.art40"][0]
e={x["entity_id"]:x for x in r["entities"]}
got={k:(v["status"], v["streak_days"], v["days_until_operating"]) for k,v in sorted(e.items())}'

# Poll until the maturity document knows the service as a non-unknown node.
wait_for_node() { # <node id>
    until_true "/api/maturity?by=node" "$WORK/wait.json" "any(n['id']=='$1' and not n['unknown'] for n in d['nodes'])"
}

echo "[1] start tumultd on a fresh lake with a function register (no feed declared)"
sed 's/FEEDS/[]/' "$S/org.yaml" >"$WORK/org.yaml"
start_daemon
pass "daemon healthy on ports it was given (pid $DPID is the listener on $HTTP_PORT)"

echo "[2] the register: three functions with register codes"
until_true /api/dora "$WORK/dora0.json" 'len(d["functions"])==3 and all(f["registered"] for f in d["functions"]) and d["register_state"]=="ok"'
py 'fs={f["entity_id"]:f for f in d["functions"]}
assert sorted(fs)==["cif-0001","cif-0002","cif-0003"], sorted(fs)
assert all(f["registered"] for f in fs.values()), fs
assert [fs[k]["register_codes"] for k in sorted(fs)]==[["F1"],["F2"],["F3"]], fs
assert fs["cif-0001"]["tier"]=="critical" and fs["cif-0001"]["org_unit"]=="firm/payments", fs["cif-0001"]
assert d["profile"]=="simplified" and d["readiness"]["functions"]==3 and d["readiness"]["registered"]==3, d["readiness"]
p=d["params"]; assert p["fallback"] is False and p["reason"] is None and p["overridden"]==[], p
assert d["register_state"]=="ok" and d["profile_overridden"] is False and d["scoped"] is False, d["register_state"]' <"$WORK/dora0.json"
pass "cif-0001..3 registered as F1..F3 under the simplified profile; house defaults in force, register state ok"

echo "[3] the registry follows the profile"
py 'arts=[a["article"] for a in d["articles"]]
full=[a for a in arts if a.isdigit() and 5<=int(a)<=15]
assert not full, f"simplified profile must carry no Art. 5-15 rows: {full}"
assert "16" in arts and "rts1774" in arts, arts
assert d["na"]["count"]>0 and all(r["id"] for r in d["na"]["rows"])
assert "compliant" not in json.dumps(d).lower().replace("non-compliant","")' <"$WORK/dora0.json"
curl -fs "$URL/api/dora?profile=simplified" >"$WORK/dora-simplified.json"
py 'assert not [a["article"] for a in d["articles"] if a["article"].isdigit() and 5<=int(a["article"])<=15]
assert d["profile_overridden"] is False, "asking for the registered profile is no override"' <"$WORK/dora-simplified.json"
curl -fs "$URL/api/dora?profile=full" >"$WORK/dora-full.json"
py 'arts=[a["article"] for a in d["articles"]]
assert d["profile"]=="full" and {"5","9","11","12"} <= set(arts) and "16" not in arts, arts
assert d["profile_overridden"] is True and d["registered_profile"]=="simplified", (d["profile_overridden"], d["registered_profile"])' <"$WORK/dora-full.json"
code=$(curl -s -o /dev/null -w '%{http_code}' "$URL/api/dora?profile=gold")
[[ "$code" == 400 ]] || fail "unknown profile should be 400, got $code"
pass "simplified: no Art. 5-15 rows, Art. 16 and RTS 2024/1774 rows instead; ?profile=full is marked as an override; unknown profile 400"

echo "[4] telemetry from three services and three backdated restore tests"
service_spans payments-api cif-0001
service_spans reporting-api cif-0002
service_spans onboarding-api cif-0003
restore_run payments-api cif-0001 10
restore_run reporting-api cif-0002 180
restore_run onboarding-api cif-0003 179
wait_for_node svc:onboarding-api
until_true /api/dora/rts1774 "$WORK/rts0.json" 'all(e["state"]=="known" and e["status"]=="existing" for r in d["requirements"] if r["id"]=="rts1774.art40" for e in r["entities"])'
pass "spans and runs ingested over OTLP/HTTP"

echo "[5] evidence status: Operating is 180 days the daily job stored, not the age of the evidence"
py "$RESTORE_ROW"'
assert got=={"cif-0001":("existing",0,180),"cif-0002":("existing",0,180),"cif-0003":("existing",0,180)}, got
assert all(x["evidence"] for x in e.values()), "every cell lists its evidence rows"
print("     no stored day:", got)' <"$WORK/rts0.json"
pass "a run 180 days old with no stored day is Existing with a streak of 0"
stop_daemon
store_days 180 1 50
start_daemon
fresh /api/dora/rts1774 "$WORK/rts-no-opt-in.json"
py "$RESTORE_ROW"'
assert got=={"cif-0001":("existing",0,180),"cif-0002":("existing",0,180),"cif-0003":("existing",0,180)}, got
p=d["proof_days"]
assert p["enabled"] is False and p["counted"]==0 and p["note"] is None, p
assert p["ignored"]==179, p' <"$WORK/rts-no-opt-in.json"
curl -fs "$URL/api/maturity" >"$WORK/maturity-no-opt-in.json"
py 'assert d["proof_days"]["enabled"] is False and d["proof_days"]["ignored"]==179, d["proof_days"]
assert all(p["state"]=="no_data" for p in d["history"][:-1]), d["history"]' <"$WORK/maturity-no-opt-in.json"
pass "179 days stored by the proof tool, daemon without the opt-in: none counts, the streak stays 0, the history has no data"
stop_daemon
export KRONIKA_SCORE_PROOF_DAYS=1
start_daemon
fresh /api/dora/rts1774 "$WORK/rts1.json"
py "$RESTORE_ROW"'
assert got=={"cif-0001":("existing",10,170),"cif-0002":("existing",49,131),"cif-0003":("existing",49,131)}, got
p=d["proof_days"]
assert p["enabled"] is True and p["counted"]==179 and p["ignored"]==0, p
assert p["note"].startswith("proof days are in use") and "not evidence of operation" in p["note"], p
print("     days 180..1 stored, day 50 missed:", got)
print("     ", p["note"])' <"$WORK/rts1.json"
grep -q 'proof days are in use' "$WORK/tumultd.log" || fail "the daemon log does not say that proof days are in use"
pass "with KRONIKA_SCORE_PROOF_DAYS=1 the response and the log say proof days are in use; one missed day (50 days ago) breaks the streak: 49 days for Reporting and Onboarding"
stop_daemon
store_days 50 50
start_daemon
fresh /api/dora/rts1774 "$WORK/rts.json"
py "$RESTORE_ROW"'
assert got=={"cif-0001":("existing",10,170),"cif-0002":("operating",180,None),"cif-0003":("existing",179,1)}, got
print("     every day stored:", got)' <"$WORK/rts.json"
pass "Reporting (180 stored days) is Operating; Onboarding (179) is Existing with 1 day to go; Payments has 170 to go"

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
py 'import re
assert d["rollup_headline"]=="level 1, capped by svc:ledger-archive", d["rollup_headline"]
assert re.fullmatch(r"level 1 of target 3 \(simplified\), \d+ % unknown", d["headline"]), d["headline"]
assert d["headline"].endswith(", %d %% unknown" % round(d["unknown_share"]*100)), (d["headline"], d["unknown_share"])
assert d["unknown_share"]==d["rollup"]["unknown_share"], "the share shown is the one of the rolled-up index"
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

echo "[8] instrumenting the service lifts the cap; history is the stored days"
service_spans ledger-archive cif-0001
wait_for_node svc:ledger-archive
fresh "/api/maturity?by=node" "$WORK/maturity1.json"
py 'import re
assert re.fullmatch(r"level 2 of target 3 \(simplified\), \d+ % unknown", d["headline"]), d["headline"]
assert d["level"]==2 and d["target"]==3 and d["profile"]=="simplified", d["level"]
c=d["cap"]; assert c["rule"]=="gate" and c["level"]==2 and c["blocked_level"]==3, c
assert c["reason"].startswith("level 3 blocked by: "), c
assert d["rollup"]["cap"]["rule"]!="weakest_critical_child", d["rollup"]["cap"]
n={x["id"]:x for x in d["nodes"]}
a=n["svc:ledger-archive"]; assert not a["unknown"] and a["level"]>=2, a
assert d["components"]["slo"] is None and d["component_states"]["slo"]["state"]=="not_declared", d["component_states"]
assert d["component_states"]["loop"]["state"]=="unknown", d["component_states"]["loop"]
assert d["scale"]["internal"] is True
h=d["history"]; assert len(h)==5 and h[-1]["state"]=="today" and h[-1]["level"]==d["level"] and h[-1]["index"]==d["index"], h
past=h[:-1]
assert {p["state"] for p in past}=={"stored","no_data"}, [p["state"] for p in past]
assert past[0]["state"]=="no_data", "a quarter that ended before the first stored day has no data"
assert d["proof_days"]["enabled"] is True and d["proof_days"]["note"], d["proof_days"]
for p in past:
    if p["state"]=="stored":
        assert p["source"]=="proof", "a day of the proof tool is named as one"
        assert isinstance(p["level"], int) and p["index"] is not None and p["as_of"], p
    else:
        assert p["level"] is None and p["index"] is None and p["unknown_share"] is None, p
print("     ", d["headline"], "|", c["reason"], "| index", d["index"])
print("      history:", [(p["quarter"], p["state"], p["level"]) for p in h])' <"$WORK/maturity1.json"
python3 -c "import json; a=json.load(open('$WORK/maturity0.json')); b=json.load(open('$WORK/maturity1.json')); assert b['index']>a['index'] and b['level']>a['level'], (a['index'], b['index'])"
curl -fs "$URL/api/maturity?profile=full" >"$WORK/maturity-full.json"
py 'assert d["profile_overridden"] is True
assert all(p["state"]=="no_data" and p["reason"] for p in d["history"][:-1]), d["history"]' <"$WORK/maturity-full.json"
pass "level 2 of target 3 (simplified), held by a gate with its reason; past quarters are stored days or no data, never recomputed"

echo "[9] ict.entity.id form: none malformed, then CF-0042 is flagged as received"
curl -fs "$URL/api/otel/conformance" >"$WORK/conf0.json"
py 'assert d["malformed_entity_ids"]==[], d["malformed_entity_ids"]
assert d["malformed_entity_ids_truncated"] is False
assert d["entity_id_pattern"]=="^[a-z][a-z0-9]*(-[a-z0-9]+)+$", d["entity_id_pattern"]' <"$WORK/conf0.json"
python3 "$S/otlp.py" --service legacy-batch --resource "resilience.target.system=legacy-batch" \
    --resource "resilience.target.environment=$ENV" --resource ict.entity.id=CF-0042 | post
until_true /api/otel/conformance "$WORK/conf1.json" 'd["malformed_entity_ids"]'
py 'm=d["malformed_entity_ids"]; assert len(m)==1 and m[0]["entity_id"]=="CF-0042" and m[0]["service"]=="legacy-batch" and m[0]["truncated"] is False, m
assert "cf-0042" not in json.dumps(d), "the id is flagged, never lower-cased"' <"$WORK/conf1.json"
fresh /api/dora "$WORK/dora2.json"
py 'assert sorted(f["entity_id"] for f in d["functions"])==["cif-0001","cif-0002","cif-0003"], "a malformed id must not create a function"' <"$WORK/dora2.json"
curl -fs "$URL/metrics" >"$WORK/metrics.txt"
grep -Eq '^tumultd_malformed_entity_ids_total [1-9]' "$WORK/metrics.txt" || fail "the malformed id was not counted in /metrics"
pass "zero malformed ids on the seeded lake; CF-0042 listed under its service, unchanged, counted, and no function made of it"

echo "[10] declare every feed and restart: nothing is ingested, so nothing changes"
fresh /api/maturity "$WORK/maturity-before-feeds.json"
stop_daemon
sed 's/FEEDS/[incident, slo, register, alert_rules, forecast]/' "$S/org.yaml" >"$WORK/org.yaml"
start_daemon
until_true /api/dora "$WORK/dora3.json" 'all(f["declared"] for f in d["feeds"]) and d["register_state"]=="ok"'
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

echo "[11] a changed org file is read without a restart; a lowered gate is shown, a refused file leaves the last valid one in force"
own_daemon
cp "$WORK/org.yaml" "$WORK/org.valid.yaml"
printf 'score:\n  operating_days: 30\n' >>"$WORK/org.yaml"
until_true /api/dora/rts1774 "$WORK/rts-lowered.json" 'd["params"]["overridden"]'
py "$RESTORE_ROW"'
p=d["params"]
assert p["source"]=="org_file" and p["fallback"] is False and p["overridden"]==[{"key":"operating_days","value":30,"default":180}], p
assert d["register_state"] in ("ok","stale"), d["register_state"]
assert got["cif-0003"][0]=="operating" and got["cif-0002"][0]=="operating" and got["cif-0001"][0]=="existing", got
print("     operating_days 30:", got)' <"$WORK/rts-lowered.json"
pass "operating_days lowered to 30 in the org file: Onboarding reads Operating, and the response names the override and the default"
own_daemon
printf '  band_good: 7\n' >>"$WORK/org.yaml"
until_true /api/dora/rts1774 "$WORK/rts-held.json" 'd["params"]["held"]'
py "$RESTORE_ROW"'
p=d["params"]
assert p["source"]=="last_valid_org_file" and p["held"] is True and p["fallback"] is False and "band_good" in p["reason"], p
assert p["overridden"]==[{"key":"operating_days","value":30,"default":180}], "the last valid file stays in force, not the defaults"
assert d["register_state"]=="invalid" and "band_good" in d["register_reason"], (d["register_state"], d["register_reason"])
s=d["snapshot"]; assert s["skipping"] is False and s["skipped"]==0 and s["reason"] is None, s
assert got["cif-0003"][0]=="operating" and got["cif-0001"][0]=="existing", got' <"$WORK/rts-held.json"
curl -fs "$URL/api/dora" >"$WORK/dora-refused.json"
py 'assert len(d["functions"])==3 and all(f["registered"] for f in d["functions"]), "the register in the lake stays as it was"' <"$WORK/dora-refused.json"
pass "a file refused after a valid one: the last valid parameters stay in force (operating_days 30), named as held with the reason; register state invalid, the register in the lake untouched"
own_daemon
cp "$WORK/org.valid.yaml" "$WORK/org.yaml"
until_true /api/dora "$WORK/dora-restored.json" 'd["register_state"]=="ok" and not d["params"]["fallback"] and not d["params"]["held"] and not d["params"]["overridden"]'
pass "the valid org file is back in force without a restart"

echo "[12] a daemon that never saw a valid org file scores with defaults, says so, and stores no day"
stop_daemon
printf 'score:\n  band_good: 7\n' >>"$WORK/org.yaml"
start_daemon
until_true /api/dora/rts1774 "$WORK/rts-refused.json" 'd["snapshot"]["skipped"]>=1'
py "$RESTORE_ROW"'
p=d["params"]
assert p["source"]=="defaults" and p["fallback"] is True and p["held"] is False and "band_good" in p["reason"] and p["overridden"]==[], p
assert d["register_state"]=="invalid" and "band_good" in d["register_reason"], (d["register_state"], d["register_reason"])
s=d["snapshot"]
assert s["skipping"] is True and s["skipped"]>=1, s
assert s["reason"].startswith("the daily snapshot is not stored while the default parameters stand in") and "band_good" in s["reason"], s
assert got=={"cif-0001":("existing",10,170),"cif-0002":("operating",180,None),"cif-0003":("existing",179,1)}, got
print("     ", s["reason"], "| skipped", s["skipped"])' <"$WORK/rts-refused.json"
grep -q 'status_snapshot_skipped' "$WORK/tumultd.log" || fail "the skipped snapshot is not in the daemon log"
cp "$WORK/org.valid.yaml" "$WORK/org.yaml"
until_true /api/dora "$WORK/dora-unblocked.json" 'd["register_state"]=="ok" and not d["params"]["fallback"] and not d["snapshot"]["skipping"]'
pass "refused from the start: defaults in force with the reason, the daily snapshot skipped, counted and logged; a valid file ends it without a restart"

echo "[13] the score page is served"
code=$(curl -s -o "$WORK/scores.html" -w '%{http_code}' "$URL/scores?view=dora")
[[ "$code" == 200 ]] || fail "/scores returned $code"
grep -q '_app/immutable' "$WORK/scores.html" || fail "/scores is not the SPA shell"
cmp -s "$WORK/scores.html" "$REPO/web/build/200.html" || fail "/scores is not the SPA shell the daemon embeds"
curl -fs "$URL/scores.html" >"$WORK/scores-static.html"
cmp -s "$WORK/scores-static.html" "$REPO/web/build/scores.html" || fail "/scores.html differs from the built page"
grep -rq 'dora-heatmap' "$REPO/web/build/_app/immutable" || fail "the built bundle lacks the DORA view"
grep -rq 'score-params' "$REPO/web/build/_app/immutable" || fail "the built bundle lacks the parameter block"
grep -rq 'proof-days' "$REPO/web/build/_app/immutable" || fail "the built bundle lacks the proof-days notice"
grep -rq 'snapshot-skipping' "$REPO/web/build/_app/immutable" || fail "the built bundle lacks the skipped-snapshot notice"
pass "/scores served by the daemon with the DORA view in the bundle"

echo "[14] the worked examples of the model (fixture inputs)"
(cd "$REPO" && ${PROOF_CARGO:-cargo} test -p tumult-compliance --lib worked_example >"$WORK/worked.out" 2>&1) || { tail -30 "$WORK/worked.out"; fail "worked-example tests failed"; }
for t in \
    'dora::maturity::tests::worked_example_a_is_level_4_with_index_4_42 ... ok' \
    'dora::maturity::tests::worked_example_b_is_level_2_with_index_3_60 ... ok' \
    'dora::report::tests::worked_example_a_reads_level_4_index_4_42 ... ok' \
    'dora::report::tests::worked_example_b_reads_level_2_of_target_3_with_its_cap ... ok'; do
    grep -qF "$t" "$WORK/worked.out" || { grep 'worked_example' "$WORK/worked.out" || true; fail "missing: $t"; }
done
pass "example A: level 4 at index 4.42; example B: level 2 at index 3.60, level 2 of target 3 (simplified)"
stop_daemon
echo "ALL DORA-SCORE PROOF CHECKS PASSED (run $RUN)"
