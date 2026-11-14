#!/usr/bin/env bash
# Entity events live proof: a real OpenTelemetry Collector (contrib 0.165.0)
# whose host_metrics receiver reports the `host` entity every ten seconds,
# and a fresh tumultd per phase. No mocks. Exit non-zero on any failed
# expectation. Run it three times in a row before a release.
#
#   phase off      KRONIKA_ENTITY_EVENTS unset: the records are stored in
#                  `logs` as before and no entity row appears.
#   phase legacy   flag on, Collector without the feature gate, OTLP/gRPC:
#                  one `host` entity, `live`, wire form `legacy`, exactly one
#                  row after several heartbeats, nothing in `logs`; the
#                  Collector is stopped and the entity is `expired` after
#                  three report intervals.
#   phase spec     the same with
#                  --feature-gates=pkg.experimentalmetricmetadata.useEntityEventsSpecification:
#                  wire form `spec`.
#   phase arrow    flag on, feature gate on, OTel-Arrow instead of OTLP: the
#                  same entity over the other log carrier.
#   phase probe    flag on, no Collector: the entity_probe example sends the
#                  events one quiet host never produces, one export each,
#                  gzip-compressed. A changed state opens a second row; a
#                  delete makes the entity `gone`; a state after it brings
#                  the entity back in a third row; a description with
#                  labels, an annotation holding a manifest, a container
#                  label and a nested map comes back with the allow-listed
#                  keys only (KRONIKA_ENTITY_LABEL_ALLOW=app) and the
#                  manifest's secret is in no answer and not in the lake
#                  file; a description over the size limit is counted as
#                  dropped and leaves no entity.
#
#   cargo build -p tumultd && (cd web && npm ci && npm run build)
#   cargo build -p tumult-ingest --example entity_probe
#   bash scripts/proof/entity-events/proof.sh [run-number]
#
# Needs docker and the otel/opentelemetry-collector-contrib image pinned
# below by digest (COLLECTOR_IMAGE). The Collector runs on the host network
# so it reaches the loopback-bound daemon; apart from that it gets a
# read-only root, no capabilities, no new privileges and an unprivileged
# user. Its `host.id` is a machine id written for the run, so the entity key
# is known in advance; the host name and OS type it detects are real and end
# up in the run's lake under $PROOF_WORK. Two free
# loopback ports are chosen per phase, and before each phase goes on the
# proof asks ss whether the daemon it started is the listener on both. Work
# files and the lakes land under $PROOF_WORK/run-N (default: a temp dir),
# which must not be inside the repository. No step waits a fixed time:
# every wait polls for its condition.
set -euo pipefail
S="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$S/../../.." && pwd)"
PROFILE=${PROOF_PROFILE:-debug}
BIN=${TUMULT_BIN_DIR:-${CARGO_TARGET_DIR:-$REPO/target}/$PROFILE}
PROBE=$BIN/examples/entity_probe
RUN=${1:-1}
[[ "$RUN" =~ ^[0-9]{1,6}$ ]] || { echo "run number must be an integer, got '$RUN'" >&2; exit 2; }
for tool in python3 curl ss docker; do
  command -v "$tool" >/dev/null 2>&1 || { echo "this proof needs $tool" >&2; exit 2; }
done
BASE=${PROOF_WORK:-$(mktemp -d)}
[[ "$BASE" == /* ]] || { echo "PROOF_WORK must be an absolute path, got '$BASE'" >&2; exit 2; }
WORK=$BASE/run-$RUN; rm -rf -- "$WORK"; mkdir -p "$WORK"
WORK=$(cd "$WORK" && pwd -P)
case "$WORK/" in
  "$(cd "$REPO" && pwd -P)"/*) echo "PROOF_WORK ($WORK) is inside the repository; the proof writes nothing there" >&2; exit 2 ;;
esac
# otel/opentelemetry-collector-contrib:0.165.0
COLLECTOR_IMAGE=${COLLECTOR_IMAGE:-otel/opentelemetry-collector-contrib@sha256:e40d61f10c9a617cca1ffd1be4d7e2b3a4b0f0001798ff07e58cabe82e618cea}
GATE=pkg.experimentalmetricmetadata.useEntityEventsSpecification
# The Collector's metadata_collection_interval (collector.yaml), seconds.
INTERVAL=10
export KRONIKA_METRICS_DIR=$REPO/metrics
export KRONIKA_INGEST_TOKEN=proof-entity-token-$RUN
unset KRONIKA_ENTITY_EVENTS KRONIKA_ENTITY_LABEL_ALLOW KRONIKA_ENTITY_ATTRIBUTE_ALLOW
unset KRONIKA_ENTITY_MAX_PER_ENVIRONMENT
# The machine id the Collector's resourcedetection reads as `host.id`.
HOST_ID=$(printf '%016x%016x' "$$" "$RUN")
printf '%s\n' "$HOST_ID" >"$WORK/machine-id"
ENTITY_KEY="host|host.id=$HOST_ID"
CONTAINER=entity-proof-$RUN-$$
DPID=""; PHASE=""; DLOG=""; URL=""

pass() { echo "  ok   $1"; }
fail() { echo "  FAIL $1"; exit 1; }
# py <python> [argument...]: run <python> with the JSON on stdin as `d` and
# the arguments as `a`.
py() {
  local code=$1
  shift
  python3 -c "import json, sys
a = sys.argv[1:]
d = json.load(sys.stdin)
$code" "$@"
}
now_ms() { date +%s%3N; }
# await <what> <seconds> <command...>: poll every 200 ms until the command
# succeeds; fail the proof when it has not within the time given.
await() {
  local what=$1 deadline=$(( $(now_ms) + $2 * 1000 )); shift 2
  until "$@" >/dev/null 2>&1; do
    [[ $(now_ms) -lt $deadline ]] || { [[ -z "$DLOG" ]] || tail -20 "$DLOG"; fail "timed out waiting for $what"; }
    sleep 0.2
  done
}
free_ports() { # free_ports <n>: n distinct loopback ports nothing listens on
  python3 -c 'import socket, sys
held = [socket.socket() for _ in range(int(sys.argv[1]))]
for s in held: s.bind(("127.0.0.1", 0))
print(*[s.getsockname()[1] for s in held])' "$1"
}
# The listener on port $1 is the daemon this proof started.
listener_is_ours() {
  local held
  held=$(ss -ltnpH "sport = :$1")
  [[ -n "$held" && "$held" == *"pid=$DPID,"* ]]
}
# The daemon of this phase is running, owns both its ports and answers.
alive() {
  kill -0 "$DPID" 2>/dev/null || { tail -20 "$DLOG"; fail "the daemon is gone ($1)"; }
  listener_is_ours "$GRPC_PORT" || fail "port $GRPC_PORT is not the proof's daemon ($1)"
  listener_is_ours "$HTTP_PORT" || fail "port $HTTP_PORT is not the proof's daemon ($1)"
  curl -fs -m 10 "$URL/healthz" >/dev/null || { tail -20 "$DLOG"; fail "the daemon does not answer ($1)"; }
}
remove_container() {
  if docker container inspect "$CONTAINER" >/dev/null 2>&1; then docker rm -f "$CONTAINER" >/dev/null; fi
}
stop_daemon() {
  [[ -n "$DPID" ]] || return 0
  kill "$DPID" 2>/dev/null || true # already gone is fine here
  wait "$DPID" 2>/dev/null || true
  DPID=""
}
# Nothing in here may end the script: under `set -e` a failing command in
# the EXIT trap would replace the exit code of the proof.
cleanup() {
  remove_container || true
  stop_daemon
}
trap cleanup EXIT

# start_daemon <phase> <on|off> [NAME=value...]: a fresh lake and two fresh
# ports; the flag is set only for `on`, and the daemon gets the variables
# named after it.
start_daemon() {
  PHASE=$1
  mkdir -p "$WORK/$PHASE"
  DLOG=$WORK/$PHASE/tumultd.log
  read -r GRPC_PORT HTTP_PORT < <(free_ports 2)
  URL=http://127.0.0.1:$HTTP_PORT
  local flag=("${@:3}")
  [[ "$2" == off ]] || flag+=(KRONIKA_ENTITY_EVENTS=1)
  env "${flag[@]}" \
    TUMULT_LAKE_PATH="$WORK/$PHASE/lake.duckdb" \
    KRONIKA_OTLP_GRPC_ADDR="127.0.0.1:$GRPC_PORT" \
    KRONIKA_OTLP_HTTP_ADDR="127.0.0.1:$HTTP_PORT" \
    TUMULT_LOG_FORMAT=text RUST_LOG=info,tumult_ingest=debug \
    "$BIN/tumultd" serve >"$DLOG" 2>&1 &
  DPID=$!
  await "the daemon to come up" 60 curl -fs "$URL/healthz"
  alive "after the health check"
  pass "daemon healthy: pid $DPID, gRPC $GRPC_PORT, HTTP $HTTP_PORT, entity events $2"
}
collector_log() { docker logs "$CONTAINER" >"$WORK/$PHASE/collector.log" 2>&1; }
collector_ready() {
  collector_log && grep -q 'Everything is ready. Begin running and processing data.' "$WORK/$PHASE/collector.log"
}
# start_collector <exporter> [collector argument...]
start_collector() {
  local exporter=$1; shift
  remove_container
  docker run -d --name "$CONTAINER" \
    --network host \
    --read-only --cap-drop ALL --security-opt no-new-privileges --user 10001:10001 \
    -v "$S/collector.yaml:/etc/otelcol-contrib/config.yaml:ro" \
    -v "$WORK/machine-id:/etc/machine-id:ro" \
    -e "KRONIKA_INGEST_TOKEN=$KRONIKA_INGEST_TOKEN" -e "PROOF_GRPC_PORT=$GRPC_PORT" \
    -e "PROOF_EXPORTER=$exporter" \
    "$COLLECTOR_IMAGE" \
    --config=/etc/otelcol-contrib/config.yaml "$@" >/dev/null
  await "the collector to start" 60 collector_ready
}
# stop_collector: end the Collector at once and keep its log.
stop_collector() {
  collector_log || true
  remove_container
}
fetch() { curl -fs -m 10 "$URL$1" >"$WORK/$PHASE/$2"; } # fetch <path> <file>
# counter <name> <label value>: the value of one labelled daemon counter.
counter() {
  curl -fs -m 10 "$URL/metrics" | python3 -c 'import sys
want = sys.argv[1] + "{"
for line in sys.stdin:
    if line.startswith(want) and "\"" + sys.argv[2] + "\"" in line:
        print(line.split()[-1]); break
else:
    sys.exit("no such counter")' "$1" "$2"
}
counter_at_least() { [[ "$(counter "$1" "$2")" -ge "$3" ]]; }
entity_count_is() {
  fetch "/api/entities$2" entities.json && py 'assert d["count"] == int(a[0]), d' "$1" <"$WORK/$PHASE/entities.json"
}
# plain_counter <name>: the value of a daemon counter without labels.
plain_counter() {
  curl -fs -m 10 "$URL/metrics" | python3 -c 'import sys
for line in sys.stdin:
    if line.startswith(sys.argv[1] + " "):
        print(line.split()[-1]); break
else:
    sys.exit("no such counter")' "$1"
}
no_drops() {
  curl -fs -m 10 "$URL/metrics" | python3 -c 'import sys
rows = [l.split() for l in sys.stdin if l.startswith("tumultd_entity_events_dropped_total{")]
assert rows and all(value == "0" for _, value in rows), rows'
}

# The assertions of a phase with the flag on: <wire form>.
check_entity() {
  local form=$1 beats
  await "the host entity" 60 entity_count_is 1 ""
  py 'e = d["entities"][0]
assert e["entity_key"] == a[0] and e["entity_type"] == "host", e
assert e["entity_id"] == {"host.id": a[1]}, e
assert e["liveness"] == "live" and e["wire_form"] == a[2], e
assert e["interval_s"] == int(a[3]) and e["observer"] == "token:default", e
assert e["deleted_at_ns"] is None and isinstance(e["description"], dict), e
print("     entity", e["entity_key"], e["liveness"], e["wire_form"], sorted(e["description"]))' \
    "$ENTITY_KEY" "$HOST_ID" "$form" "$INTERVAL" <"$WORK/$PHASE/entities.json"
  pass "one host entity, live, wire form $form, report interval ${INTERVAL}s, identity from the resource's host.id"

  await "three more heartbeats" 90 counter_at_least tumultd_entity_events_total refreshed 3
  alive "after the heartbeats"
  beats=$(counter tumultd_entity_events_total refreshed)
  [[ "$(counter tumultd_entity_events_total opened)" == 1 ]] || fail "more than one row was opened"
  no_drops || fail "an entity event was dropped"
  entity_count_is 1 "" || fail "the heartbeats changed the number of entities"
  fetch "/api/entities/host%7Chost.id=$HOST_ID/history" history.json
  py 'assert d["entity_key"] == a[0] and d["count"] == 1, d
v = d["versions"][0]
kept = (v["last_seen_ns"] - v["valid_from_ns"]) / 1e9
assert kept >= 2 * int(a[1]), (kept, v)
assert v["wire_form"] == a[2], v
print(f"     one version, first seen {kept:.0f}s before it was last seen")' \
    "$ENTITY_KEY" "$INTERVAL" "$form" <"$WORK/$PHASE/history.json"
  pass "exactly one row after $beats heartbeats (1 opened, $beats refreshed, none dropped)"

  fetch "/api/logs?range=24h&attr=host.id=$HOST_ID" logs.json
  py 'assert d["count"] == 0, d' <"$WORK/$PHASE/logs.json"
  pass "no record of the Collector's resource was stored in logs"
}

# The entity of this phase is `expired` once three intervals have passed
# since it was last heard of, and not before.
check_expiry() {
  stop_collector
  entity_count_is 1 "?liveness=live" || fail "the entity must still be live right after the Collector stopped"
  expired() { entity_count_is 1 "?liveness=expired"; }
  await "the entity to expire" $((3 * INTERVAL + 30)) expired
  py 'import time
e = d["entities"][0]
silent = time.time() - e["last_seen_ns"] / 1e9
assert e["entity_key"] == a[0] and e["liveness"] == "expired", e
assert silent > 3 * int(a[1]), (silent, e)
print(f"     expired {silent:.0f}s after it was last heard of")' "$ENTITY_KEY" "$INTERVAL" <"$WORK/$PHASE/entities.json"
  entity_count_is 0 "?liveness=live" || fail "an expired entity is still listed as live"
  fetch "/api/entities/host%7Chost.id=$HOST_ID/history" history.json
  py 'assert d["count"] == 1 and d["versions"][0]["deleted_at_ns"] is None, d' <"$WORK/$PHASE/history.json"
  alive "after the expiry"
  pass "Collector stopped: the entity is expired after three intervals, still one row, not deleted"
}

# probe <case> <pod uid>: one hand-built entity event, sent the way a
# Collector sends its own (OTLP/gRPC, gzip, the ingest token).
probe() {
  "$PROBE" "http://127.0.0.1:$GRPC_PORT" "$1" "$2" "$KRONIKA_INGEST_TOKEN" >>"$WORK/$PHASE/probe.log" 2>&1 \
    || { tail -5 "$WORK/$PHASE/probe.log"; fail "the probe could not send '$1'"; }
}
# pod_history <rows>: the pod of the probe phase has that many stored rows.
pod_history() {
  fetch "/api/entities/$POD_PATH/history" history.json && py 'assert d["total"] == int(a[0]), d' "$1" <"$WORK/$PHASE/history.json"
}
# The outcome and drop counters of the probe phase are exactly these.
probe_counters() { # <opened> <tombstoned> <oversized>
  [[ "$(counter tumultd_entity_events_total opened)" == "$1" ]] || fail "expected $1 rows opened"
  [[ "$(counter tumultd_entity_events_total tombstoned)" == "$2" ]] || fail "expected $2 deletes applied"
  [[ "$(counter tumultd_entity_events_dropped_total oversized)" == "$3" ]] || fail "expected $3 oversized drops"
  alive "$4"
}

[[ -x "$BIN/tumultd" ]] || fail "missing $BIN/tumultd (cargo build -p tumultd)"
[[ -x "$PROBE" ]] || fail "missing $PROBE (cargo build -p tumult-ingest --example entity_probe)"

echo "[1] flag off: entity events are stored as the log records they are"
start_daemon off off
start_collector otlp/tumultd
entity_logs() {
  fetch "/api/logs?range=24h&attr=host.id=$HOST_ID" logs.json && py 'assert d["count"] >= 2, d' <"$WORK/$PHASE/logs.json"
}
await "two entity events in logs" 60 entity_logs
py 'for l in d["logs"]:
    assert l["log_attrs"]["otel.entity.event.type"] == "entity_state", l
    assert l["log_attrs"]["otel.entity.type"] == "host", l
    assert l["resource_attrs"]["host.id"] == a[0], l
print("     logs", d["count"], "records with otel.entity.event.type=entity_state")' "$HOST_ID" <"$WORK/$PHASE/logs.json"
entity_count_is 0 "" || fail "an entity row appeared with the flag off"
[[ "$(counter tumultd_entity_events_total opened)" == 0 ]] || fail "an entity event was applied with the flag off"
no_drops || fail "an entity event was counted as dropped with the flag off"
alive "at the end of the phase"
stop_collector
stop_daemon
pass "flag off: the records are in logs, no entity row, no entity counter moved"

echo "[2] flag on, legacy wire form (no feature gate), OTLP/gRPC"
start_daemon legacy on
start_collector otlp/tumultd
check_entity legacy
check_expiry
stop_daemon

echo "[3] flag on, specification wire form (feature gate), OTLP/gRPC"
start_daemon spec on
start_collector otlp/tumultd "--feature-gates=$GATE"
check_entity spec
check_expiry
stop_daemon

echo "[4] flag on, specification wire form, OTel-Arrow"
start_daemon arrow on
start_collector otelarrow/tumultd "--feature-gates=$GATE"
check_entity spec
grep -q 'OTAP stream opened' "$DLOG" || fail "the entity events did not arrive over an OTel-Arrow stream"
collector_log
if grep -q 'arrow is not supported' "$WORK/$PHASE/collector.log"; then fail "the Collector fell back to OTLP"; fi
alive "at the end"
stop_collector
stop_daemon
pass "the same entity over the OTel-Arrow log stream"

echo "[5] flag on, hand-built events over OTLP/gRPC with gzip: change, delete, return, filter, limit"
start_daemon probe on KRONIKA_ENTITY_LABEL_ALLOW=app
POD=proof-pod-$RUN
POD_KEY="k8s.pod|k8s.pod.uid=$POD"
POD_PATH="k8s.pod%7Ck8s.pod.uid%3D$POD"
# What the manifest in the probe's annotation holds (entity_probe.rs).
SECRET=proof-secret

probe state:Pending "$POD"
await "the pod entity" 30 entity_count_is 1 "?liveness=live"
py 'e = d["entities"][0]
assert e["entity_key"] == a[0] and e["entity_type"] == "k8s.pod" and e["wire_form"] == "spec", e
assert e["description"] == {"k8s.pod.name": "probe", "k8s.pod.phase": "Pending"}, e
assert e["interval_s"] == 600 and e["observer"] == "token:default", e' "$POD_KEY" <"$WORK/$PHASE/entities.json"
pod_history 1 || fail "a first state must be one row"
probe_counters 1 0 0 "after the first state"
pass "a state sent with gzip opened the entity: live, phase Pending, one row"

probe state:Running "$POD"
await "the second row" 30 pod_history 2
py 'new, old = d["versions"]
assert new["description"]["k8s.pod.phase"] == "Running", new
assert old["description"]["k8s.pod.phase"] == "Pending", old
assert new["valid_from_ns"] > old["valid_from_ns"], d
assert new["deleted_at_ns"] is None and old["deleted_at_ns"] is None, d' <"$WORK/$PHASE/history.json"
entity_count_is 1 "" || fail "a changed state must not add an entity"
py 'assert d["entities"][0]["description"]["k8s.pod.phase"] == "Running", d' <"$WORK/$PHASE/entities.json"
probe_counters 2 0 0 "after the changed state"
pass "a changed state opened a second row; the first is kept as it was (phase Pending)"

probe delete "$POD"
await "the entity to be gone" 30 entity_count_is 1 "?liveness=gone"
py 'e = d["entities"][0]
assert e["liveness"] == "gone" and e["deleted_at_ns"] is not None and e["delete_reason"] == "probe", e' <"$WORK/$PHASE/entities.json"
entity_count_is 0 "?liveness=live" || fail "a deleted entity is still listed as live"
pod_history 2 || fail "a delete must mark the latest row, not add one"
probe_counters 2 1 0 "after the delete"
pass "a delete made the entity gone (reason probe), still two rows"

probe state:Running "$POD"
await "the entity to be back" 30 entity_count_is 1 "?liveness=live"
py 'e = d["entities"][0]
assert e["deleted_at_ns"] is None and e["delete_reason"] is None, e' <"$WORK/$PHASE/entities.json"
entity_count_is 0 "?liveness=gone" || fail "the entity is back and still listed as gone"
pod_history 3 || fail "a state after a delete must open a new row"
py 'back, deleted, first = d["versions"]
assert back["deleted_at_ns"] is None and back["valid_from_ns"] > deleted["deleted_at_ns"], d
assert deleted["delete_reason"] == "probe" and first["deleted_at_ns"] is None, d' <"$WORK/$PHASE/history.json"
probe_counters 3 1 0 "after the return"
pass "a state after the delete brought the entity back in a third row; the deleted row keeps its mark"

[[ "$(plain_counter tumultd_entity_event_keys_filtered_total)" == 0 ]] || fail "a key was filtered before any label was sent"
probe labelled "$POD"
await "the labelled state" 30 pod_history 4
fetch "/api/entities" entities.json
py 'e = d["entities"][0]
assert e["description"] == {"k8s.pod.name": "probe", "k8s.pod.phase": "Running", "k8s.pod.label.app": "checkout"}, e
print("     description", sorted(e["description"]))' <"$WORK/$PHASE/entities.json"
if grep -q -e "$SECRET" -e annotation -e payments -e maintainer "$WORK/$PHASE/entities.json" "$WORK/$PHASE/history.json"; then
  fail "a key that is not allow-listed came back from the API"
fi
[[ "$(plain_counter tumultd_entity_event_keys_filtered_total)" == 4 ]] || fail "expected four filtered keys (a label, the annotation, a container label, a nested map)"
probe_counters 4 1 0 "after the labelled state"
pass "labels and annotation sent: only the allow-listed label came back, four keys counted as filtered"

probe oversized "$POD-big"
await "the oversized drop" 30 counter_at_least tumultd_entity_events_dropped_total oversized 1
probe_counters 4 1 1 "after the oversized state"
entity_count_is 1 "" || fail "an oversized state left an entity behind"
py 'assert d["total"] == 1 and d["truncated"] is False and d["next_cursor"] is None, d' <"$WORK/$PHASE/entities.json"
for reason in unknown_kind no_type no_identity malformed export_limit; do
  [[ "$(counter tumultd_entity_events_dropped_total "$reason")" == 0 ]] || fail "an event was dropped as $reason"
done
fetch "/api/logs?range=24h&attr=k8s.pod.uid=$POD-big" logs.json
py 'assert d["count"] == 0, d' <"$WORK/$PHASE/logs.json"
pass "a description over 64 KiB: counted as dropped (oversized), no entity, not stored as a log"
stop_daemon
if grep -q -a -e "$SECRET" "$WORK/$PHASE"/lake.duckdb*; then fail "the manifest's secret is in the lake file"; fi
pass "the annotation's content is not in the lake file"

echo "ALL ENTITY EVENT PROOF CHECKS PASSED (run $RUN, $PROFILE build)"
