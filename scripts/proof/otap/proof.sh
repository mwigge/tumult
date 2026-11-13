#!/usr/bin/env bash
# OTel-Arrow carrier live proof: a fresh tumultd, a real OpenTelemetry
# Collector (contrib) container with an `otlp` receiver and an `otelarrow`
# exporter that may not fall back to OTLP, OTLP/HTTP JSON spans sent into the
# Collector with curl, and the lake queried through the daemon's API for the
# promoted columns. No mocks. Exit non-zero on any failed expectation. Run it
# three times in a row before a release.
#
#   cargo build -p tumultd
#   bash scripts/proof/otap/proof.sh [run-number]
#
# Needs docker and the otel/opentelemetry-collector-contrib image
# (COLLECTOR_TAG, default 0.157.0); the Collector runs on the host network so
# it reaches the loopback-bound daemon. Ports 45317/45318/45418 must be free.
# Work files land under $PROOF_WORK (default: a temp dir).
set -euo pipefail
S="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$S/../../.." && pwd)"
BIN=${TUMULT_BIN_DIR:-${CARGO_TARGET_DIR:-$REPO/target}/debug}
RUN=${1:-1}
WORK=${PROOF_WORK:-$(mktemp -d)}/run-$RUN; rm -rf "$WORK"; mkdir -p "$WORK"
COLLECTOR_TAG=${COLLECTOR_TAG:-0.157.0}
GRPC_PORT=45317
HTTP_PORT=45318
COLLECTOR_PORT=45418
export TUMULT_LAKE_PATH=$WORK/lake.duckdb
export KRONIKA_OTLP_GRPC_ADDR=127.0.0.1:$GRPC_PORT
export KRONIKA_OTLP_HTTP_ADDR=127.0.0.1:$HTTP_PORT
export KRONIKA_METRICS_DIR=$REPO/metrics
export KRONIKA_INGEST_TOKEN=proof-otap-token-$RUN
URL=http://127.0.0.1:$HTTP_PORT
CONTAINER=otap-proof-$RUN

pass() { echo "  ok   $1"; }
fail() { echo "  FAIL $1"; exit 1; }
py() { python3 -c "import json,sys; d=json.load(sys.stdin); $1"; }
cleanup() {
  docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
  kill "${DPID:-0}" 2>/dev/null || true
}
trap cleanup EXIT

echo "[1] start tumultd on a fresh lake (ingest token required on gRPC)"
RUST_LOG=info,tumult_ingest::otap=debug "$BIN/tumultd" serve >"$WORK/tumultd.log" 2>&1 &
DPID=$!
for _ in $(seq 1 60); do curl -fs "$URL/healthz" >/dev/null 2>&1 && break; sleep 0.5; done
curl -fs "$URL/healthz" >/dev/null || { cat "$WORK/tumultd.log"; fail "daemon did not come up"; }
pass "daemon healthy"

echo "[2] start the Collector: otlp receiver -> otelarrow exporter (default zstd gRPC compression) -> tumultd, no OTLP downgrade"
docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
docker run -d --name "$CONTAINER" \
  --network host \
  -v "$S/collector.yaml:/etc/otelcol-contrib/config.yaml:ro" \
  -e KRONIKA_INGEST_TOKEN -e "PROOF_GRPC_PORT=$GRPC_PORT" -e "PROOF_COLLECTOR_PORT=$COLLECTOR_PORT" \
  "otel/opentelemetry-collector-contrib:$COLLECTOR_TAG" \
  --config=/etc/otelcol-contrib/config.yaml >/dev/null
for _ in $(seq 1 60); do
  curl -s -o /dev/null "http://127.0.0.1:$COLLECTOR_PORT/v1/traces" 2>/dev/null && break; sleep 0.5
done
docker logs "$CONTAINER" >"$WORK/collector.log" 2>&1
grep -q 'Everything is ready' "$WORK/collector.log" || { cat "$WORK/collector.log"; fail "collector did not start"; }
pass "collector $COLLECTOR_TAG ready with the otelarrow exporter"

echo "[3] send OTLP/HTTP JSON spans with resilience.target.* resource attributes into the Collector"
TRACE=$(printf '%032x' "$((RANDOM * 32768 + RANDOM))$RUN" | tail -c 32)
EXP="exp-otap-proof-$RUN-$$"
NOW=$(date +%s%N)
cat >"$WORK/spans.json" <<EOF
{"resourceSpans":[{"resource":{"attributes":[
  {"key":"service.name","value":{"stringValue":"proof-checkout"}},
  {"key":"service.version","value":{"stringValue":"1.4.2"}},
  {"key":"resilience.target.system","value":{"stringValue":"proof-db"}},
  {"key":"resilience.target.technology","value":{"stringValue":"postgres"}},
  {"key":"resilience.target.environment","value":{"stringValue":"proof"}},
  {"key":"resilience.target.component","value":{"stringValue":"shop/checkout"}}
]},"scopeSpans":[{"scope":{"name":"proof","version":"1"},"spans":[
  {"traceId":"$TRACE","spanId":"0000000000000001","name":"resilience.experiment","kind":1,
   "startTimeUnixNano":"$((NOW - 3000000000))","endTimeUnixNano":"$NOW",
   "attributes":[
     {"key":"resilience.experiment.id","value":{"stringValue":"$EXP"}},
     {"key":"resilience.experiment.title","value":{"stringValue":"otap-proof"}},
     {"key":"resilience.fault.type","value":{"stringValue":"termination"}},
     {"key":"resilience.target.name","value":{"stringValue":"proof-db"}},
     {"key":"resilience.outcome.status","value":{"stringValue":"completed"}},
     {"key":"resilience.outcome.hypothesis_met","value":{"boolValue":true}},
     {"key":"resilience.outcome.recovery_time_s","value":{"doubleValue":4.5}},
     {"key":"resilience.estimate.rationale","value":{"stringValue":"primary restart is fast"}},
     {"key":"retries","value":{"intValue":"3"}}],
   "events":[{"timeUnixNano":"$((NOW - 2000000000))","name":"fault.injected",
              "attributes":[{"key":"action","value":{"stringValue":"pod-kill"}}]}],
   "status":{"code":1}},
  {"traceId":"$TRACE","spanId":"0000000000000002","parentSpanId":"0000000000000001",
   "name":"resilience.action","kind":1,
   "startTimeUnixNano":"$((NOW - 2900000000))","endTimeUnixNano":"$((NOW - 2500000000))",
   "attributes":[{"key":"resilience.fault.severity","value":{"stringValue":"high"}},
                 {"key":"targets","value":{"arrayValue":{"values":[{"stringValue":"pg-0"},{"intValue":"7"}]}}}],
   "status":{"code":2,"message":"pod not found"}},
  {"traceId":"$TRACE","spanId":"0000000000000003","parentSpanId":"0000000000000001",
   "name":"GET /checkout","kind":2,
   "startTimeUnixNano":"$((NOW - 1000000000))","endTimeUnixNano":"$((NOW - 950000000))",
   "attributes":[{"key":"http.request.method","value":{"stringValue":"GET"}},
                 {"key":"http.response.status_code","value":{"intValue":"200"}}]}
]}]}]}
EOF
code=$(curl -s -o "$WORK/post.out" -w '%{http_code}' -X POST -H 'content-type: application/json' \
  --data-binary @"$WORK/spans.json" "http://127.0.0.1:$COLLECTOR_PORT/v1/traces")
[[ "$code" == 200 ]] || { cat "$WORK/post.out"; fail "collector refused the OTLP/HTTP post ($code)"; }
pass "3 spans accepted by the Collector (trace $TRACE)"

echo "[4] the spans arrive in the lake over the Arrow stream"
for _ in $(seq 1 60); do
  curl -fs "$URL/api/traces/$TRACE" >"$WORK/trace.json" 2>/dev/null && break; sleep 0.5
done
[[ -s "$WORK/trace.json" ]] || { docker logs "$CONTAINER" 2>&1 | tail -20; tail -20 "$WORK/tumultd.log"; fail "trace never arrived"; }
grep -q 'OTAP stream opened' "$WORK/tumultd.log" || fail "daemon log shows no OTel-Arrow stream (did the exporter downgrade?)"
grep -q "zstdarrow gRPC encoding accepted" "$WORK/tumultd.log" || fail "exporter did not use its default zstdarrow gRPC compression"
docker logs "$CONTAINER" >"$WORK/collector.log" 2>&1
grep -qi 'downgrad' "$WORK/collector.log" && fail "collector downgraded to OTLP"
grep -q 'arrow is not supported\|zstdarrow' "$WORK/collector.log" && fail "daemon refused the exporter's default gRPC compression (zstdarrow)"
py "sp=d['spans']; assert len(sp)==3, sp
root=[x for x in sp if x['span_name']=='resilience.experiment'][0]
assert root['experiment_id']=='$EXP' and root['experiment_name']=='otap-proof', root
assert root['fault_type']=='termination' and root['status_code']=='Ok' and root['service_name']=='proof-checkout', root
assert root['span_attrs']['resilience.estimate.rationale']=='primary restart is fast', root['span_attrs']
assert root['span_attrs']['retries']=='3' and 'resilience.experiment.id' not in root['span_attrs'], root['span_attrs']
ev=root['events']; assert ev[0]['name']=='fault.injected' and ev[0]['attributes']['action']=='pod-kill', ev
act=[x for x in sp if x['span_name']=='resilience.action'][0]
assert act['parent_span_id']=='0000000000000001' and act['status_code']=='Error' and act['status_message']=='pod not found', act
assert act['span_attrs']['targets']=='[\"pg-0\",\"7\"]', act['span_attrs']
get=[x for x in sp if x['span_name']=='GET /checkout'][0]
assert get['span_kind']=='Server' and get['span_attrs']['http.response.status_code']=='200', get
print('     spans', [(x['span_name'], x['status_code']) for x in sp])" <"$WORK/trace.json"
pass "3 spans decoded from OTel-Arrow (zstdarrow gRPC compression accepted): ids, kinds, status, events, unpromoted attrs"

echo "[5] promoted columns from the resource: target system/technology/environment on the experiment"
curl -fs "$URL/api/experiments" >"$WORK/experiments.json"
py "e=[x for x in d['experiments'] if x['id']=='$EXP']; assert e, d
e=e[0]; assert (e['target_system'],e['target_technology'],e['target_environment'])==('proof-db','postgres','proof'), e
print('     experiment', e['id'], e['target_system'], e['target_technology'], e['target_environment'])" <"$WORK/experiments.json"
curl -fs "$URL/api/traces?service=proof-checkout" >"$WORK/traces.json"
py "t=[x for x in d['traces'] if x['trace_id']=='$TRACE']; assert t and t[0]['span_count']==3 and t[0]['error_count']==1, d" <"$WORK/traces.json"
curl -fs "$URL/api/topology" >"$WORK/topology.json"
py "n={(x['name'],x['type']) for x in d['nodes']}; assert ('proof-checkout','service') in n and ('proof-db','target') in n, n
assert any(e['from_id']=='svc:proof-checkout' and e['to_id']=='tgt:proof-db' for e in d['edges']), d['edges']" <"$WORK/topology.json"
pass "service name, target_* columns, trace listing and topology edge all answer from the lake"

echo "[6] a wrong ingest token is refused on the Arrow stream (nothing silently dropped)"
docker rm -f "$CONTAINER" >/dev/null 2>&1
KRONIKA_INGEST_TOKEN=wrong docker run -d --name "$CONTAINER" \
  --network host \
  -v "$S/collector.yaml:/etc/otelcol-contrib/config.yaml:ro" \
  -e KRONIKA_INGEST_TOKEN -e "PROOF_GRPC_PORT=$GRPC_PORT" -e "PROOF_COLLECTOR_PORT=$COLLECTOR_PORT" \
  "otel/opentelemetry-collector-contrib:$COLLECTOR_TAG" --config=/etc/otelcol-contrib/config.yaml >/dev/null
for _ in $(seq 1 60); do
  curl -s -o /dev/null "http://127.0.0.1:$COLLECTOR_PORT/v1/traces" 2>/dev/null && break; sleep 0.5
done
sed -i "s/$TRACE/$(printf '%032x' 7)/" "$WORK/spans.json"
curl -s -o /dev/null -X POST -H 'content-type: application/json' --data-binary @"$WORK/spans.json" "http://127.0.0.1:$COLLECTOR_PORT/v1/traces" || true
sleep 3
docker logs "$CONTAINER" >"$WORK/collector-wrong.log" 2>&1
grep -qi 'unauthenticated\|bearer' "$WORK/collector-wrong.log" || { tail -20 "$WORK/collector-wrong.log"; fail "collector did not report the rejected token"; }
code=$(curl -s -o /dev/null -w '%{http_code}' "$URL/api/traces/$(printf '%032x' 7)")
[[ "$code" == 404 ]] || fail "spans sent with a wrong token must not land (got $code)"
pass "UNAUTHENTICATED reported to the exporter; lake untouched"
echo "ALL OTEL-ARROW PROOF CHECKS PASSED (run $RUN)"
