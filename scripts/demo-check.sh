#!/usr/bin/env bash
#
# demo-check.sh — health-wait + per-domain fault sweep + telemetry assertions
# for the Tumult 2.2 one-command demo.
#
# Two modes:
#   full      (default) — assert that the journal of every run says what
#                         demo/experiments/expectations.json expects AND that
#                         spans reached the collector, then assert the Star
#                         Map of the Krönika demo. Exits non-zero on any
#                         failure. This is the dev smoke test
#                         (`make demo-check`).
#   populate            — run the ten domains once, best-effort, to warm the
#                         SigNoz dashboards. Never fails the caller
#                         (`make demo`).
#
# Usage:
#   scripts/demo-check.sh [--mode full|populate] [--no-wait] [--starmap-only]
#
# The Star Map assertions (scripts/starmap-check.py) need a Krönika daemon
# that holds the Krönika demo. By default this script brings
# docker/docker-compose.kronika.yml up as its own compose project, with its
# own container names and image tag (tumult-demo-check-kronika), on a fresh
# volume with throwaway credentials and on host ports 24317/24318, so a
# Krönika demo that is already up is left alone. It runs the seed, compares
# each seed journal with demo/kronika/experiments/expectations.json, checks the
# map (suppressing one risk to show a suppressed one) and removes the project
# again, also when it is interrupted; the image stays as a build cache.
# The project, its names and its ports are the same for every run, so one
# check at a time has the stack: a second one waits for the first (up to
# STARMAP_LOCK_WAIT seconds, 900 by default) on a lock file,
# ${TMPDIR:-/tmp}/tumult-demo-check-kronika.lock, and touches nothing if it
# cannot have its turn. To check a daemon that is already running, set
# STARMAP_URL (and STARMAP_TOKEN when it asks for one): nothing is started,
# removed or written then, so the suppressed-risk line is skipped unless a
# risk is suppressed already. --starmap-only runs just this part, and with
# STARMAP_URL it is the one use of this script that does not need Docker.
#
# Assumes the demo stack is already `up` (the Makefile brings it up first).
# Runs each experiment inside the tumult-mcp container via `tumult run`, the
# same path the control panel exercises through the MCP run_experiment tool.
#
# Journals and run logs go to a directory of this run. It is removed at the
# end, unless a journal was not as expected: then it is kept and its path is
# printed.

set -euo pipefail

# ── Config ────────────────────────────────────────────────────────
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMPOSE="${COMPOSE_DEMO:-docker compose -f ${REPO_ROOT}/docker/docker-compose.demo.yml}"
MODE="full"
DO_WAIT=1

# The sweep runs what demo/experiments/expectations.json gives to the proof
# `demo-check`, in the order of that file. The same file says what the journal
# of each run must hold, so a run that halts by design is judged like any
# other: by its journal, not by the exit status of `tumult run`.
EXPECTATIONS="${REPO_ROOT}/demo/experiments/expectations.json"
JOURNAL_CHECK="${REPO_ROOT}/scripts/check-proof-journals.py"
# Not in the warm-up of `make demo`, which runs the ten domains only.
FULL_ONLY=" demo-guard-halt demo-topo-recommended demo-topo-blind "
SWEEP=()

# Journals and run logs of this check. See the header.
WORK=""
KEEP_WORK=0
JOURNAL_FAILS=0

# Containers that carry a healthcheck we wait on. demo-collector is excluded:
# its image is distroless (no in-container probe possible), so its readiness is
# instead proven by the span-throughput assertion at the end of the sweep.
HEALTH_CONTAINERS=(demo-postgres demo-app demo-sshd demo-signoz demo-mcp)

HEALTH_TIMEOUT="${HEALTH_TIMEOUT:-240}"
COLLECTOR_METRICS_URL="${COLLECTOR_METRICS_URL:-http://localhost:18888/metrics}"

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
KRONIKA_LOCK="${TMPDIR:-/tmp}/${KRONIKA_PROJECT}.lock"
KRONIKA_LOCKED=0
KRONIKA_WAITER=""
STARMAP_LOCK_WAIT="${STARMAP_LOCK_WAIT:-900}"
STARMAP_URL="${STARMAP_URL:-}"
STARMAP_WAIT="${STARMAP_WAIT:-90}"
STARMAP_ONLY=0
# What the journal of each experiment of the Krönika seed must hold.
SEED_EXPECTATIONS="${REPO_ROOT}/demo/kronika/experiments/expectations.json"

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
# A problem that fails the full check and is only a warning in a warm-up.
complain() {
  if [[ "$MODE" == "full" ]]; then fail "$1"; else warn "$1"; fi
}

# ── Work directory ────────────────────────────────────────────────
# Whatever this script started or made is removed on every way out, an
# interrupt included.
arm_cleanup() {
  trap cleanup EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
}

work_dir() {
  [[ -n "$WORK" ]] && return 0
  WORK="$(mktemp -d)" || return 1
  arm_cleanup
}

cleanup() {
  kronika_cleanup
  [[ -z "$WORK" || $KEEP_WORK -eq 1 ]] && return 0
  rm -rf "$WORK"
  WORK=""
}

# ── Journal check ─────────────────────────────────────────────────
# Compare the journals in a directory with what an expectations file says
# the proof $1 must find. A line of the check is `ok   name`, `FAIL name`,
# or a detail of the FAIL above it. $4 goes in front of each name. Sets
# JOURNAL_FAILS to the number of names that failed.
check_journals() {
  local proof="$1" expectations="$2" journals="$3" prefix="$4"
  local out line rc=0 fails=0
  out="$(python3 "$JOURNAL_CHECK" --proof "$proof" "$expectations" "$journals" 2>&1)" || rc=1
  while IFS= read -r line; do
    case "$line" in
      "ok   "*) pass "${prefix}${line#ok   }" ;;
      "FAIL "*) fail "${prefix}${line#FAIL }"; fails=$((fails + 1)) ;;
      *)        printf '        %s\n' "${line#     }" ;;
    esac
  done <<<"$out"
  if [[ $rc -eq 1 && $fails -eq 0 ]]; then
    fail "${prefix}the journal check did not run"
    fails=1
  fi
  JOURNAL_FAILS=$fails
  [[ $fails -eq 0 ]] && return 0
  KEEP_WORK=1
  return 1
}

# ── Docker ────────────────────────────────────────────────────────
# Everything here drives the demo through Docker, except a Star Map check
# of a daemon that is already running. Say so once, before anything runs,
# instead of once per step that cannot find the command.
require_docker() {
  [[ $STARMAP_ONLY -eq 1 && -n "$STARMAP_URL" ]] && return 0
  local -a tools=("${KRONIKA_COMPOSE[0]}")
  [[ $STARMAP_ONLY -eq 1 ]] || tools+=(docker "${COMPOSE%% *}")
  local tool
  for tool in "${tools[@]}"; do
    command -v "$tool" >/dev/null 2>&1 && continue
    fail "'${tool}' is not installed or not on PATH: this check runs the demo through Docker. Without Docker, only a Krönika daemon that is already running can be checked: STARMAP_URL=http://host:port $0 --starmap-only"
    return 1
  done
}

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
# Run one experiment in the demo and bring its journal out. The journal of
# an earlier run is removed first, so a run that writes none leaves none.
# -T: no TTY (safe under make / CI); stdin is not the caller's.
run_experiment() {
  local name="$1" rc=0 status
  local journal="/journals/${name}.journal.toon"
  local out="${WORK}/sweep/${name}"
  $COMPOSE exec -T tumult-mcp rm -f "$journal" </dev/null >/dev/null 2>&1 || true
  $COMPOSE exec -T tumult-mcp \
    tumult run "/demo/experiments/${name}.toon" --journal-path "$journal" --force \
    </dev/null >"${out}.log" 2>&1 || rc=$?
  if ! $COMPOSE exec -T tumult-mcp cat "$journal" </dev/null >"${out}.journal.toon" 2>/dev/null; then
    rm -f "${out}.journal.toon"
  fi
  status="$(sed -n 's/^Status: //p' "${out}.log" | head -1)"
  info "${name}  (${status:-no status}, exit ${rc})"
}

# Fill SWEEP with the names this mode runs, in the order of the expectations.
sweep_list() {
  local listed name
  if ! listed="$(python3 "$JOURNAL_CHECK" --proof demo-check --list "$EXPECTATIONS" 2>&1)"; then
    complain "sweep — nothing to run: ${listed##*$'\n'}"
    return 1
  fi
  SWEEP=()
  while IFS= read -r name; do
    name="${name%.toon}"
    [[ "$MODE" == "populate" && "$FULL_ONLY" == *" ${name} "* ]] && continue
    SWEEP+=("$name")
  done <<<"$listed"
}

sweep() {
  sweep_list || return 1
  if ! work_dir || ! mkdir -p "${WORK}/sweep"; then
    complain "sweep — cannot make a directory for the journals"
    return 1
  fi
  echo "Running the fault sweep (${#SWEEP[@]} experiments)..."
  local name
  for name in "${SWEEP[@]}"; do
    run_experiment "$name"
  done
}

# ── Telemetry assertion ───────────────────────────────────────────
# Assert the collector accepted spans. Its self-telemetry (published on host
# :18888) exposes otelcol_receiver_accepted_spans* — a value > 0 proves the
# experiment runs and demo-app traffic reached the collector.
assert_telemetry() {
  local metrics
  if ! metrics="$(curl -sf -m 8 "$COLLECTOR_METRICS_URL" 2>/dev/null)"; then
    fail "telemetry — could not scrape collector self-metrics at ${COLLECTOR_METRICS_URL}"
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

# Take the lock file on descriptor 9 if nobody holds it, waiting up to
# $1 seconds. The lock is on the open file, so it lasts until this script
# closes the descriptor or ends, however it ends: there is no stale lock
# to clear. python3 (needed for the check anyway) is used because flock(1)
# is not on every system this script runs on. It waits in the background
# so that an interrupt ends this script at once, not after the wait; the
# waiter gives up as soon as this script is gone.
kronika_try_lock() {
  python3 -c '
import fcntl, os, signal, sys, time
signal.signal(signal.SIGINT, signal.SIG_DFL)
deadline = time.monotonic() + float(sys.argv[1])
while os.getppid() == int(sys.argv[2]):
    try:
        fcntl.flock(9, fcntl.LOCK_EX | fcntl.LOCK_NB)
        sys.exit(0)
    except OSError:
        if time.monotonic() >= deadline:
            sys.exit(1)
        time.sleep(0.2)
sys.exit(1)
' "$1" "$$" >/dev/null 2>&1 &
  KRONIKA_WAITER=$!
  local got=0
  wait "$KRONIKA_WAITER" || got=$?
  KRONIKA_WAITER=""
  return $got
}

# One check at a time owns the check stack: without this a second run
# would remove the stack the first one is still using.
kronika_lock() {
  if ! exec 9>>"$KRONIKA_LOCK"; then
    fail "star map — cannot open the lock file ${KRONIKA_LOCK}"
    return 1
  fi
  if ! kronika_try_lock 0; then
    info "star map — waiting for another demo check to finish (up to ${STARMAP_LOCK_WAIT}s)"
    if ! kronika_try_lock "$STARMAP_LOCK_WAIT"; then
      exec 9>&-
      fail "star map — another demo check still holds ${KRONIKA_LOCK}; its stack was left alone"
      return 1
    fi
  fi
  KRONIKA_LOCKED=1
}

kronika_unlock() {
  [[ $KRONIKA_LOCKED -eq 1 ]] || return 0
  KRONIKA_LOCKED=0
  exec 9>&-
}

# Remove the check stack once, however the script ends, and let the next
# check have its turn.
kronika_cleanup() {
  if [[ -n "$KRONIKA_WAITER" ]]; then
    kill "$KRONIKA_WAITER" 2>/dev/null || true
    KRONIKA_WAITER=""
  fi
  if [[ $KRONIKA_STARTED -eq 1 ]]; then
    KRONIKA_STARTED=0
    kronika_down
  fi
  kronika_unlock
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
  # The daemon takes a bootstrap token only in the shape of its own tokens.
  KRONIKA_BOOTSTRAP_TOKEN="kro_$(throwaway_secret)"
  KRONIKA_INGEST_TOKEN="$(throwaway_secret)"
}

# Bring the Krönika demo up on a fresh volume, run its seed and compare the
# seed journals with what is expected of them. Sets STARMAP_URL and
# STARMAP_TOKEN for the daemon it started. From here on the stack is removed
# on every way out, an interrupt included. The seed does not run as this
# user, so the directory it writes its journals to is open to all.
kronika_up() {
  kronika_env
  arm_cleanup
  kronika_lock || return 1
  local seed_journals
  if ! work_dir || ! mkdir -p "${WORK}/seed" || ! chmod 777 "${WORK}/seed"; then
    fail "star map — cannot make a directory for the seed journals"
    return 1
  fi
  seed_journals="${WORK}/seed"
  local log; log="$(mktemp)"
  kronika_down
  KRONIKA_STARTED=1
  if ! "${KRONIKA_COMPOSE[@]}" up -d --build --wait tumultd >"$log" 2>&1; then
    fail "star map — the Krönika daemon did not come up"
    tail -12 "$log" | sed 's/^/        /'
    rm -f "$log"
    return 1
  fi
  if ! "${KRONIKA_COMPOSE[@]}" run --rm -v "${seed_journals}:/tmp/journals" seed >"$log" 2>&1; then
    fail "star map — the Krönika seed exited non-zero"
    tail -12 "$log" | sed 's/^/        /'
    rm -f "$log"
    return 1
  fi
  mv "$log" "${WORK}/seed.log"
  check_journals kronika-seed "$SEED_EXPECTATIONS" "$seed_journals" "seed — " || return 1
  pass "star map — Krönika demo seeded"
  STARMAP_URL="http://127.0.0.1:${KRONIKA_HOST_HTTP_PORT}"
  STARMAP_TOKEN="$KRONIKA_BOOTSTRAP_TOKEN"
}

# Print each line of the check as a PASS, a FAIL or a skipped line of this
# script. Fails if any line is not an `ok` or a `skip`.
report_starmap() {
  local line failed=0
  while IFS= read -r line; do
    case "$line" in
      "ok  "*) pass "${line#ok  }" ;;
      "skip "*) warn "${line#skip }" ;;
      *)       fail "${line#FAIL }"; failed=1 ;;
    esac
  done
  return $failed
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
    "$STARMAP_URL" --wait "$STARMAP_WAIT" ${write[@]+"${write[@]}"} 2>&1)" || rc=1
  if [[ -z "$out" ]]; then
    fail "star map — the check printed nothing"
    rc=1
  elif ! report_starmap <<<"$out"; then
    rc=1
  fi
  kronika_cleanup
  return $rc
}

finish() {
  echo ""
  echo "------------------------------------------------------------------"
  if [[ $KEEP_WORK -eq 1 ]]; then
    info "run logs and journals kept in ${WORK}"
  fi
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

if ! require_docker; then
  [[ "$MODE" == "populate" ]] && exit 0
  exit 1
fi

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
if [[ "$MODE" == "populate" ]]; then
  sweep || true
  echo ""
  echo "------------------------------------------------------------------"
  info "populate mode — dashboards warmed; not asserting (exit 0)"
  echo "------------------------------------------------------------------"
  exit 0
fi

if sweep; then
  echo ""
  echo "Checking the journal of each run against demo/experiments/expectations.json..."
  check_journals demo-check "$EXPECTATIONS" "${WORK}/sweep" "" \
    || FAILURES=$((FAILURES + JOURNAL_FAILS))
else
  record_fail
fi

echo ""
echo "Asserting telemetry landed..."
assert_telemetry || record_fail
echo ""
echo "Asserting the Star Map of the Krönika demo..."
assert_starmap || record_fail

finish
