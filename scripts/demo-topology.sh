#!/usr/bin/env bash
# Topology/compliance demo proofs. Runs against the ONE demo environment
# (docker-compose.demo.yml stack must be up: `make demo`).
#
# Usage: scripts/demo-topology.sh [1|2|3|4|5|all]
#
# 1  green lineage: latency drill on demo-app evidences DORA Art. 25
# 2  break + attribution: guard-halt on demo-postgres breaks DORA Art. 11,
#    cause attributed to the pause-database fault
# 3  recommendation loop: recommender flags the NIS2 BC/DR gap, the
#    recommended run closes it, the map flips — captured before/after
# 4  autopilot: gated autonomous injection with lineage
# 5  pre-flight, enrollment, change events and OTel-derived criticality
set -euo pipefail
cd "$(dirname "$0")/.."

# The demo experiments address services by container name (demo-app,
# demo-postgres). When running the CLI on the host, re-exec inside a user
# namespace with a private /etc/hosts + nsswitch.conf mapping those names to
# the compose stack's published localhost ports — no sudo, no system change.
if [ -z "${TUMULT_DEMO_NS:-}" ]; then
  ALIAS_DIR=demo/topology
  printf '127.0.0.1 localhost\n::1 localhost\n127.0.0.1 demo-app\n127.0.0.1 demo-postgres\n' > "$ALIAS_DIR/hosts"
  sed 's/^hosts:.*/hosts: files dns/' /etc/nsswitch.conf > "$ALIAS_DIR/nsswitch.conf"
  export TUMULT_DEMO_NS=1
  exec unshare -r -m sh -c "mount --bind '$PWD/$ALIAS_DIR/hosts' /etc/hosts && mount --bind '$PWD/$ALIAS_DIR/nsswitch.conf' /etc/nsswitch.conf && exec bash '$0' $*"
fi

STORE="${TUMULT_TOPOLOGY_DEMO_STORE:-$PWD/demo/proof/topology/store.duckdb}"
# `tumult run` writes each run to the store the maps below read.
export TUMULT_LAKE_PATH="$STORE"
PROOF=demo/proof/topology
BIN="${TUMULT_BIN:-./target/debug/tumult}"
export TUMULT_NET_PROXYD="${TUMULT_NET_PROXYD:-$PWD/target/debug/tumult-net-proxyd}"
JOURNALS="$PROOF/journals"
mkdir -p "$PROOF" "$JOURNALS"

step() { printf '\n=== %s ===\n' "$*"; }

# One proof line: the command after the message must succeed. `set -e` does
# not see a failing `check && echo` line, so every check goes through here.
check() {
  local number=$1 message=$2
  shift 2
  if ! "$@"; then
    echo "PROOF ${number} FAIL: ${message}" >&2
    exit 1
  fi
  echo "PROOF ${number} OK: ${message}"
}

import_topology() {
  "$BIN" topology import demo/topology/topology.toml --store "$STORE"
}

# Run one demo experiment. Each keeps its own journal, and --force replaces
# the one of an earlier demo: the CLI does not write over a journal otherwise.
run_experiment() {
  "$BIN" run "demo/experiments/$1.toon" --journal-path "$JOURNALS/$1.journal.toon" --force
}

demo1() {
  step "demo 1: green lineage (DORA Art. 25 on demo-app)"
  import_topology
  run_experiment demo-net
  "$BIN" topology map --store "$STORE" --framework DORA > "$PROOF/1-map-after-green.txt"
  # The scoped view is the story shot: one control, evidence visible.
  "$BIN" topology map --store "$STORE" --framework DORA --control "Art.25" --format mermaid > "$PROOF/1-map-after-green.mmd"
  "$BIN" topology lineage --store "$STORE" --framework DORA --format json > "$PROOF/1-lineage.json"
  # The legend of a map holds the word too, so the check reads the cell line.
  check 1 "Art.25 evidenced on demo-app" grep -q "DORA/Art.25 evidenced" "$PROOF/1-map-after-green.txt"
}

demo2() {
  step "demo 2: break + attribution (guard halt on demo-postgres)"
  import_topology
  # The guard halts the run, so it ends non-zero. Its status line is the proof.
  run_output=$(run_experiment demo-guard-halt 2>&1) || true
  echo "$run_output"
  check 2 "the guard halted the run" grep -q "^Status: Halted" <<< "$run_output"
  "$BIN" topology map --store "$STORE" --framework DORA > "$PROOF/2-map-after-break.txt"
  "$BIN" topology map --store "$STORE" --framework DORA --format mermaid > "$PROOF/2-map-after-break.mmd"
  "$BIN" topology lineage --store "$STORE" --framework DORA --format json > "$PROOF/2-lineage.json"
  check 2 "break visible on map" grep -q "^\[BROKEN\] svc:demo-postgres" "$PROOF/2-map-after-break.txt"
  check 2 "cause attributed to pause-database" grep -q "pause-database" "$PROOF/2-lineage.json"
}

demo3() {
  step "demo 3: recommendation loop (NIS2 BC/DR gap -> closed)"
  import_topology
  "$BIN" topology recommend --store "$STORE" --format json --limit 6 > "$PROOF/3-recommendation.json"
  "$BIN" topology map --store "$STORE" --format mermaid > "$PROOF/3-map-before.mmd"
  python3 - "$PROOF/3-recommendation.json" << 'PY'
import json, sys
recs = json.load(open(sys.argv[1]))
recs = recs.get("recommendations", recs)
assert recs[0]["article_id"] == "compliance:DORA/Art.11", "top pick must be the broken control"
assert any("21(2)(c)" in r["article_id"] for r in recs), "NIS2 BC/DR gap must be flagged"
print("PROOF 3 OK: top pick = broken DORA/Art.11; NIS2 21(2)(c) gap flagged")
PY
  run_experiment demo-topo-recommended
  "$BIN" topology map --store "$STORE" > "$PROOF/3-map-after.txt"
  "$BIN" topology map --store "$STORE" --framework NIS2 --control "Art.21(2)(c)" --format mermaid > "$PROOF/3-map-after.mmd"
  "$BIN" topology lineage --store "$STORE" --framework NIS2 --format json > "$PROOF/3-lineage-after.json"
  python3 - "$PROOF/3-lineage-after.json" << 'PY'
import json, sys
cells = json.load(open(sys.argv[1]))
cells = cells.get("cells", cells)
cell = next(c for c in cells if "21(2)(c)" in c["article_id"] and c["service_id"] == "svc:demo-postgres")
assert str(cell["status"]).lower() == "evidenced", cell
print("PROOF 3 OK: NIS2 21(2)(c) on demo-postgres flipped untested -> evidenced")
PY
}

demo4() {
  step "demo 4: autopilot — gated autonomous injection with lineage"
  POLICY=demo/topology/autopilot.toml
  # Pass 1: the broken DORA/Art.11 on demo-postgres is the top candidate and
  # its class is operator-pretrusted -> the gate enacts and the recovery
  # playbook runs. Every OTHER candidate targeting demo-postgres is vetoed:
  # the service has an open break, and the only injection allowed into a
  # broken service is the revalidation of the break itself.
  "$BIN" autopilot once --policy "$POLICY" --execute --limit 4 --store "$STORE" \
    | tee "$PROOF/4-autopilot-pass1.txt"
  check 4 "gate enacted the pretrusted class" grep -q "\[enact\]" "$PROOF/4-autopilot-pass1.txt"
  check 4 "enacted run completed" grep -qiE "run_completed|-> completed" "$PROOF/4-autopilot-pass1.txt"
  check 4 "gate vetoed new faults into the broken service" grep -q "ambient.no_open_deviation" "$PROOF/4-autopilot-pass1.txt"

  # Pass 2, same policy, minutes later: the same top candidate now hits the
  # per-service cooldown -> downgraded to the human queue with the exact
  # rule recorded. A human denies it with a reason; the denial is feedback.
  "$BIN" autopilot once --policy "$POLICY" --execute --limit 2 --store "$STORE" \
    | tee "$PROOF/4-autopilot-pass2.txt"
  check 4 "cooldown downgraded the repeat to the human queue" grep -qE "\[(downgrade|propose)\].*cooldown" "$PROOF/4-autopilot-pass2.txt"

  "$BIN" autopilot status --store "$STORE" --format json > "$PROOF/4-status.json"
  DENY_ID=$(python3 - "$PROOF/4-status.json" << 'PY'
import json, sys
rows = json.load(open(sys.argv[1]))
rows = rows.get("decisions", rows)
for r in rows:
    if r["verdict"] in ("propose", "downgrade") and not r.get("last_event"):
        print(r["id"]); break
PY
)
  test -n "$DENY_ID"
  "$BIN" autopilot deny "$DENY_ID" --reason "demo: not this quarter" --store "$STORE"
  "$BIN" autopilot status --store "$STORE" --format json > "$PROOF/4-status-after-deny.json"
  check 4 "human veto recorded as feedback" grep -q "human_denied" "$PROOF/4-status-after-deny.json"

  # Lineage: decisions are graph citizens; archive: parquet cold store.
  "$BIN" chaosgraph query --kind recommendation --store "$STORE" > "$PROOF/4-graph-recommendations.txt"
  check 4 "decisions are graph nodes" grep -q "rec:" "$PROOF/4-graph-recommendations.txt"
  "$BIN" autopilot export "$PROOF/parquet" --store "$STORE"
  check 4 "parquet archive written" test -f "$PROOF/parquet/autopilot_decisions.parquet"
}

demo5() {
  step "demo 5: pre-flight, enrollment, change events, OTel criticality"
  # 5a. Dynamic guard-telemetry pre-flight: the blind-guard playbook's probe
  # hits a dead endpoint; the pre-flight actually RUNS the probe and the
  # gate refuses autonomy for a blast it cannot see.
  "$BIN" autopilot once --policy demo/topology/autopilot-blind.toml --limit 1 --store "$STORE" \
    | tee "$PROOF/5-blind-guard.txt"
  check 5 "blind guard caught by dynamic pre-flight" grep -qE "guard.telemetry|telemetry" "$PROOF/5-blind-guard.txt"

  # 5b. Enrollment (structural consent): demo-postgres is not enrolled in
  # this policy variant — injection is structurally impossible, verdict veto.
  "$BIN" autopilot once --policy demo/topology/autopilot-unenrolled.toml --limit 1 --store "$STORE" \
    | tee "$PROOF/5-unenrolled.txt"
  check 5 "unenrolled target vetoed" grep -q "target.enrolled" "$PROOF/5-unenrolled.txt"

  # 5c. Change-event trigger: a deploy notification invalidates demo-app's
  # evidence; the next pass carries a change_event-triggered decision.
  "$BIN" autopilot notify-change --service demo-app --source deploy-webhook \
    --detail "demo: image tag bumped" --store "$STORE"
  "$BIN" autopilot once --policy demo/topology/autopilot.toml --limit 4 --store "$STORE" \
    | tee "$PROOF/5-change-event-pass.txt"
  "$BIN" autopilot status --store "$STORE" --format json > "$PROOF/5-status.json"
  check 5 "change event produced a decision" grep -q "change_event" "$PROOF/5-status.json"

  # 5d. OTel-derived criticality: real span counts from the demo's SigNoz
  # ClickHouse, handed to the recommender as a reviewable artifact.
  docker exec demo-signoz clickhouse client -q \
    "SELECT serviceName, count() FROM signoz_traces.distributed_signoz_index_v3 WHERE timestamp > now() - INTERVAL 60 MINUTE GROUP BY serviceName FORMAT JSON" \
    | python3 -c "import json,sys; d=json.load(sys.stdin); print(json.dumps({r['serviceName']: float(r['count()']) for r in d['data']}))" \
    > "$PROOF/5-criticality.json"
  TUMULT_CRITICALITY_FILE="$PROOF/5-criticality.json" \
    "$BIN" topology recommend --limit 6 --format json --store "$STORE" > "$PROOF/5-recommend-critical.json"
  check 5 "OTel span rates weighted the recommendation" grep -q "observed traffic" "$PROOF/5-recommend-critical.json"
}

case "${1:-all}" in
  1) demo1;; 2) demo2;; 3) demo3;; 4) demo4;; 5) demo5;;
  all) rm -f "$STORE"; demo1; demo2; demo3; demo4; demo5;;
  *) echo "usage: $0 [1|2|3|4|5|all]"; exit 2;;
esac
