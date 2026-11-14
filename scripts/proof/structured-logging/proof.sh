#!/usr/bin/env bash
# Structured-logging live proof: a real tumultd on a private lake and free
# loopback ports, driven over HTTP. No mocks. Exit non-zero on any failed
# expectation. Run it three times in a row before a release.
#
#   bash scripts/proof/structured-logging/proof.sh [run-number]
#
# The script builds tumultd and tumult first (memory-guarded when
# systemd-run is available); PROOF_SKIP_BUILD=1 uses the binaries already in
# TUMULT_BIN_DIR (default: $CARGO_TARGET_DIR/debug or target/debug).
# tumultd embeds web/build, so `npm --prefix web ci && npm --prefix web run
# build` must have run once.
#
# What it proves:
#   1. with RUST_LOG unset and no terminal, every stderr line of the daemon
#      is one JSON object with the required keys, and stdout has no log line;
#   2. a failed login is a `login failed` event with structured fields and
#      without the password;
#   3. a field value holding a newline stays on one line;
#   4. the daemon's events arrive as OTLP log records with
#      service.name=tumultd;
#   5. an event logged inside an OpenTelemetry span carries that span's
#      trace id, on the local line and on the OTLP record;
#   6. TUMULT_LOG_FORMAT=text gives text.
#
# Notes on 4 and 5. tumultd always exports its own telemetry to its own
# OTLP/gRPC ingest listener (it has no setting that points it elsewhere), so
# the receiver in 4 is that listener and the records are read back through
# GET /api/logs. The daemon's HTTP API opens no request spans yet (tumult-api
# is converted in structured-logging phase 2), so 5 uses the span that
# exists today: `tumult run` logs `experiment.completed` inside the
# `resilience.experiment` span and exports it to the same listener.
#
# Nothing here touches ~/.tumult: the lake is in the work directory and HOME
# is redirected for the child processes. Work files land under $PROOF_WORK
# (default: a temp dir, removed on success).
set -euo pipefail
S="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$S/../../.." && pwd)"
RUN=${1:-1}
TARGET_DIR=${CARGO_TARGET_DIR:-$REPO/target}
BIN=${TUMULT_BIN_DIR:-$TARGET_DIR/debug}
KEEP_WORK=${PROOF_WORK:+1}
WORK_ROOT=${PROOF_WORK:-$(mktemp -d)}
WORK=$WORK_ROOT/run-$RUN
rm -rf "$WORK"
mkdir -p "$WORK/home"

DPID=""
PASSWORD="proof-pw-$RANDOM-$RANDOM-not-in-any-log"
NEWLINE_USER=$'proof-line-one\nlevel=ERROR forged-line'

pass() { echo "  ok   $1"; }
fail() {
  echo "  FAIL $1"
  exit 1
}

# Stops the daemon started by this script and verifies it is gone.
stop_daemon() {
  [[ -n "$DPID" ]] || return 0
  local pid=$DPID
  DPID=""
  kill "$pid" 2>/dev/null || true
  for _ in $(seq 1 100); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
  done
  if kill -0 "$pid" 2>/dev/null; then
    kill -9 "$pid" 2>/dev/null || true
    sleep 0.2
  fi
  wait "$pid" 2>/dev/null || true
  if kill -0 "$pid" 2>/dev/null; then
    echo "  FAIL daemon $pid is still alive"
    return 1
  fi
}

cleanup() {
  local status=$?
  stop_daemon || status=1
  if [[ $status -eq 0 && -z "$KEEP_WORK" ]]; then
    rm -rf "$WORK_ROOT"
  else
    echo "work files kept in $WORK"
  fi
  exit "$status"
}
trap cleanup EXIT

free_port() {
  python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'
}

# Starts tumultd on fresh loopback ports. $1 names the output files; the
# remaining arguments are extra environment assignments.
start_daemon() {
  local tag=$1
  shift
  GRPC_PORT=$(free_port)
  HTTP_PORT=$(free_port)
  URL=http://127.0.0.1:$HTTP_PORT
  env -u RUST_LOG -u TUMULT_LOG_FORMAT -u OTEL_SERVICE_NAME -u NO_COLOR \
    -u KRONIKA_DB -u KRONIKA_INGEST_TOKEN -u KRONIKA_TLS_CERT -u KRONIKA_TLS_KEY \
    -u KRONIKA_BOOTSTRAP_ADMIN_PASSWORD -u KRONIKA_BOOTSTRAP_TOKEN \
    HOME="$WORK/home" \
    TUMULT_LAKE_PATH="$WORK/lake.duckdb" \
    KRONIKA_OTLP_GRPC_ADDR="127.0.0.1:$GRPC_PORT" \
    KRONIKA_OTLP_HTTP_ADDR="127.0.0.1:$HTTP_PORT" \
    KRONIKA_METRICS_DIR="$REPO/metrics" \
    "$@" \
    "$BIN/tumultd" serve >"$WORK/$tag.stdout" 2>"$WORK/$tag.stderr" </dev/null &
  DPID=$!
  for _ in $(seq 1 120); do
    kill -0 "$DPID" 2>/dev/null || break
    curl -fs "$URL/healthz" >/dev/null 2>&1 && break
    sleep 0.25
  done
  curl -fs "$URL/healthz" >/dev/null || {
    cat "$WORK/$tag.stderr"
    fail "daemon did not come up"
  }
  assert_own_daemon_listens "$HTTP_PORT"
  assert_own_daemon_listens "$GRPC_PORT"
}

# The process answering on port $1 is the daemon this script started.
assert_own_daemon_listens() {
  local owner
  owner=$(ss -Hltnp "sport = :$1" 2>/dev/null | grep -o 'pid=[0-9]*' | sort -u | tr '\n' ' ')
  [[ "$owner" == "pid=$DPID " ]] || fail "port $1 is served by '${owner}', expected pid=$DPID"
}

login() {
  python3 - "$URL" "$1" "$2" <<'PY'
import json, sys, urllib.error, urllib.request

url, username, password = sys.argv[1:4]
body = json.dumps({"username": username, "password": password}).encode()
request = urllib.request.Request(
    url + "/api/auth/login", data=body, headers={"content-type": "application/json"}
)
try:
    urllib.request.urlopen(request, timeout=30)
except urllib.error.HTTPError as error:
    print(error.code)
else:
    print(200)
PY
}

echo "[0] build"
if [[ -z "${PROOF_SKIP_BUILD:-}" ]]; then
  guard=()
  command -v systemd-run >/dev/null && guard=(systemd-run --user --scope -q -p MemoryMax=16G)
  (cd "$REPO" && CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-6} CARGO_TARGET_DIR=$TARGET_DIR \
    "${guard[@]}" cargo build -p tumultd -p tumult-cli) || fail "build failed"
fi
[[ -x "$BIN/tumultd" && -x "$BIN/tumult" ]] || fail "tumultd and tumult are not in $BIN"
pass "binaries in $BIN"

echo "[1] start tumultd: private lake, free ports, RUST_LOG unset, stderr captured"
start_daemon json
pass "daemon $DPID answers on $HTTP_PORT (http) and $GRPC_PORT (grpc)"

echo "[2] failed logins: a plain one, and one whose user name holds a newline"
code=$(login proof-user "$PASSWORD")
[[ "$code" == 401 ]] || fail "login with a wrong password returned $code, expected 401"
code=$(login "$NEWLINE_USER" "$PASSWORD")
[[ "$code" == 401 ]] || fail "login with a newline user name returned $code, expected 401"
pass "both rejected with 401"

echo "[3] a run through the CLI: an event inside the experiment span"
cat >"$WORK/mini.toon" <<'TOON'
title: structured logging proof
description: one probe and one action, to log inside the experiment span

steady_state_hypothesis:
  title: probe answers ok
  probes[1]:
    - name: health
      activity_type: probe
      provider:
        type: process
        path: sh
        arguments[2]: "-c", "echo ok"
        timeout_s: 5.0
      tolerance:
        type: regex
        pattern: "ok"

method[1]:
  - name: noop
    activity_type: action
    provider:
      type: process
      path: sh
      arguments[2]: "-c", "echo done"
      timeout_s: 5.0

rollbacks[0]:
TOON
(cd "$WORK" && env -u TUMULT_LOG_FORMAT -u OTEL_SERVICE_NAME -u TUMULT_DAEMON_URL \
  HOME="$WORK/home" TUMULT_LAKE_PATH="$WORK/unused-cli-lake.duckdb" \
  RUST_LOG=info TUMULT_OTEL_ENABLED=true \
  OTEL_EXPORTER_OTLP_ENDPOINT="http://127.0.0.1:$GRPC_PORT" \
  "$BIN/tumult" run mini.toon --journal-path "$WORK/mini-journal.toon" --force --no-ingest \
  >"$WORK/cli.stdout" 2>"$WORK/cli.stderr" </dev/null) || {
  cat "$WORK/cli.stderr"
  fail "tumult run failed"
}
pass "experiment ran"

echo "[4] OTLP: the daemon's own events and the CLI's event arrive as log records"
python3 - "$URL" "$WORK" <<'PY' || fail "OTLP log records did not arrive as expected"
import json, re, sys, time, urllib.request

url, work = sys.argv[1:3]


def fetch(service):
    with urllib.request.urlopen(f"{url}/api/logs?service={service}&limit=1000&range=24h", timeout=30) as response:
        return json.load(response)["logs"]


def as_map(value):
    return json.loads(value) if isinstance(value, str) else (value or {})


def wait_for(service, wanted):
    deadline = time.time() + 60
    while True:
        rows = [row for row in fetch(service) if wanted(row)]
        if rows:
            return rows
        if time.time() > deadline:
            sys.exit(f"no matching OTLP log record for service {service} after 60 s")
        time.sleep(1)


daemon = wait_for("tumultd", lambda row: str(row["body"]).startswith("login failed"))
for row in daemon:
    # The ingest promotes the resource's service.name into this column and
    # keeps only the remaining resource attributes in resource_attrs.
    assert row["service_name"] == "tumultd", row
    assert "service.name" not in as_map(row["resource_attrs"]), row
    assert row["severity_text"] == "WARN", row
print(f"     {len(daemon)} `login failed` record(s) with service.name=tumultd")

cli_line = None
for line in open(f"{work}/cli.stderr", encoding="utf-8"):
    event = json.loads(line)
    if event["message"] == "experiment.completed":
        cli_line = event
assert cli_line, "the CLI logged no experiment.completed line"
trace_id = cli_line.get("trace_id", "")
assert re.fullmatch(r"[0-9a-f]{32}", trace_id) and set(trace_id) != {"0"}, cli_line
assert re.fullmatch(r"[0-9a-f]{16}", cli_line.get("span_id", "")), cli_line

records = wait_for("tumult", lambda row: row["body"] == "experiment.completed")
assert any(row["trace_id"] == trace_id for row in records), (trace_id, records)
print(f"     experiment.completed carries trace_id {trace_id} locally and over OTLP")
open(f"{work}/trace_id", "w").write(trace_id)
PY
pass "records with service.name=tumultd arrived; span event correlates by trace id"

echo "[5] stop the daemon, then read what it wrote"
stop_daemon || fail "daemon did not stop"
pass "daemon stopped"

python3 - "$WORK" "$PASSWORD" "$NEWLINE_USER" <<'PY' || fail "daemon stderr is not the structured stream expected"
import json, re, sys

work, password, newline_user = sys.argv[1:4]
required = ("timestamp", "level", "target", "message", "service.name", "service.version")
raw = open(f"{work}/json.stderr", encoding="utf-8").read()
assert raw.endswith("\n"), "last line is not terminated"
lines = raw.splitlines()
assert len(lines) >= 5, f"only {len(lines)} lines on stderr"
events = []
for number, line in enumerate(lines, 1):
    try:
        event = json.loads(line)
    except ValueError:
        sys.exit(f"stderr line {number} is not JSON: {line!r}")
    assert isinstance(event, dict), (number, line)
    for key in required:
        assert isinstance(event.get(key), str) and event[key], (number, key, line)
    assert event["service.name"] == "tumultd", (number, line)
    assert event["level"] in ("TRACE", "DEBUG", "INFO", "WARN", "ERROR"), (number, line)
    assert re.fullmatch(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{6}Z", event["timestamp"]), (number, line)
    events.append(event)
print(f"     {len(events)} stderr lines, all JSON with {', '.join(required)}")

names = [event.get("event.name") for event in events]
for expected in ("daemon.starting", "daemon.shutdown.signal", "daemon.stopped"):
    assert expected in names, (expected, names)
assert names[-1] == "daemon.stopped", names[-5:]
errors = [event for event in events if event["level"] == "ERROR"]
assert not errors, errors
print("     no ERROR line: the clean stop lost no telemetry export")

failed = [event for event in events if event["message"].startswith("login failed")]
assert len(failed) == 2, [event["message"] for event in events]
plain = [event for event in failed if event.get("username") == "proof-user"]
assert len(plain) == 1, failed
assert plain[0]["level"] == "WARN" and isinstance(plain[0].get("client"), str), plain[0]
print(f"     login failed: level={plain[0]['level']} username={plain[0]['username']} client={plain[0]['client']}")

forged = [event for event in failed if event.get("username") == newline_user]
assert len(forged) == 1, failed
assert not any(line.startswith("level=ERROR") for line in lines), "a forged line reached stderr"
print("     the newline user name is one event on one line")

for name in ("json.stderr", "json.stdout", "cli.stderr", "cli.stdout"):
    assert password not in open(f"{work}/{name}", encoding="utf-8").read(), f"password in {name}"
print("     the password is in no output")
PY
pass "every stderr line is JSON; login failed is structured; newline contained; no password"

[[ ! -s "$WORK/json.stdout" ]] || {
  head -5 "$WORK/json.stdout"
  fail "daemon wrote to stdout"
}
pass "daemon stdout is empty"

echo "[6] TUMULT_LOG_FORMAT=text gives text"
start_daemon text TUMULT_LOG_FORMAT=text
stop_daemon || fail "daemon did not stop"
python3 - "$WORK" <<'PY' || fail "text format is not text"
import json, re, sys

lines = open(f"{sys.argv[1]}/text.stderr", encoding="utf-8").read().splitlines()
assert len(lines) >= 3, lines
for line in lines:
    try:
        json.loads(line)
    except ValueError:
        continue
    sys.exit(f"a JSON line in text mode: {line!r}")

assert all(re.match(r"\d{4}-\d\d-\d\dT\S+Z +(TRACE|DEBUG|INFO|WARN|ERROR) ", line) for line in lines), lines[:3]
assert any("event.name=daemon.starting" in line for line in lines), lines[:3]
assert not any("\x1b[" in line for line in lines), "colour codes without a terminal"
print(f"     {len(lines)} text lines, e.g. {lines[0][:100]}")
PY
[[ ! -s "$WORK/text.stdout" ]] || fail "daemon wrote to stdout in text mode"
pass "text lines on stderr, stdout empty"

echo "[7] nothing left running"
if pgrep -f "$WORK" >/dev/null 2>&1; then
  pgrep -af "$WORK"
  fail "a process started by this proof is still running"
fi
pass "no process left"
echo "ALL STRUCTURED-LOGGING PROOF CHECKS PASSED (run $RUN)"
