#!/usr/bin/env bash
# Forecasting live proof: a fresh daemon gets a day of experiment spans over
# OTLP and one real run launched through its own queue (the run waits for
# approval and is released by break-glass, as this private daemon has no
# approval policy). /api/forecast and /api/runs/{id}/impact answer from the
# lake with the built-in Holt-Winters provider, then with the real Timestral
# model served by forecastd as a separate process, then fall back through
# the circuit breaker when that process is killed, and return to it when it
# is back. No mocks. Every wait is a poll with a deadline. Exit non-zero on
# any failed expectation, 2 when the model environment is missing. Run it
# three times in a row before a release.
#
#   cargo build -p tumultd -p tumult-cli && (cd web && npm ci && npm run build)
#   python3 -m venv "$FORECASTD_VENV"            # see docs/guides/forecasting.md
#   bash scripts/proof/rip-phase3-forecast/proof.sh [run-number]
#
# Environment:
#   FORECASTD_VENV     virtualenv with torch and timestral (default
#                      /home/morgan/data/venvs/forecastd)
#   HF_HOME            Hugging Face cache holding the weights, or where they
#                      are downloaded to (default /home/morgan/data/cache/hf)
#   FORECASTD_MEMORY   memory ceiling for the model process (default 12G)
#   PROOF_OFFLINE_STUB=1  OFFLINE MODE: serve forecastd's stub instead of the
#                      real model. It proves the wiring only; the result line
#                      says so and it does not count as a release proof.
#
# Needs python3, curl and systemd-run. Ports 46317/46318 (daemon) and 46700
# (forecastd) must be free. Work files land under $PROOF_WORK (default: a
# temp dir).
set -euo pipefail
S="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$S/../../.." && pwd)"
BIN=${TUMULT_BIN_DIR:-$REPO/target/debug}
RUN=${1:-1}
WORK=${PROOF_WORK:-$(mktemp -d)}/run-$RUN; rm -rf "$WORK"; mkdir -p "$WORK/state"
export TUMULT_LAKE_PATH=$WORK/lake.duckdb
export KRONIKA_OTLP_GRPC_ADDR=127.0.0.1:46317
export KRONIKA_OTLP_HTTP_ADDR=127.0.0.1:46318
export KRONIKA_METRICS_DIR=$REPO/metrics
URL=http://127.0.0.1:46318
SIDECAR_PORT=46700
SIDECAR=http://127.0.0.1:$SIDECAR_PORT
VENV=${FORECASTD_VENV:-/home/morgan/data/venvs/forecastd}
export HF_HOME=${HF_HOME:-/home/morgan/data/cache/hf}
MEMORY=${FORECASTD_MEMORY:-12G}
COOLDOWN_S=5
FORECAST="/api/forecast?metric=experiment_count&range=24h"

pass() { echo "  ok   $1"; }
fail() { echo "  FAIL $1"; exit 1; }
py() { python3 -c "import json,sys; d=json.load(sys.stdin); $1"; }
now_ms() { date +%s%3N; }

DPID=""; FPID=""
cleanup() { kill $DPID $FPID 2>/dev/null || true; }
trap cleanup EXIT

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
    env "$@" RUST_LOG=warn "$BIN/tumultd" serve >>"$WORK/tumultd.log" 2>&1 &
    DPID=$!
    poll 60 "the daemon's /healthz" curl -fs -o /dev/null "$URL/healthz"
}

stop_daemon() {
    kill "$DPID"; wait "$DPID" 2>/dev/null || true
    DPID=""
}

# Start forecastd as its own process under a memory ceiling.
start_sidecar() {
    (cd "$REPO/forecastd" && exec env FORECASTD_BIND=127.0.0.1:$SIDECAR_PORT \
        FORECASTD_MODEL="$MODEL_KIND" "${SIDECAR_RUN[@]}" -m forecastd) >>"$WORK/forecastd.log" 2>&1 &
    FPID=$!
    poll 180 "forecastd's /healthz" curl -fs -o "$WORK/healthz.json" "$SIDECAR/healthz"
}

stop_sidecar() {
    kill "$FPID"; wait "$FPID" 2>/dev/null || true
    FPID=""
    poll 20 "the forecastd port to close" sidecar_is_down
}
sidecar_is_down() { ! curl -s -o /dev/null --max-time 1 "$SIDECAR/healthz"; }

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

echo "[1] start tumultd on a fresh lake, no sidecar configured"
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
pass "30-step forecast with an 80 % band; the second call is the cached row with the same provenance"

echo "[4] launch a real run through the daemon: 12 s latency fault, sampled every second"
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
    [[ "$REVISION" =~ ^[0-9a-f]{40}$ || "$REVISION" == local ]] || fail "no weights revision: $REVISION"
    [[ "$WEIGHTS" =~ ^[0-9a-f]{64}$ ]] || fail "no weights hash: $WEIGHTS"
fi
pass "forecastd healthy ${LOAD_MS} ms after start, model loaded before the socket opened"

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
pass "forecast (${FORECAST_MS} ms end to end, 1,430-point context) and impact (${IMPACT_MS} ms) come from $EXPECT_MODEL; its row is cached separately"

echo "[8] kill the sidecar: the circuit opens and Holt-Winters answers"
stop_sidecar
curl -fs "$URL/api/runs/$RUN_ID/impact" >"$WORK/impact-down.json"
py 'v=d["probes"][0]["provenance"]; assert v["model"]=="holt-winters", v
assert "sidecar unreachable" in v["fallback_reason"], v; assert d["probes"][0]["impact"]["during_peak"]>1' <"$WORK/impact-down.json"
T0=$(now_ms)
curl -fs "$URL/api/runs/$RUN_ID/impact" >"$WORK/impact-open.json"
OPEN_MS=$(($(now_ms) - T0))
py 'v=d["probes"][0]["provenance"]; assert v["model"]=="holt-winters", v
assert "circuit open" in v["fallback_reason"] and "sidecar unreachable" in v["fallback_reason"], v' <"$WORK/impact-open.json"
[[ $OPEN_MS -lt 2000 ]] || fail "an open circuit must answer quickly, took $OPEN_MS ms"
curl -fs "$URL$FORECAST&horizon=12" >"$WORK/forecast-open.json"
py 'p=d["provenance"]; assert p["model"]=="holt-winters" and "circuit open" in p["fallback_reason"], p
assert len(d["forecast"]["quantiles"])==12' <"$WORK/forecast-open.json"
pass "first request names the failure, the next ones skip the sidecar (${OPEN_MS} ms) and say the circuit is open"

echo "[9] bring the sidecar back: it is used again after the ${COOLDOWN_S} s cool-down"
start_sidecar
poll 60 "the daemon to return to the sidecar" impact_model_is "$EXPECT_MODEL"
curl -fs "$URL/api/runs/$RUN_ID/impact" >"$WORK/impact-back.json"
py 'v=d["probes"][0]["provenance"]; assert v["model"]=="'"$EXPECT_MODEL"'" and v["weights_hash"]=="'"$WEIGHTS"'" and v.get("fallback_reason") is None, v' <"$WORK/impact-back.json"
pass "provenance is $EXPECT_MODEL again, no fallback reason"

echo "[10] the forecasts table holds both providers' rows with their provenance"
stop_daemon
"$BIN/tumult" analyze --query "SELECT model, model_version, weights_hash, horizon, context_points FROM forecasts ORDER BY model, horizon" >"$WORK/forecasts.out" 2>&1 ||
    { cat "$WORK/forecasts.out"; fail "could not read the forecasts table"; }
grep -q "^holt-winters	.*	none	30	" "$WORK/forecasts.out" || { cat "$WORK/forecasts.out"; fail "no Holt-Winters row for horizon 30"; }
grep -q "^$EXPECT_MODEL	$MODEL_ID@$REVISION .*	$WEIGHTS	30	" "$WORK/forecasts.out" || { cat "$WORK/forecasts.out"; fail "no $EXPECT_MODEL row for horizon 30"; }
pass "rows for holt-winters and $EXPECT_MODEL, each with version and weights hash"

if [[ "$MODEL_KIND" == stub ]]; then
    echo "ALL FORECAST PROOF CHECKS PASSED IN OFFLINE STUB MODE — wiring only, not a release proof (run $RUN)"
    exit 0
fi
echo "ALL FORECAST PROOF CHECKS PASSED (run $RUN) — $MODEL_ID@$REVISION, weights sha256 ${WEIGHTS:0:12}…, forecast ${FORECAST_MS} ms, impact ${IMPACT_MS} ms, open circuit ${OPEN_MS} ms"
