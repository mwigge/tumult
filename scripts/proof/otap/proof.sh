#!/usr/bin/env bash
# OTel-Arrow carrier live proof: a fresh tumultd, a real OpenTelemetry
# Collector (contrib) container with an `otlp` receiver and an `otelarrow`
# exporter that may not fall back to OTLP, OTLP/HTTP JSON traces, logs and
# metrics sent into the Collector with curl, and the lake queried through the
# daemon's API. Two trace batches share one Arrow stream, so the second is
# decoded with the first one's schemas and dictionaries. A small gRPC probe
# then plays the hostile client (no token, wrong token, undecodable payload,
# oversized message) and the exact status of each is asserted. No mocks. Exit
# non-zero on any failed expectation. Run it three times in a row before a
# release.
#
#   cargo build -p tumultd
#   cargo build -p tumult-ingest --features otap-client --example otap_probe
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
COLLECTOR=http://127.0.0.1:$COLLECTOR_PORT
CONTAINER=otap-proof-$RUN
DLOG=$WORK/tumultd.log
PROBE=$BIN/examples/otap_probe

pass() { echo "  ok   $1"; }
fail() { echo "  FAIL $1"; exit 1; }
py() { python3 -c "import json,sys; d=json.load(sys.stdin); $1"; }
# poll <what> <command...>: retry for up to 30 s, fail the proof otherwise.
poll() {
  local what=$1; shift
  for _ in $(seq 1 60); do
    if "$@" >/dev/null 2>&1; then return 0; fi
    sleep 0.5
  done
  fail "timed out waiting for $what"
}
# plain <file>: the file without terminal colour codes.
plain() { sed 's/\x1b\[[0-9;]*m//g' "$1"; }
# lines <file> <fixed string>...: how many lines contain every string.
lines() {
  local file=$1; shift
  plain "$file" | python3 -c 'import sys
print(sum(all(n in line for n in sys.argv[1:]) for line in sys.stdin))' "$@"
}
# expect_lines <n> <file> <fixed string>...
expect_lines() {
  local want=$1 got; shift
  got=$(lines "$@")
  [[ "$got" == "$want" ]] || { tail -30 "$1"; fail "expected $want line(s) with [${*:2}] in $1, found $got"; }
}
has_lines() { [[ "$(lines "${@:2}")" == "$1" ]]; }
remove_container() {
  if docker container inspect "$CONTAINER" >/dev/null 2>&1; then docker rm -f "$CONTAINER" >/dev/null; fi
}
cleanup() {
  remove_container
  if [[ -n "${DPID:-}" ]] && kill -0 "$DPID" 2>/dev/null; then kill "$DPID"; fi
}
trap cleanup EXIT
start_collector() { # start_collector <ingest token>
  remove_container
  docker run -d --name "$CONTAINER" \
    --network host \
    -v "$S/collector.yaml:/etc/otelcol-contrib/config.yaml:ro" \
    -e "KRONIKA_INGEST_TOKEN=$1" -e "PROOF_GRPC_PORT=$GRPC_PORT" -e "PROOF_COLLECTOR_PORT=$COLLECTOR_PORT" \
    "otel/opentelemetry-collector-contrib:$COLLECTOR_TAG" \
    --config=/etc/otelcol-contrib/config.yaml >/dev/null
  poll "the collector to start" collector_ready
}
collector_log() { docker logs "$CONTAINER" >"$WORK/collector.log" 2>&1; }
collector_ready() {
  collector_log && has_lines 1 "$WORK/collector.log" 'Everything is ready. Begin running and processing data.'
}
post() { # post <signal path> <file>: OTLP/HTTP JSON into the Collector
  local code
  code=$(curl -s -o "$WORK/post.out" -w '%{http_code}' -X POST -H 'content-type: application/json' \
    --data-binary @"$2" "$COLLECTOR/v1/$1")
  [[ "$code" == 200 ]] || { cat "$WORK/post.out"; fail "collector refused the OTLP/HTTP $1 post ($code)"; }
}
fetch() { curl -fs "$1" >"$2"; } # fetch <url> <file>
[[ -x "$PROBE" ]] || fail "missing $PROBE (cargo build -p tumult-ingest --features otap-client --example otap_probe)"

echo "[1] start tumultd on a fresh lake (ingest token required on gRPC)"
RUST_LOG=info,tumult_ingest=debug "$BIN/tumultd" serve >"$DLOG" 2>&1 &
DPID=$!
poll "the daemon to come up" curl -fs "$URL/healthz"
pass "daemon healthy"

echo "[2] start the Collector: otlp receiver -> otelarrow exporter (default zstd gRPC compression) -> tumultd, no OTLP downgrade"
start_collector "$KRONIKA_INGEST_TOKEN"
pass "collector $COLLECTOR_TAG ready with the otelarrow exporter on traces, logs and metrics"

echo "[3] send OTLP/HTTP JSON spans with resilience.target.* resource attributes into the Collector"
NOW=$(date +%s%N)
spans() { # spans <trace id> <experiment id> <file>
  cat >"$3" <<EOF
{"resourceSpans":[{"resource":{"attributes":[
  {"key":"service.name","value":{"stringValue":"proof-checkout"}},
  {"key":"service.version","value":{"stringValue":"1.4.2"}},
  {"key":"resilience.target.system","value":{"stringValue":"proof-db"}},
  {"key":"resilience.target.technology","value":{"stringValue":"postgres"}},
  {"key":"resilience.target.environment","value":{"stringValue":"proof"}},
  {"key":"resilience.target.component","value":{"stringValue":"shop/checkout"}}
]},"scopeSpans":[{"scope":{"name":"proof","version":"1"},"spans":[
  {"traceId":"$1","spanId":"0000000000000001","name":"resilience.experiment","kind":1,
   "startTimeUnixNano":"$((NOW - 3000000000))","endTimeUnixNano":"$NOW",
   "attributes":[
     {"key":"resilience.experiment.id","value":{"stringValue":"$2"}},
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
  {"traceId":"$1","spanId":"0000000000000002","parentSpanId":"0000000000000001",
   "name":"resilience.action","kind":1,
   "startTimeUnixNano":"$((NOW - 2900000000))","endTimeUnixNano":"$((NOW - 2500000000))",
   "attributes":[{"key":"resilience.fault.severity","value":{"stringValue":"high"}},
                 {"key":"targets","value":{"arrayValue":{"values":[{"stringValue":"pg-0"},{"intValue":"7"}]}}}],
   "status":{"code":2,"message":"pod not found"}},
  {"traceId":"$1","spanId":"0000000000000003","parentSpanId":"0000000000000001",
   "name":"GET /checkout","kind":2,
   "startTimeUnixNano":"$((NOW - 1000000000))","endTimeUnixNano":"$((NOW - 950000000))",
   "attributes":[{"key":"http.request.method","value":{"stringValue":"GET"}},
                 {"key":"http.response.status_code","value":{"intValue":"200"}}]}
]}]}]}
EOF
}
trace_id() { printf '%016x%016x' "$$" "$((RUN * 16 + $1))"; }
TRACE=$(trace_id 1); EXP="exp-otap-proof-$RUN-$$-a"
TRACE2=$(trace_id 2); EXP2="exp-otap-proof-$RUN-$$-b"
spans "$TRACE" "$EXP" "$WORK/spans.json"
post traces "$WORK/spans.json"
pass "3 spans accepted by the Collector (trace $TRACE)"

# The assertions one decoded trace must satisfy: <file> <experiment id>.
check_trace() {
  py "sp=d['spans']; assert len(sp)==3, sp
root=[x for x in sp if x['span_name']=='resilience.experiment'][0]
assert root['experiment_id']=='$2' and root['experiment_name']=='otap-proof', root
assert root['fault_type']=='termination' and root['status_code']=='Ok' and root['service_name']=='proof-checkout', root
assert root['span_attrs']['resilience.estimate.rationale']=='primary restart is fast', root['span_attrs']
assert root['span_attrs']['retries']=='3' and 'resilience.experiment.id' not in root['span_attrs'], root['span_attrs']
ev=root['events']; assert ev[0]['name']=='fault.injected' and ev[0]['attributes']['action']=='pod-kill', ev
act=[x for x in sp if x['span_name']=='resilience.action'][0]
assert act['parent_span_id']=='0000000000000001' and act['status_code']=='Error' and act['status_message']=='pod not found', act
assert act['span_attrs']['targets']=='[\"pg-0\",\"7\"]', act['span_attrs']
get=[x for x in sp if x['span_name']=='GET /checkout'][0]
assert get['span_kind']=='Server' and get['span_attrs']['http.response.status_code']=='200', get
assert {x['service_name'] for x in sp}=={'proof-checkout'}, sp
print('     spans', [(x['span_name'], x['status_code']) for x in sp])" <"$1"
}
# The promoted resource columns of one experiment: <experiment id>.
check_experiment() {
  fetch "$URL/api/experiments" "$WORK/experiments.json"
  py "e=[x for x in d['experiments'] if x['id']=='$1']; assert len(e)==1, d
e=e[0]; assert (e['target_system'],e['target_technology'],e['target_environment'])==('proof-db','postgres','proof'), e
print('     experiment', e['id'], e['target_system'], e['target_technology'], e['target_environment'])" <"$WORK/experiments.json"
}

echo "[4] the spans arrive in the lake over the Arrow stream"
poll "trace $TRACE" fetch "$URL/api/traces/$TRACE" "$WORK/trace.json"
expect_lines 1 "$DLOG" 'OTAP stream opened' 'signal="traces"'
expect_lines 3 "$DLOG" 'zstdarrow gRPC encoding accepted as zstd'
expect_lines 1 "$DLOG" 'OTAP batch decoded' 'signal="traces"'
expect_lines 1 "$DLOG" 'OTAP batch decoded' 'signal="traces"' ' continued=0'
collector_log
expect_lines 0 "$WORK/collector.log" 'owngrad'
expect_lines 0 "$WORK/collector.log" 'arrow is not supported'
check_trace "$WORK/trace.json" "$EXP"
pass "3 spans decoded from OTel-Arrow (zstdarrow gRPC compression accepted): ids, kinds, status, events, unpromoted attrs"

echo "[5] promoted columns from the resource: target system/technology/environment on the experiment"
check_experiment "$EXP"
fetch "$URL/api/traces?service=proof-checkout" "$WORK/traces.json"
py "t=[x for x in d['traces'] if x['trace_id']=='$TRACE']; assert len(t)==1 and t[0]['span_count']==3 and t[0]['error_count']==1, d" <"$WORK/traces.json"
fetch "$URL/api/topology" "$WORK/topology.json"
py "n={(x['name'],x['type']) for x in d['nodes']}; assert ('proof-checkout','service') in n and ('proof-db','target') in n, n
assert any(e['from_id']=='svc:proof-checkout' and e['to_id']=='tgt:proof-db' for e in d['edges']), d['edges']" <"$WORK/topology.json"
pass "service name, target_* columns, trace listing and topology edge all answer from the lake"

echo "[6] a second batch on the same stream: no schema resent, resource attributes still attributed"
spans "$TRACE2" "$EXP2" "$WORK/spans2.json"
post traces "$WORK/spans2.json"
poll "trace $TRACE2" fetch "$URL/api/traces/$TRACE2" "$WORK/trace2.json"
expect_lines 1 "$DLOG" 'OTAP stream opened' 'signal="traces"'
expect_lines 2 "$DLOG" 'OTAP batch decoded' 'signal="traces"'
expect_lines 1 "$DLOG" 'OTAP batch decoded' 'signal="traces"' ' continued=0'
CONTINUED=$(plain "$DLOG" | python3 -c 'import re, sys
rows = [l for l in sys.stdin if "OTAP batch decoded" in l and "signal=\"traces\"" in l]
payloads, continued = (int(re.search(rf" {k}=(\d+)", rows[1]).group(1)) for k in ("payloads", "continued"))
assert payloads >= 4 and continued == payloads, rows[1]
print(f"{continued}/{payloads}")')
check_trace "$WORK/trace2.json" "$EXP2"
check_experiment "$EXP2"
check_experiment "$EXP"
pass "continuation batch: $CONTINUED tables decoded with the first batch's schemas; service and target_* columns intact"

echo "[7] logs over the Arrow stream"
LOGMARK="otap-proof-log-$RUN-$$"
cat >"$WORK/logs.json" <<EOF
{"resourceLogs":[{"resource":{"attributes":[
  {"key":"service.name","value":{"stringValue":"proof-checkout"}},
  {"key":"deployment.environment","value":{"stringValue":"proof"}}
]},"scopeLogs":[{"scope":{"name":"proof"},"logRecords":[
  {"timeUnixNano":"$((NOW - 2000000000))","severityNumber":17,"severityText":"ERROR",
   "body":{"stringValue":"$LOGMARK payment declined"},
   "traceId":"$TRACE","spanId":"0000000000000002",
   "attributes":[{"key":"experiment_id","value":{"stringValue":"$EXP"}},
                 {"key":"attempt","value":{"intValue":"2"}}]},
  {"timeUnixNano":"$((NOW - 1000000000))","severityNumber":9,"severityText":"INFO",
   "body":{"stringValue":"$LOGMARK retry succeeded"},
   "attributes":[{"key":"experiment_id","value":{"stringValue":"$EXP"}}]}
]}]}]}
EOF
post logs "$WORK/logs.json"
logs_arrived() {
  fetch "$URL/api/logs?service=proof-checkout&q=$LOGMARK" "$WORK/logs.out" && py "assert d['count']==2, d" <"$WORK/logs.out"
}
poll "the 2 log records" logs_arrived
py "new,old=d['logs']
assert (old['severity_text'],old['body'])==('ERROR','$LOGMARK payment declined'), old
assert (old['trace_id'],old['span_id'])==('$TRACE','0000000000000002'), old
assert old['experiment_id']=='$EXP' and old['log_attrs']['attempt']=='2', old
assert (new['severity_text'],new['body'])==('INFO','$LOGMARK retry succeeded'), new
assert {x['service_name'] for x in d['logs']}=={'proof-checkout'}, d
assert all(x['resource_attrs']['deployment.environment']=='proof' for x in d['logs']), d
print('     logs', [(x['severity_text'], x['body']) for x in d['logs']])" <"$WORK/logs.out"
expect_lines 1 "$DLOG" 'OTAP stream opened' 'signal="logs"'
expect_lines 1 "$DLOG" 'OTAP batch decoded' 'signal="logs"'
pass "2 log records decoded from OTel-Arrow: severity, body, trace link, attributes, resource"

echo "[8] metrics over the Arrow stream: a sum, a gauge and a histogram"
M=proof.otap.run$RUN
cat >"$WORK/metrics.json" <<EOF
{"resourceMetrics":[{"resource":{"attributes":[
  {"key":"service.name","value":{"stringValue":"proof-checkout"}}
]},"scopeMetrics":[{"scope":{"name":"proof"},"metrics":[
  {"name":"$M.requests","unit":"1","sum":{"aggregationTemporality":2,"isMonotonic":true,"dataPoints":[
    {"timeUnixNano":"$NOW","startTimeUnixNano":"$((NOW - 60000000000))","asInt":"5",
     "attributes":[{"key":"route","value":{"stringValue":"/checkout"}}]},
    {"timeUnixNano":"$NOW","startTimeUnixNano":"$((NOW - 60000000000))","asInt":"7",
     "attributes":[{"key":"route","value":{"stringValue":"/cart"}}]}]}},
  {"name":"$M.queue_depth","unit":"1","gauge":{"dataPoints":[
    {"timeUnixNano":"$NOW","asDouble":3.5,
     "attributes":[{"key":"queue","value":{"stringValue":"orders"}}]}]}},
  {"name":"$M.latency","unit":"s","histogram":{"aggregationTemporality":2,"dataPoints":[
    {"timeUnixNano":"$NOW","startTimeUnixNano":"$((NOW - 60000000000))",
     "count":"4","sum":10.0,"bucketCounts":["1","2","1"],"explicitBounds":[1.0,5.0],
     "attributes":[{"key":"route","value":{"stringValue":"/checkout"}}]}]}}
]}]}]}
EOF
post metrics "$WORK/metrics.json"
metric() { fetch "$URL/api/metrics/query?name=$M.$1&interval=5m&group_by=$2" "$WORK/metric-$1.json"; }
metrics_arrived() { metric requests route && metric queue_depth queue && metric latency route; }
poll "the 3 metrics" metrics_arrived
py "assert d['type']=='sum', d
got={s['group']: [p['v'] for p in s['points']] for s in d['series']}
assert got=={'/checkout': [5.0], '/cart': [7.0]}, got
print('     sum', got)" <"$WORK/metric-requests.json"
py "assert d['type']=='gauge', d
got={s['group']: [p['v'] for p in s['points']] for s in d['series']}
assert got=={'orders': [3.5]}, got
print('     gauge', got)" <"$WORK/metric-queue_depth.json"
py "assert d['type']=='histogram', d
got={s['group']: [(p['avg'], p['p95']) for p in s['points']] for s in d['series']}
assert list(got)==['/checkout'] and got['/checkout'][0][0]==2.5 and got['/checkout'][0][1]==5.0, got
print('     histogram', got)" <"$WORK/metric-latency.json"
expect_lines 1 "$DLOG" 'OTAP stream opened' 'signal="metrics"'
expect_lines 1 "$DLOG" 'OTAP batch decoded' 'signal="metrics"'
pass "sum, gauge and histogram points decoded from OTel-Arrow with their attributes"
expect_lines 0 "$DLOG" 'OTAP batch rejected'
expect_lines 0 "$DLOG" 'OTAP batch undecodable'
expect_lines 0 "$DLOG" 'ingest bearer token rejected'
collector_log
expect_lines 0 "$WORK/collector.log" 'owngrad'
remove_container

echo "[9] hostile clients get the precise status"
probe() { # probe <name> <case> [token]: output in $WORK/probe-<name>.out
  "$PROBE" "http://127.0.0.1:$GRPC_PORT" "${@:2}" >"$WORK/probe-$1.out" 2>"$WORK/probe-$1.err" \
    || { cat "$WORK/probe-$1.err"; fail "probe $1 could not run"; }
}
expect_output() { # expect_output <name> <exact expected output>
  [[ "$(cat "$WORK/probe-$1.out")" == "$2" ]] \
    || { echo "--- expected"; echo "$2"; echo "--- got"; cat "$WORK/probe-$1.out"; fail "probe $1"; }
}
probe missing-token no-batch
expect_output missing-token 'open: Unauthenticated: missing or invalid bearer token'
expect_lines 1 "$DLOG" 'WARN' 'ingest bearer token rejected' 'presented=false'
pass "no token: UNAUTHENTICATED, logged at WARN"
probe wrong-token no-batch "not-$KRONIKA_INGEST_TOKEN"
expect_output wrong-token 'open: Unauthenticated: missing or invalid bearer token'
expect_lines 1 "$DLOG" 'WARN' 'ingest bearer token rejected' 'presented=true'
expect_lines 0 "$DLOG" "not-$KRONIKA_INGEST_TOKEN"
expect_lines 0 "$DLOG" "$KRONIKA_INGEST_TOKEN"
pass "wrong token: UNAUTHENTICATED, logged at WARN without the token"
probe accepted no-batch "$KRONIKA_INGEST_TOKEN"
expect_output accepted 'closed: clean'
pass "right token, no batches: clean close"
probe malformed malformed "$KRONIKA_INGEST_TOKEN"
expect_output malformed 'ack: 77 INVALID_ARGUMENT malformed SPANS payload: missing continuation marker
closed: InvalidArgument: otel-arrow decode: malformed SPANS payload: missing continuation marker'
expect_lines 1 "$DLOG" 'WARN' 'OTAP batch undecodable, closing the stream' 'batch_id=77'
pass "undecodable payload: batch status INVALID_ARGUMENT, then the stream ends INVALID_ARGUMENT"
probe oversized oversized "$KRONIKA_INGEST_TOKEN"
expect_output oversized 'closed: OutOfRange: Error, decoded message length too large: found 4194326 bytes, the limit is: 4194304 bytes'
expect_lines 1 "$DLOG" 'WARN' 'OTAP stream read failed' 'decoded message length too large'
pass "message over KRONIKA_OTAP_MAX_MESSAGE_BYTES: stream ends OUT_OF_RANGE"
kill -0 "$DPID" || fail "the daemon did not survive the hostile clients"
fetch "$URL/api/traces?service=proof-checkout" "$WORK/traces.json"
py "assert sorted(x['trace_id'] for x in d['traces'])==sorted(['$TRACE','$TRACE2']), d" <"$WORK/traces.json"
pass "daemon still serving; the lake holds exactly the two proof traces"

echo "[10] a Collector with a wrong ingest token is refused on the Arrow stream (nothing silently dropped)"
start_collector "wrong-$RUN"
TRACE3=$(trace_id 3)
spans "$TRACE3" "exp-otap-proof-$RUN-$$-c" "$WORK/spans3.json"
post traces "$WORK/spans3.json"
# The exporter keeps reopening its streams, so the counts only grow: one
# rejection is the probe's from step 9, the rest are the Collector's.
daemon_refused() { [[ "$(lines "$DLOG" 'WARN' 'ingest bearer token rejected' 'presented=true')" -ge 2 ]]; }
poll "the daemon to reject the collector's token" daemon_refused
collector_refused() { # gRPC code 16 is UNAUTHENTICATED
  collector_log && [[ "$(lines "$WORK/collector.log" 'arrow stream error' '"otelcol.signal": "traces"' \
    '"code": 16, "message": "missing or invalid bearer token"')" -ge 1 ]]
}
poll "the collector to report the refusal" collector_refused
expect_lines 0 "$DLOG" "wrong-$RUN"
code=$(curl -s -o /dev/null -w '%{http_code}' "$URL/api/traces/$TRACE3")
[[ "$code" == 404 ]] || fail "spans sent with a wrong token must not land (got $code)"
expect_lines 2 "$DLOG" 'OTAP batch decoded' 'signal="traces"'
pass "UNAUTHENTICATED reported to the exporter; lake untouched"
echo "ALL OTEL-ARROW PROOF CHECKS PASSED (run $RUN)"
