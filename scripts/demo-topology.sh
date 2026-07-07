#!/usr/bin/env bash
# Topology/compliance demo proofs. Runs against the ONE demo environment
# (docker-compose.demo.yml stack must be up: `make demo`).
#
# Usage: scripts/demo-topology.sh [1|2|3|all]
#
# 1  green lineage: latency drill on demo-app evidences DORA Art. 25
# 2  break + attribution: guard-halt on demo-postgres breaks DORA Art. 11,
#    cause attributed to the pause-database fault
# 3  recommendation loop: recommender flags the NIS2 BC/DR gap, the
#    recommended run closes it, the map flips — captured before/after
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
export TUMULT_ANALYTICS_PATH="$STORE"
PROOF=demo/proof/topology
BIN="${TUMULT_BIN:-./target/debug/tumult}"
export TUMULT_NET_PROXYD="${TUMULT_NET_PROXYD:-$PWD/target/debug/tumult-net-proxyd}"
mkdir -p "$PROOF"

step() { printf '\n=== %s ===\n' "$*"; }

import_topology() {
  "$BIN" topology import demo/topology/topology.toml --store "$STORE"
}

demo1() {
  step "demo 1: green lineage (DORA Art. 25 on demo-app)"
  import_topology
  "$BIN" run demo/experiments/demo-net.toon
  "$BIN" topology map --store "$STORE" --framework DORA > "$PROOF/1-map-after-green.txt"
  # The scoped view is the story shot: one control, evidence visible.
  "$BIN" topology map --store "$STORE" --framework DORA --control "Art.25" --format mermaid > "$PROOF/1-map-after-green.mmd"
  "$BIN" topology lineage --store "$STORE" --framework DORA --format json > "$PROOF/1-lineage.json"
  grep -q "evidenced" "$PROOF/1-map-after-green.txt" && echo "PROOF 1 OK: Art.25 evidenced on demo-app"
}

demo2() {
  step "demo 2: break + attribution (guard halt on demo-postgres)"
  import_topology
  "$BIN" run demo/experiments/demo-guard-halt.toon || true  # halt = non-zero exit, expected
  "$BIN" topology map --store "$STORE" --framework DORA > "$PROOF/2-map-after-break.txt"
  "$BIN" topology map --store "$STORE" --framework DORA --format mermaid > "$PROOF/2-map-after-break.mmd"
  "$BIN" topology lineage --store "$STORE" --framework DORA --format json > "$PROOF/2-lineage.json"
  grep -q "BROKEN" "$PROOF/2-map-after-break.txt" && echo "PROOF 2 OK: break visible on map"
  grep -q "pause-database" "$PROOF/2-lineage.json" && echo "PROOF 2 OK: cause attributed to pause-database"
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
  "$BIN" run demo/experiments/demo-topo-recommended.toon
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

case "${1:-all}" in
  1) demo1;; 2) demo2;; 3) demo3;;
  all) rm -f "$STORE"; demo1; demo2; demo3;;
  *) echo "usage: $0 [1|2|3|all]"; exit 2;;
esac
