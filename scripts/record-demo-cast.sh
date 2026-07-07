#!/usr/bin/env bash
# Record docs/demo/tumult-demo.cast from a real showcase run against the
# live demo stack. Wraps itself in the same user namespace the proof suite
# uses so container hostnames resolve from the host.
set -euo pipefail
cd "$(dirname "$0")/.."

if [ -z "${TUMULT_DEMO_NS:-}" ]; then
  ALIAS_DIR=demo/topology
  printf '127.0.0.1 localhost\n::1 localhost\n127.0.0.1 demo-app\n127.0.0.1 demo-postgres\n' > "$ALIAS_DIR/hosts"
  sed 's/^hosts:.*/hosts: files dns/' /etc/nsswitch.conf > "$ALIAS_DIR/nsswitch.conf"
  export TUMULT_DEMO_NS=1
  exec unshare -r -m sh -c "mount --bind '$PWD/$ALIAS_DIR/hosts' /etc/hosts && mount --bind '$PWD/$ALIAS_DIR/nsswitch.conf' /etc/nsswitch.conf && exec bash '$0'"
fi

export TUMULT_BIN=./target/debug/tumult
export TUMULT_NET_PROXYD="$PWD/target/debug/tumult-net-proxyd"
export TUMULT_ANALYTICS_PATH="$PWD/demo/proof/topology/cast-store.duckdb"
rm -f "$TUMULT_ANALYTICS_PATH"
python3 scripts/record-demo-cast.py docs/demo/tumult-demo.cast
