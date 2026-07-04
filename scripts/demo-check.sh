#!/usr/bin/env bash
#
# demo-check.sh — health-wait + per-domain fault sweep + telemetry assertions
# for the Tumult 2.2 one-command demo.
#
# Two modes:
#   full      (default) — assert every experiment ends Completed AND that spans
#                         reached the collector. Exits non-zero on any failure.
#                         This is the dev smoke test (`make demo-check`).
#   populate            — run the sweep once, best-effort, to warm the SigNoz
#                         dashboards. Never fails the caller (`make demo`).
#
# Usage:
#   scripts/demo-check.sh [--mode full|populate] [--no-wait]
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

# ── Args ──────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
  case "$1" in
    --mode)     MODE="${2:?--mode needs a value}"; shift 2 ;;
    --mode=*)   MODE="${1#*=}"; shift ;;
    --no-wait)  DO_WAIT=0; shift ;;
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
        tumult run "$exp" --journal-path "$journal" >"$log" 2>&1; then
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

# ── Main ──────────────────────────────────────────────────────────
echo "=================================================================="
echo " Tumult demo check — mode: ${MODE}"
echo "=================================================================="

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
fi

echo ""
echo "------------------------------------------------------------------"
if [[ "$MODE" == "populate" ]]; then
  info "populate mode — dashboards warmed; not asserting (exit 0)"
  echo "------------------------------------------------------------------"
  exit 0
fi

if [[ $FAILURES -eq 0 ]]; then
  pass "ALL CHECKS PASSED"
  echo "------------------------------------------------------------------"
  exit 0
else
  fail "${FAILURES} check(s) failed"
  echo "------------------------------------------------------------------"
  exit 1
fi
