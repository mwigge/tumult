#!/usr/bin/env bash
#
# demo-check.sh — health-wait + per-domain fault sweep + telemetry assertions
# for the Tumult 2.2 one-command demo.
#
# Two modes:
#   full      (default) — assert every experiment ends Completed AND that spans
#                         reached the collector, then assert the Star Map of
#                         the Krönika demo. Exits non-zero on any failure.
#                         This is the dev smoke test (`make demo-check`).
#   populate            — run the sweep once, best-effort, to warm the SigNoz
#                         dashboards. Never fails the caller (`make demo`).
#
# Usage:
#   scripts/demo-check.sh [--mode full|populate] [--no-wait] [--starmap-only]
#
# The Star Map assertions (scripts/starmap-check.py) need a Krönika daemon
# that holds the Krönika demo. By default this script brings
# docker/docker-compose.kronika.yml up as its own compose project, with its
# own container names and image tag (tumult-demo-check-kronika), on a fresh
# volume with throwaway credentials and on host ports 24317/24318, so a
# Krönika demo that is already up is left alone. It runs the seed, checks the
# map (suppressing one risk to show a suppressed one) and removes the project
# again, also when it is interrupted; the image stays as a build cache. To
# check a daemon that is already running, set STARMAP_URL (and STARMAP_TOKEN
# when it asks for one): nothing is started, removed or written then, so the
# suppressed-risk line is skipped unless a risk is suppressed already.
# --starmap-only runs just this part.
#
# Assumes the demo stack is already `up` (the Makefile brings it up first).
# Runs each experiment inside the tumult-mcp container via `tumult run`, the
# same path the control panel exercises through the MCP run_experiment tool.

set -euo pipefail

# ── Config ────────────────────────────────────────────────────────
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMPOSE="${COMPOSE_DEMO:-docker compose -f ${REPO_ROOT}/docker/docker-compose.demo.yml}"
MODE="full"
DO_WAIT=1

# Domains in run order (name → experiment file basename). Each ends Completed
# on success, so they belong in the pass/fail sweep. The auto-halt guardrail
# (demo-guard-halt.toon) is deliberately excluded: its expected outcome is
# Halted, not Completed, so it is exercised from the control panel's own
# "Safety guardrail" card instead.
DOMAINS=(net postgres container stress process ssh agentic agentic-trajectory timewarp-clock timewarp-entropy)

# Containers that carry a healthcheck we wait on. demo-collector is excluded:
# its image is distroless (no in-container probe possible), so its readiness is
# instead proven by the span-throughput assertion at the end of the sweep.
HEALTH_CONTAINERS=(demo-postgres demo-app demo-sshd demo-signoz demo-mcp)

HEALTH_TIMEOUT="${HEALTH_TIMEOUT:-240}"

# Star Map: the compose project this script owns, and where its daemon listens.
# The command is an array so that a checkout path with a space stays one word;
# COMPOSE_KRONIKA, when set, is split on whitespace like COMPOSE_DEMO.
KRONIKA_PROJECT="tumult-demo-check-kronika"
KRONIKA_COMPOSE=(docker compose -f "${REPO_ROOT}/docker/docker-compose.kronika.yml")
if [[ -n "${COMPOSE_KRONIKA:-}" ]]; then
  read -r -a KRONIKA_COMPOSE <<<"$COMPOSE_KRONIKA"
fi
KRONIKA_COMPOSE+=(-p "$KRONIKA_PROJECT")
KRONIKA_STARTED=0
STARMAP_URL="${STARMAP_URL:-}"
STARMAP_WAIT="${STARMAP_WAIT:-90}"
STARMAP_ONLY=0
# The Krönika seed runs eight experiments; two do not pass by design.
SEED_TALLY="6 passed, 2 deviated/failed"

# ── Args ──────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
  case "$1" in
    --mode)     MODE="${2:?--mode needs a value}"; shift 2 ;;
    --mode=*)   MODE="${1#*=}"; shift ;;
    --no-wait)  DO_WAIT=0; shift ;;
    --starmap-only) STARMAP_ONLY=1; shift ;;
    -h|--help)  grep '^#' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

if [[ "$MODE" != "full" && "$MODE" != "populate" ]]; then
  echo "invalid --mode: $MODE (expected 'full' or 'populate')" >&2
  exit 2
fi

# ── Output helpers ────────────────────────────────────────────────
c_green=$'\033[32m'; c_red=$'\033[31m'; c_yellow=$'\033[33m'; c_reset=$'\033[0m'
pass() { printf "  %sPASS%s  %s\n" "$c_green" "$c_reset" "$1"; }
fail() { printf "  %sFAIL%s  %s\n" "$c_red" "$c_reset" "$1"; }
warn() { printf "  %sWARN%s  %s\n" "$c_yellow" "$c_reset" "$1"; }
info() { printf "  ----  %s\n" "$1"; }

FAILURES=0
record_fail() { FAILURES=$((FAILURES + 1)); }

# ── Health wait ───────────────────────────────────────────────────
container_health() {
  # Prints the container's health status, or "missing" if it does not exist.
  docker inspect -f '{{if .State.Health}}{{.State.Health.Status}}{{else}}nohealthcheck{{end}}' "$1" 2>/dev/null || echo "missing"
}

wait_for_health() {
  echo "Waiting for services to become healthy (timeout ${HEALTH_TIMEOUT}s)..."
  local deadline=$((SECONDS + HEALTH_TIMEOUT))
  while true; do
    local all_ok=1 pending=""
    for c in "${HEALTH_CONTAINERS[@]}"; do
      local st; st="$(container_health "$c")"
      if [[ "$st" != "healthy" ]]; then
        all_ok=0; pending+=" ${c}:${st}"
      fi
    done
    if [[ $all_ok -eq 1 ]]; then
      pass "all services healthy"
      return 0
    fi
    if [[ $SECONDS -ge $deadline ]]; then
      fail "health timeout; still pending:${pending}"
      return 1
    fi
    printf "\r  ....  pending:%s   " "${pending}"
    sleep 4
  done
}

# ── Sweep ─────────────────────────────────────────────────────────
run_experiment() {
  local domain="$1"
  local exp="/demo/experiments/demo-${domain}.toon"
  local journal="/journals/demo-${domain}.journal.toon"
  # -T: no TTY (safe under make / CI). Redirect run output to a temp log.
  local log; log="$(mktemp)"
  if $COMPOSE exec -T tumult-mcp \
        tumult run "$exp" --journal-path "$journal" --force >"$log" 2>&1; then
    local status; status="$(grep -E '^Status:' "$log" | head -1 | sed 's/Status: //')"
    pass "demo-${domain}  (${status:-Completed})"
    rm -f "$log"
    return 0
  else
    fail "demo-${domain}  — tumult run exited non-zero"
    sed 's/^/        /' "$log" | grep -viE 'opentelemetry|TracerProvider' | tail -12 || true
    rm -f "$log"
    return 1
  fi
}

# ── Telemetry assertion ───────────────────────────────────────────
# Assert the collector accepted spans. Its self-telemetry (published on host
# :18888) exposes otelcol_receiver_accepted_spans* — a value > 0 proves the
# experiment runs and demo-app traffic reached the collector.
assert_telemetry() {
  local metrics
  if ! metrics="$(curl -sf -m 8 http://localhost:18888/metrics 2>/dev/null)"; then
    fail "telemetry — could not scrape collector self-metrics on :18888"
    return 1
  fi
  local accepted
  accepted="$(printf '%s\n' "$metrics" \
    | grep -E '^otelcol_receiver_accepted_spans' \
    | grep -v '#' \
    | awk '{s += $NF} END {printf "%d", s}')"
  accepted="${accepted:-0}"
  if [[ "$accepted" -gt 0 ]]; then
    pass "telemetry — collector accepted ${accepted} spans"
    return 0
  fi
  # Fallback: grep the collector logs for exported traces.
  if $COMPOSE logs tumult-collector 2>/dev/null | grep -qiE 'TracesExporter|"spans"|ResourceSpans'; then
    pass "telemetry — collector logs show exported spans"
    return 0
  fi
  fail "telemetry — no spans observed at the collector"
  return 1
}

# ── Star Map assertion ────────────────────────────────────────────
throwaway_secret() { head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n'; }

kronika_down() {
  if ! "${KRONIKA_COMPOSE[@]}" down -v --remove-orphans >/dev/null 2>&1; then
    warn "star map — could not remove the Krönika check stack"
  fi
}

# Remove the check stack once, however the script ends.
kronika_cleanup() {
  [[ $KRONIKA_STARTED -eq 1 ]] || return 0
  KRONIKA_STARTED=0
  kronika_down
}

# What the compose file reads. Container names and the image tag are global
# to the Docker host, so they are always the check's own: a Krönika demo the
# user has up keeps its containers and its image.
kronika_env() {
  export KRONIKA_NAME_PREFIX="$KRONIKA_PROJECT"
  export KRONIKA_IMAGE="${KRONIKA_PROJECT}:latest"
  export KRONIKA_HOST_GRPC_PORT="${KRONIKA_HOST_GRPC_PORT:-24317}"
  export KRONIKA_HOST_HTTP_PORT="${KRONIKA_HOST_HTTP_PORT:-24318}"
  export KRONIKA_BOOTSTRAP_ADMIN_PASSWORD KRONIKA_BOOTSTRAP_TOKEN KRONIKA_INGEST_TOKEN
  KRONIKA_BOOTSTRAP_ADMIN_PASSWORD="$(throwaway_secret)"
  KRONIKA_BOOTSTRAP_TOKEN="$(throwaway_secret)"
  KRONIKA_INGEST_TOKEN="$(throwaway_secret)"
}

# Bring the Krönika demo up on a fresh volume and run its seed. Sets
# STARMAP_URL and STARMAP_TOKEN for the daemon it started. From here on the
# stack is removed on every way out, an interrupt included.
kronika_up() {
  kronika_env
  local log; log="$(mktemp)"
  kronika_down
  KRONIKA_STARTED=1
  trap kronika_cleanup EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  if ! "${KRONIKA_COMPOSE[@]}" up -d --build --wait tumultd >"$log" 2>&1; then
    fail "star map — the Krönika daemon did not come up"
    tail -12 "$log" | sed 's/^/        /'
    rm -f "$log"
    return 1
  fi
  if ! "${KRONIKA_COMPOSE[@]}" run --rm seed >"$log" 2>&1; then
    fail "star map — the Krönika seed exited non-zero"
    tail -12 "$log" | sed 's/^/        /'
    rm -f "$log"
    return 1
  fi
  if ! grep -q "suite done — ${SEED_TALLY}" "$log"; then
    fail "star map — the seed did not end with ${SEED_TALLY}"
    grep '=== seed:' "$log" | sed 's/^/        /' || true
    rm -f "$log"
    return 1
  fi
  rm -f "$log"
  pass "star map — Krönika demo seeded (${SEED_TALLY})"
  STARMAP_URL="http://127.0.0.1:${KRONIKA_HOST_HTTP_PORT}"
  STARMAP_TOKEN="$KRONIKA_BOOTSTRAP_TOKEN"
}

# Print each line of the check as a PASS, a FAIL or a skipped line of this
# script.
report_starmap() {
  local line
  while IFS= read -r line; do
    case "$line" in
      "ok  "*) pass "${line#ok  }" ;;
      "skip "*) warn "${line#skip }" ;;
      *)       fail "${line#FAIL }" ;;
    esac
  done
}

# The check writes (it suppresses a risk) only to the stack this script
# started and removes; a daemon named by STARMAP_URL is read, never written.
assert_starmap() {
  local rc=0 out
  local -a write=()
  if [[ -z "$STARMAP_URL" ]]; then
    write=(--write)
    if ! kronika_up; then
      kronika_cleanup
      return 1
    fi
  fi
  out="$(STARMAP_TOKEN="${STARMAP_TOKEN:-}" python3 "${REPO_ROOT}/scripts/starmap-check.py" \
    "$STARMAP_URL" --wait "$STARMAP_WAIT" "${write[@]}" 2>&1)" || rc=1
  report_starmap <<<"$out"
  kronika_cleanup
  return $rc
}

finish() {
  echo ""
  echo "------------------------------------------------------------------"
  if [[ $FAILURES -eq 0 ]]; then
    pass "ALL CHECKS PASSED"
    echo "------------------------------------------------------------------"
    exit 0
  fi
  fail "${FAILURES} check(s) failed"
  echo "------------------------------------------------------------------"
  exit 1
}

# ── Main ──────────────────────────────────────────────────────────
echo "=================================================================="
echo " Tumult demo check — mode: ${MODE}"
echo "=================================================================="

if [[ $STARMAP_ONLY -eq 1 ]]; then
  echo ""
  echo "Asserting the Star Map of the Krönika demo..."
  assert_starmap || record_fail
  finish
fi

if [[ $DO_WAIT -eq 1 ]]; then
  if ! wait_for_health; then
    [[ "$MODE" == "full" ]] && exit 1
    warn "continuing despite unhealthy services (populate mode)"
  fi
fi

echo ""
echo "Running fault sweep across ${#DOMAINS[@]} domains..."
for d in "${DOMAINS[@]}"; do
  if ! run_experiment "$d"; then
    [[ "$MODE" == "full" ]] && record_fail
  fi
done

if [[ "$MODE" == "full" ]]; then
  echo ""
  echo "Asserting telemetry landed..."
  assert_telemetry || record_fail
  echo ""
  echo "Asserting the Star Map of the Krönika demo..."
  assert_starmap || record_fail
fi

if [[ "$MODE" == "populate" ]]; then
  echo ""
  echo "------------------------------------------------------------------"
  info "populate mode — dashboards warmed; not asserting (exit 0)"
  echo "------------------------------------------------------------------"
  exit 0
fi

finish
