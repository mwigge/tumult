#!/usr/bin/env bash
# Forecasting live proof: a fresh daemon gets a day of experiment spans over
# OTLP and one real run launched through its own queue (the run waits for
# approval and is released by break-glass, as this private daemon has no
# approval policy). /api/forecast and /api/runs/{id}/impact answer from the
# lake with the built-in Holt-Winters provider, then with the real Timestral
# model served by forecastd as a separate process, then fall back through
# the circuit breaker when that process is killed, and return to it when it
# is back. The model runs with forecastd's default pin, and the proof checks
# that the weights it loaded are exactly the tested revision and digest, and
# that a wrong digest keeps the service from starting. No mocks. Every wait
# is a poll with a deadline. Exit non-zero on any failed expectation, 2 when
# the model environment is missing, 3 in offline stub mode (never a proof).
# Run it three times in a row before a release.
#
#   cargo build -p tumultd -p tumult-cli && (cd web && npm ci && npm run build)
#   python3 -m venv "$FORECASTD_VENV"            # see docs/guides/forecasting.md
#   bash scripts/proof/rip-phase3-forecast/proof.sh [run-number]
#
# Environment:
#   FORECASTD_VENV     virtualenv with torch and timestral (default
#                      $XDG_DATA_HOME/tumult/forecastd-venv, that is
#                      ~/.local/share/tumult/forecastd-venv)
#   HF_HOME            Hugging Face cache holding the weights, or where they
#                      are downloaded to (default $XDG_CACHE_HOME/huggingface)
#   FORECASTD_MEMORY   memory ceiling for the model process (default 12G)
#   PROOF_WORK         where work files go (default: a new temp dir); each
#                      run uses its own lake at $PROOF_WORK/run-N/lake.duckdb
#   PROOF_OFFLINE_STUB=1  OFFLINE MODE: serve forecastd's stub instead of the
#                      real model. It checks the wiring only: the last line
#                      says NOT A PROOF and the exit status is 3.
#
# Needs python3, curl, ss and systemd-run. Ports 46317/46318 (daemon) and
# 46700/46701 (forecastd) must be free: the proof refuses to start otherwise, and
# checks that the processes it started are the ones listening before it
# posts anything.
set -euo pipefail
S="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$S/../../.." && pwd)"
BIN=${TUMULT_BIN_DIR:-$REPO/target/debug}
RUN=${1:-1}
[[ "$RUN" =~ ^[0-9]{1,6}$ ]] || { echo "the run number must be a number, got '$RUN'"; exit 64; }
BASE=${PROOF_WORK:-$(mktemp -d)}
[[ "$BASE" == /* && "$BASE" != / ]] || { echo "PROOF_WORK must be an absolute directory, got '$BASE'"; exit 64; }
WORK=$BASE/run-$RUN
rm -rf -- "$WORK"; mkdir -p "$WORK/state"
# A private lake for this run only; nothing else may point at it.
export TUMULT_LAKE_PATH=$WORK/lake.duckdb
export KRONIKA_OTLP_GRPC_ADDR=127.0.0.1:46317
export KRONIKA_OTLP_HTTP_ADDR=127.0.0.1:46318
export KRONIKA_METRICS_DIR=$REPO/metrics
URL=http://127.0.0.1:46318
DAEMON_PORTS=(46317 46318)
SIDECAR_PORT=46700
WRONG_PORT=46701
SIDECAR=http://127.0.0.1:$SIDECAR_PORT
VENV=${FORECASTD_VENV:-${XDG_DATA_HOME:-$HOME/.local/share}/tumult/forecastd-venv}
export HF_HOME=${HF_HOME:-${XDG_CACHE_HOME:-$HOME/.cache}/huggingface}
MEMORY=${FORECASTD_MEMORY:-12G}
# The weights forecastd is pinned to by default (forecastd/forecastd/model.py).
PINNED_REVISION=bdd20cfc15a3fea0e4cb5ccef37f8d3ca7aa335f
PINNED_SHA256=577236a804ccb3c2b02638c94b6b433920acf8edd248c9f7a6a32eb50104d63e
COOLDOWN_S=5
FORECAST="/api/forecast?metric=experiment_count&range=24h"

pass() { echo "  ok   $1"; }
fail() { echo "  FAIL $1"; exit 1; }
py() { python3 -c "import json,sys; d=json.load(sys.stdin); $1"; }
now_ms() { date +%s%3N; }

DPID=""; FPID=""; WPID=""
# Stop what this run started and wait for it, so nothing outlives the proof.
cleanup() {
    local pid
    for pid in $DPID $FPID $WPID; do
        kill "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
    done
}
trap cleanup EXIT

# The pid listening on a local TCP port, or nothing.
listener() { ss -H -ltnp "sport = :$1" 2>/dev/null | sed -n 's/.*pid=\([0-9]*\).*/\1/p' | head -n 1; }
port_is_free() { [[ -z "$(ss -H -ltn "sport = :$1" 2>/dev/null)" ]]; }
# A child that has exited stays a zombie until it is waited for, and a
# zombie still answers kill -0: read its state instead.
has_exited() { [[ ! -r /proc/$1/stat ]] || [[ "$(sed 's/.*) //' "/proc/$1/stat" | cut -c1)" == Z ]]; }
# owns <pid> <port>: the process this run started is alive and is the listener.
owns() { kill -0 "$1" 2>/dev/null && [[ "$(listener "$2")" == "$1" ]]; }
require_free() {
    local port
    for port in "$@"; do
        port_is_free "$port" || fail "port $port is already in use (pid $(listener "$port")): not ours, refusing to talk to it"
    done
}

# poll <seconds> <description> <command...>: retry until the command succeeds.
poll() {
    local deadline=$(($(date +%s) + $1)) what=$2
    shift 2
    until "$@"; do
        [[ $(date +%s) -lt $deadline ]] || fail "timed out waiting for $what"
        sleep 0.25
    done
}

# Start the daemon; extra arguments are environment assignments.
start_daemon() {
    require_free "${DAEMON_PORTS[@]}"
    env "$@" RUST_LOG=warn "$BIN/tumultd" serve >>"$WORK/tumultd.log" 2>&1 &
    DPID=$!
    poll 60 "the daemon's /healthz" curl -fs -o /dev/null "$URL/healthz"
    owns "$DPID" "${DAEMON_PORTS[1]}" ||
        fail "the process answering on ${DAEMON_PORTS[1]} is not the daemon this run started (pid $DPID)"
}

stop_daemon() {
    kill "$DPID"; wait "$DPID" 2>/dev/null || true
    DPID=""
    poll 20 "the daemon's ports to close" daemon_ports_free
}
daemon_ports_free() { port_is_free "${DAEMON_PORTS[0]}" && port_is_free "${DAEMON_PORTS[1]}"; }

# sidecar_exec <port> [VAR=value...]: become forecastd (call it in a
# subshell). The model settings of the calling shell are dropped so the
# service runs with its own defaults, pin included.
sidecar_exec() {
    local port=$1
    shift
    cd "$REPO/forecastd"
    exec env -u FORECASTD_MODEL_SIZE -u FORECASTD_MODEL_ID -u FORECASTD_MODEL_REVISION \
        -u FORECASTD_WEIGHTS_SHA256 -u FORECASTD_WEIGHTS_PATH \
        FORECASTD_BIND=127.0.0.1:"$port" FORECASTD_MODEL="$MODEL_KIND" "$@" \
        "${SIDECAR_RUN[@]}" -m forecastd
}

# Start forecastd as its own process under a memory ceiling.
start_sidecar() {
    require_free "$SIDECAR_PORT"
    (sidecar_exec "$SIDECAR_PORT") >>"$WORK/forecastd.log" 2>&1 &
    FPID=$!
    poll 180 "forecastd's /healthz" sidecar_answers
}
# The health answer counts only when it comes from the process this run
# started (the scope's main process, or a child of it).
sidecar_answers() {
    ! has_exited "$FPID" || fail "forecastd exited: $(tail -n 3 "$WORK/forecastd.log")"
    curl -fs -o "$WORK/healthz.json" "$SIDECAR/healthz" || return 1
    local pid; pid=$(listener "$SIDECAR_PORT")
    [[ "$pid" == "$FPID" || "$(ps -o ppid= -p "${pid:-0}" 2>/dev/null | tr -d ' ')" == "$FPID" ]] ||
        fail "the process answering on $SIDECAR_PORT (pid ${pid:-unknown}) is not the forecastd this run started (pid $FPID)"
}

stop_sidecar() {
    kill "$FPID"; wait "$FPID" 2>/dev/null || true
    FPID=""
    poll 20 "the forecastd port to close" sidecar_is_down
}
sidecar_is_down() { ! curl -s -o /dev/null --max-time 1 "$SIDECAR/healthz"; }

# structured_log <file> <event...>: every line of a forecastd log (stdout
# and stderr together) is one JSON object with the six agreed keys, and
# each named event is among them. Prints the number of lines.
structured_log() {
    python3 -I -c '
import json, sys
keys = {"timestamp", "level", "logger", "event", "message", "fields"}
seen = set()
lines = open(sys.argv[1], encoding="utf-8").read().splitlines()
assert lines, "the log is empty"
for number, line in enumerate(lines, 1):
    try:
        entry = json.loads(line)
    except ValueError:
        sys.exit("line %d is not JSON: %r" % (number, line[:200]))
    if not (isinstance(entry, dict) and set(entry) == keys and isinstance(entry["fields"], dict)):
        sys.exit("line %d is not a log entry: %r" % (number, line[:200]))
    seen.add(entry["event"])
missing = [name for name in sys.argv[2:] if name not in seen]
if missing:
    sys.exit("no %s event among %s" % (" or ".join(missing), sorted(seen)))
print(len(lines))' "$@"
}

# The provider named by a response's provenance.
model_of() { py 'print(d.get("provenance", (d.get("probes") or [{}])[0].get("provenance", {})).get("model"))'; }
impact_model_is() { [[ "$(curl -fs "$URL/api/runs/$RUN_ID/impact" | model_of)" == "$1" ]]; }

if [[ "${PROOF_OFFLINE_STUB:-}" == 1 ]]; then
    MODEL_KIND=stub; EXPECT_MODEL=stub
    SIDECAR_RUN=(python3)
    echo "OFFLINE MODE: forecastd serves its stub, not the Timestral model. This run proves the wiring only."
else
    MODEL_KIND=timestral; EXPECT_MODEL=timestral
    [[ -x "$VENV/bin/python" ]] || { echo "  no virtualenv at $VENV: set FORECASTD_VENV (docs/guides/forecasting.md)"; exit 2; }
    "$VENV/bin/python" -c 'import torch, timestral' 2>/dev/null ||
        { echo "  $VENV has no torch/timestral: install them (docs/guides/forecasting.md)"; exit 2; }
    command -v systemd-run >/dev/null || { echo "  this proof needs systemd-run for the memory ceiling"; exit 2; }
    SIDECAR_RUN=(systemd-run --user --scope -q -p "MemoryMax=$MEMORY" "$VENV/bin/python")
fi

command -v ss >/dev/null || { echo "  this proof needs ss (iproute2) to see who is listening"; exit 2; }

echo "[1] start tumultd on a fresh lake, no sidecar configured"
require_free "$SIDECAR_PORT"
start_daemon
code=$(curl -s -o /dev/null -w '%{http_code}' "$URL$FORECAST&horizon=30")
[[ "$code" == 404 ]] || fail "a metric with no points should be 404, got $code"
pass "daemon healthy; /api/forecast is 404 before any telemetry"

echo "[2] seed a day of experiment spans over OTLP/HTTP"
SENT=$(python3 "$S/seed_spans.py" "$URL")
seeded() {
    curl -fs "$URL/api/overview?range=24h" >"$WORK/overview.json" || return 1
    py "k=[k for k in d['kpis'] if k['name']=='experiments'][0]; assert k['value'] >= $SENT, k" <"$WORK/overview.json" 2>/dev/null
}
poll 60 "the seeded spans to be queryable" seeded
pass "$SENT spans ingested (the 24h overview counts them all)"

echo "[3] /api/forecast: Holt-Winters with intervals and provenance, then served from the cache"
# The cache key includes the step the context ends at, so a pair of calls
# straddling a minute boundary is a miss twice; ask again until a pair lands.
hw_pair() {
    curl -fs "$URL$FORECAST&horizon=30" >"$WORK/hw.json" || return 1
    curl -fs "$URL$FORECAST&horizon=30" >"$WORK/hw-again.json" || return 1
    py 'assert d["cached"] is True' <"$WORK/hw-again.json" 2>/dev/null
}
poll 90 "a cached Holt-Winters answer" hw_pair
py 'p=d["provenance"]; assert p["model"]=="holt-winters", p; assert p["version"] and p["weights_hash"]=="none", p
assert p.get("fallback_reason") is None, p
f=d["forecast"]; assert f["levels"]==[0.1,0.5,0.9], f["levels"]; assert len(f["quantiles"])==30
assert all(lo<=mid<=hi for lo,mid,hi in f["quantiles"]), "quantiles cross"
assert any(hi>lo for lo,_,hi in f["quantiles"]), "the band has no width"
assert f["context_points"]>=1400 and f["insufficient_history"] is False, f["context_points"]
b=d["series"]["bands"][0]; assert b["level"]==0.8 and len(b["lower"])==len(d["series"]["ts"])
mids=[q[1] for q in f["quantiles"]]
print("     holt-winters %s over %d context points; median %.2f..%.2f, first band [%.2f, %.2f]" % (p["version"], f["context_points"], min(mids), max(mids), f["quantiles"][0][0], f["quantiles"][0][2]))' <"$WORK/hw.json"
python3 - "$WORK/hw.json" "$WORK/hw-again.json" <<'PY'
import json, sys
first, again = (json.load(open(p)) for p in sys.argv[1:3])
assert again["provenance"] == first["provenance"], (first["provenance"], again["provenance"])
# The stored row round-trips through text: equal to the last bit or two.
gap = max(abs(x - y) for a, b in zip(first["forecast"]["quantiles"], again["forecast"]["quantiles"]) for x, y in zip(a, b))
assert len(again["forecast"]["quantiles"]) == 30 and gap < 1e-9, f"the cached row differs by {gap}"
PY
# A longer range in the same resolution tier is another context: it must be
# fitted on its own window, never answered with the 24 h row.
curl -fs "$URL/api/forecast?metric=experiment_count&range=14d&horizon=30" >"$WORK/hw-14d.json"
python3 - "$WORK/hw.json" "$WORK/hw-14d.json" <<'PY'
import json, sys
day, fortnight = (json.load(open(p)) for p in sys.argv[1:3])
assert day["step_ns"] == fortnight["step_ns"], "the two ranges are meant to share a step"
assert fortnight["from_ms"] < day["from_ms"], (day["from_ms"], fortnight["from_ms"])
assert fortnight["cached"] is False, "a 14 d view was answered with a row fitted on another window"
PY
pass "30-step forecast with an 80 % band; the second call is the cached row with the same provenance, and a 14 d view is not"

echo "[4] launch a real run through the daemon: 12 s latency fault, sampled every second"
owns "$DPID" "${DAEMON_PORTS[1]}" || fail "the daemon on ${DAEMON_PORTS[1]} is not the one this run started"
sed -e "s|@PROOF_DIR@|$S|g" -e "s|@STATE_DIR@|$WORK/state|g" "$S/latency-fault.toon.in" >"$WORK/latency-fault.toon"
post() { curl -fs -X POST -H 'content-type: application/json' -d "$2" "$URL$1"; }
REG=$(python3 -c 'import json,sys; print(json.dumps({"toon": open(sys.argv[1]).read()}))' "$WORK/latency-fault.toon" |
    curl -fs -X POST -H 'content-type: application/json' -d @- "$URL/api/runs/validate" |
    py 'assert d["valid"] is True, d; print(d["registry_id"])')
post /api/runs/dry-run "{\"registry_id\":\"$REG\",\"env\":\"dev\"}" >"$WORK/dry-run.json"
HASH=$(py 'print(d["execution_hash"])' <"$WORK/dry-run.json")
post /api/runs "{\"registry_id\":\"$REG\",\"env\":\"dev\",\"execution_hash\":\"$HASH\"}" >"$WORK/run.json"
RUN_ID=$(py 'print(d["run_id"])' <"$WORK/run.json")
# A definition with an action and no rollback waits for approval, and this
# daemon has no autopilot policy to approve against. The proof daemon is
# private and the "fault" is a flag file, so release the run by break-glass,
# which the daemon audits.
owns "$DPID" "${DAEMON_PORTS[1]}" || fail "the daemon on ${DAEMON_PORTS[1]} is no longer the one this run started"
if [[ "$(py 'print(d["state"])' <"$WORK/run.json")" == pending_approval ]]; then
    post "/api/runs/$RUN_ID/break-glass" '{"justification":"forecast proof run on a private throwaway daemon"}' >"$WORK/break-glass.json"
    py 'assert d["break_glass"] is True and d["state"]=="queued", d' <"$WORK/break-glass.json"
fi
finished() {
    curl -fs "$URL/api/runs/$RUN_ID" >"$WORK/run-detail.json" || return 1
    py 'import sys; s=d["run"]["state"]; sys.exit(0 if s in ("passed","deviated","failed","aborted","orphaned","rejected","expired") else 1)' <"$WORK/run-detail.json"
}
poll 120 "run $RUN_ID to finish" finished
py 's=d["run"]["state"]; assert s=="passed", s' <"$WORK/run-detail.json" || { cat "$WORK/run-detail.json"; fail "the run did not pass"; }
pass "run $RUN_ID passed (previewed, launched and executed by the daemon)"

echo "[5] /api/runs/{id}/impact: peak, recovery and the reference kind"
has_impact() { curl -fs "$URL/api/runs/$RUN_ID/impact" >"$WORK/impact-hw.json"; }
poll 60 "the run's probe samples" has_impact
py 'assert d["run_id"]=="'"$RUN_ID"'", d["run_id"]
p=d["probes"][0]; assert p["probe"]=="health", p["probe"]; assert p["reference"]=="post_backcast", p["reference"]
i=p["impact"]; assert i["samples_during"]>=8 and i["samples_post"]>=15, i
assert i["during_peak"]>1 and i["during_outside_ratio"]>=0.8, i
r=p["recovery"]; assert r["recovery_time_s"] is not None and 0<=r["recovery_time_s"]<=30, r
assert d["recovery_time_s"]==r["recovery_time_s"] and abs(d["impact_peak"]-abs(i["during_peak"]))<1e-9, (d["recovery_time_s"], d["impact_peak"])
assert p["provenance"]["model"]=="holt-winters" and p["provenance"].get("fallback_reason") is None, p["provenance"]
assert p["degenerate_band"] in (True, False) and d["degenerate_band"]==p["degenerate_band"], "the band flag is missing"
assert p["fault"]["start_ms"]==p["fault"]["start_ns"]//10**6 and p["recovery"]["recovered_at_ms"] is not None, p["fault"]
n=i["samples_during"]; cf=[m for m in p["series"]["median"][:n] if m is not None]; top=max(o for o in p["series"]["observed"][:n] if o is not None)
assert len(cf)==n and max(cf)<top/2, "the counterfactual follows the fault or the recovery tail: %s" % cf
b=p["series"]["bands"][0]; assert len(b["lower"])==len(p["series"]["observed"])
print("     health: %d fault samples, %d post samples; peak impact %.1f half-widths, %.0f %% of the fault outside the band, recovered %.0f s after the fault (%s)" % (i["samples_during"], i["samples_post"], i["during_peak"], 100*i["during_outside_ratio"], r["recovery_time_s"], p["reference"]))' <"$WORK/impact-hw.json"
pass "the fault stands out from the post-phase backcast and the probe recovers"

echo "[6] start forecastd as a separate process ($MODEL_KIND model, memory ceiling $MEMORY)"
T0=$(now_ms)
start_sidecar
LOAD_MS=$(($(now_ms) - T0))
py 'assert d["status"]=="ok" and d["model"]=="'"$EXPECT_MODEL"'", d
print("     %s %s, model %s, weights revision %s, weights sha256 %s" % (d["model"], d["version"], d["model_id"], d["revision"], d["weights_hash"]))' <"$WORK/healthz.json"
REVISION=$(py 'print(d["revision"])' <"$WORK/healthz.json")
WEIGHTS=$(py 'print(d["weights_hash"])' <"$WORK/healthz.json")
MODEL_ID=$(py 'print(d["model_id"])' <"$WORK/healthz.json")
if [[ "$MODEL_KIND" == timestral ]]; then
    [[ "$REVISION" == "$PINNED_REVISION" ]] || fail "weights revision $REVISION is not the pinned $PINNED_REVISION"
    [[ "$WEIGHTS" == "$PINNED_SHA256" ]] || fail "weights sha256 $WEIGHTS is not the pinned $PINNED_SHA256"
fi
pass "forecastd healthy ${LOAD_MS} ms after start, serving exactly the pinned revision and weights"

if [[ "$MODEL_KIND" == timestral ]]; then
    echo "[6b] a second forecastd told to expect other weights refuses to start"
    WRONG=$(printf '0%.0s' $(seq 64)); code=0
    require_free "$WRONG_PORT"
    (sidecar_exec "$WRONG_PORT" FORECASTD_WEIGHTS_SHA256="$WRONG") >"$WORK/forecastd-wrong.log" 2>&1 &
    WPID=$!
    poll 120 "the mispinned forecastd to exit" has_exited "$WPID"
    wait "$WPID" || code=$?
    WPID=""
    [[ $code -eq 1 ]] || { tail -n 5 "$WORK/forecastd-wrong.log"; fail "expected exit status 1, got $code"; }
    grep -q "WeightsMismatch" "$WORK/forecastd-wrong.log" || fail "the refusal does not name the mismatch"
    grep -q "$PINNED_SHA256" "$WORK/forecastd-wrong.log" || fail "the refusal does not show the digest it found"
    port_is_free "$WRONG_PORT" || fail "the refusing forecastd left a listener behind"
    LINES=$(structured_log "$WORK/forecastd-wrong.log" startup.failed) || fail "the refusal is not a structured log: $LINES"
    pass "exit status 1 with WeightsMismatch before any socket was opened, said in $LINES JSON log line(s)"
fi

echo "[7] restart tumultd with the sidecar configured: provenance names the model and its weights"
stop_daemon
start_daemon TUMULTD_FORECAST_URL=$SIDECAR TUMULTD_FORECAST_COOLDOWN_S=$COOLDOWN_S
T0=$(now_ms)
curl -fs "$URL$FORECAST&horizon=30" >"$WORK/ts.json"
FORECAST_MS=$(($(now_ms) - T0))
py 'p=d["provenance"]; assert p["model"]=="'"$EXPECT_MODEL"'", p
assert p["version"].startswith("'"$MODEL_ID@$REVISION"'"), p["version"]; assert p["weights_hash"]=="'"$WEIGHTS"'", p
assert p.get("fallback_reason") is None, p; assert d["cached"] is False, "the Holt-Winters row must not answer for the sidecar"
f=d["forecast"]; assert len(f["quantiles"])==30 and all(lo<=mid<=hi for lo,mid,hi in f["quantiles"])
assert any(hi>lo for lo,_,hi in f["quantiles"]); assert f["context_points"]>=1400, f["context_points"]
mids=[q[1] for q in f["quantiles"]]
print("     %s; median %.2f..%.2f, first band [%.2f, %.2f]" % (p["version"], min(mids), max(mids), f["quantiles"][0][0], f["quantiles"][0][2]))' <"$WORK/ts.json"
CONTEXT_POINTS=$(py 'print(d["forecast"]["context_points"])' <"$WORK/ts.json")
ts_cached() {
    curl -fs "$URL$FORECAST&horizon=30" >"$WORK/ts-again.json" || return 1
    py 'assert d["cached"] is True and d["provenance"]["model"]=="'"$EXPECT_MODEL"'" and d["provenance"]["weights_hash"]=="'"$WEIGHTS"'"' <"$WORK/ts-again.json" 2>/dev/null
}
poll 90 "a cached sidecar answer" ts_cached
T0=$(now_ms)
curl -fs "$URL/api/runs/$RUN_ID/impact" >"$WORK/impact-ts.json"
IMPACT_MS=$(($(now_ms) - T0))
py 'p=d["probes"][0]; v=p["provenance"]; assert v["model"]=="'"$EXPECT_MODEL"'", v
assert v["version"].startswith("'"$MODEL_ID@$REVISION"'") and v["weights_hash"]=="'"$WEIGHTS"'", v
assert p["reference"]=="post_backcast" and p["impact"]["during_peak"]>1, p["impact"]
assert p["recovery"]["recovery_time_s"] is not None, p["recovery"]
n=p["impact"]["samples_during"]; cf=[m for m in p["series"]["median"][:n] if m is not None]; top=max(o for o in p["series"]["observed"][:n] if o is not None)
assert len(cf)==n and max(cf)<top/2, "the counterfactual follows the fault or the recovery tail: %s" % cf
print("     impact with %s: peak %.1f half-widths, recovered %.0f s after the fault" % (v["model"], p["impact"]["during_peak"], p["recovery"]["recovery_time_s"]))' <"$WORK/impact-ts.json"
pass "forecast (${FORECAST_MS} ms end to end, ${CONTEXT_POINTS}-point context) and impact (${IMPACT_MS} ms) come from $EXPECT_MODEL; its row is cached separately"

echo "[8] kill the sidecar: the circuit opens and Holt-Winters answers"
stop_sidecar
curl -fs "$URL/api/runs/$RUN_ID/impact" >"$WORK/impact-down.json"
py 'v=d["probes"][0]["provenance"]; assert v["model"]=="holt-winters", v
assert v["fallback_reason"]=="unreachable", v; assert d["probes"][0]["impact"]["during_peak"]>1' <"$WORK/impact-down.json"
T0=$(now_ms)
curl -fs "$URL/api/runs/$RUN_ID/impact" >"$WORK/impact-open.json"
OPEN_MS=$(($(now_ms) - T0))
py 'v=d["probes"][0]["provenance"]; assert v["model"]=="holt-winters", v
assert v["fallback_reason"]=="circuit_open", v' <"$WORK/impact-open.json"
[[ $OPEN_MS -lt 2000 ]] || fail "an open circuit must answer quickly, took $OPEN_MS ms"
curl -fs "$URL$FORECAST&horizon=12" >"$WORK/forecast-open.json"
py 'p=d["provenance"]; assert p["model"]=="holt-winters" and p["fallback_reason"]=="circuit_open", p
assert len(d["forecast"]["quantiles"])==12' <"$WORK/forecast-open.json"
grep -q "forecast sidecar failed" "$WORK/tumultd.log" ||
    fail "the detail of the sidecar failure is not in the daemon's log"
pass "first request says unreachable, the next ones skip the sidecar (${OPEN_MS} ms) and say circuit_open; the detail is in the log only"

echo "[9] bring the sidecar back: it is used again after the ${COOLDOWN_S} s cool-down"
start_sidecar
poll 60 "the daemon to return to the sidecar" impact_model_is "$EXPECT_MODEL"
curl -fs "$URL/api/runs/$RUN_ID/impact" >"$WORK/impact-back.json"
py 'v=d["probes"][0]["provenance"]; assert v["model"]=="'"$EXPECT_MODEL"'" and v["weights_hash"]=="'"$WEIGHTS"'" and v.get("fallback_reason") is None, v' <"$WORK/impact-back.json"
pass "provenance is $EXPECT_MODEL again, no fallback reason"
LINES=$(structured_log "$WORK/forecastd.log" model.loaded http.access) || fail "forecastd's log is not structured: $LINES"
pass "all $LINES lines forecastd wrote (two starts, stdout and stderr) are JSON log entries"

echo "[10] the forecasts table holds both providers' rows with their provenance"
stop_daemon
"$BIN/tumult" analyze --query "SELECT model, model_version, weights_hash, horizon, context_points FROM forecasts ORDER BY model, horizon" >"$WORK/forecasts.out" 2>&1 ||
    { cat "$WORK/forecasts.out"; fail "could not read the forecasts table"; }
grep -q "^holt-winters	.*	none	96	" "$WORK/forecasts.out" || { cat "$WORK/forecasts.out"; fail "no Holt-Winters row at the 96-step bucket (asked 30)"; }
grep -q "^$EXPECT_MODEL	$MODEL_ID@$REVISION .*	$WEIGHTS	96	" "$WORK/forecasts.out" || { cat "$WORK/forecasts.out"; fail "no $EXPECT_MODEL row at the 96-step bucket (asked 30)"; }
pass "rows for holt-winters and $EXPECT_MODEL, each with version and weights hash"

if [[ "$MODEL_KIND" == stub ]]; then
    echo "NOT A PROOF: offline stub mode checked the wiring only, no model was run (run $RUN)"
    exit 3
fi
echo "ALL FORECAST PROOF CHECKS PASSED (run $RUN) — $MODEL_ID@$REVISION, weights sha256 ${WEIGHTS:0:12}…, forecast ${FORECAST_MS} ms, impact ${IMPACT_MS} ms, open circuit ${OPEN_MS} ms"
