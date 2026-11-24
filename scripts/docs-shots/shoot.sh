#!/usr/bin/env bash
# Take every Krönika screenshot of the docs again, and record the terminal
# demo of the site. See README.md next to this file.
#
# Usage: scripts/docs-shots/shoot.sh [out dir [cast file]]
#        scripts/docs-shots/shoot.sh --down   (after DOCS_SHOTS_KEEP=1)
#
# Builds the daemon image from this checkout, starts it as a compose project
# of its own on loopback ports, runs the demo seed, adds the data of
# data.py, writes one PNG per entry of shots.json into the out dir
# (default docs/images/3.0/screenshots) and records cast.py into the cast
# file (default docs/demo/tumult-demo.cast). At the end the stack goes down
# with its volumes; the image stays as a build cache.
#
# Env:
#   DOCS_SHOTS_PROJECT      compose project, container prefix and image name
#                           (default tumult-docs-shots)
#   DOCS_SHOTS_GRPC_PORT    loopback OTLP gRPC port (default 34317)
#   DOCS_SHOTS_HTTP_PORT    loopback API and web UI port (default 34318)
#   DOCS_SHOTS_TARGET_PORT  loopback port of the checkout service (default 34380)
#   DOCS_SHOTS_NO_BUILD=1   use the image as it is, without a build
#   DOCS_SHOTS_BUILD_MEMORY memory cap of the image build (default 12G)
#   DOCS_SHOTS_BUILD_CPUS   CPUs the image build may use (default 0-7)
#   DOCS_SHOTS_KEEP=1       leave the stack and the work dir in place, to
#                           take single pictures again with shoot.mjs
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
PROJECT=${DOCS_SHOTS_PROJECT:-tumult-docs-shots}
GRPC_PORT=${DOCS_SHOTS_GRPC_PORT:-34317}
HTTP_PORT=${DOCS_SHOTS_HTTP_PORT:-34318}
TARGET_PORT=${DOCS_SHOTS_TARGET_PORT:-34380}
KEEP=${DOCS_SHOTS_KEEP:-0}
NO_BUILD=${DOCS_SHOTS_NO_BUILD:-0}
BUILD_MEMORY=${DOCS_SHOTS_BUILD_MEMORY:-12G}
BUILD_CPUS=${DOCS_SHOTS_BUILD_CPUS:-0-7}

dc() { docker compose -p "$PROJECT" -f "$ROOT/docker/docker-compose.kronika.yml" "$@"; }
step() { printf '== docs-shots: %s\n' "$*"; }
fail() {
  printf 'docs-shots: %s\n' "$*" >&2
  exit 1
}
# The image build with a cap on memory and CPUs, so a Rust build cannot take
# the host down: the classic builder honours --memory, and a user scope of
# systemd caps the docker client too where there is one.
build() {
  local scope=()
  if systemd-run --user --scope -q true 2>/dev/null; then
    scope=(systemd-run --user --scope -q -p "MemoryMax=$BUILD_MEMORY")
  fi
  DOCKER_BUILDKIT=0 "${scope[@]}" docker build --memory "$BUILD_MEMORY" \
    --memory-swap "$BUILD_MEMORY" --cpuset-cpus "$BUILD_CPUS" \
    -f "$ROOT/docker/Dockerfile.tumultd" -t "$KRONIKA_IMAGE" "$ROOT"
}

if [ "${1:-}" = --down ]; then
  # The compose file needs the three values to parse; down uses none of them.
  export KRONIKA_BOOTSTRAP_ADMIN_PASSWORD=- KRONIKA_BOOTSTRAP_TOKEN=- KRONIKA_INGEST_TOKEN=-
  dc down -v --remove-orphans
  exit
fi
OUT=$(mkdir -p "${1:-$ROOT/docs/images/3.0/screenshots}" && cd "${1:-$ROOT/docs/images/3.0/screenshots}" && pwd)
CAST=$(realpath -m "${2:-$ROOT/docs/demo/tumult-demo.cast}")

for tool in docker node python3 curl oha; do
  command -v "$tool" >/dev/null || fail "$tool is not installed"
done
[ -f "$ROOT/web/node_modules/playwright/index.mjs" ] ||
  fail "no Playwright: run npm ci and npx playwright install chromium in web"
for port in "$GRPC_PORT" "$HTTP_PORT" "$TARGET_PORT"; do
  if curl -s -o /dev/null "http://127.0.0.1:$port/"; then
    fail "port $port answers already; set DOCS_SHOTS_*_PORT"
  fi
done

# The work dir holds the tokens of this run, the CLI and its private HOME.
WORK=$(mktemp -d "${TMPDIR:-/tmp}/docs-shots.XXXXXX")
chmod 700 "$WORK"
secret() { python3 -c 'import secrets; print(secrets.token_hex(24))'; }

# Tokens are made for this run and live only in the environment and the
# work dir; the compose file reads them from the environment.
export KRONIKA_NAME_PREFIX=$PROJECT KRONIKA_IMAGE=$PROJECT:latest
export KRONIKA_HOST_GRPC_PORT=$GRPC_PORT KRONIKA_HOST_HTTP_PORT=$HTTP_PORT
KRONIKA_BOOTSTRAP_ADMIN_PASSWORD=$(secret)
KRONIKA_BOOTSTRAP_TOKEN=kro_docs_seed_$(secret)
KRONIKA_INGEST_TOKEN=kro_docs_ingest_$(secret)
export KRONIKA_BOOTSTRAP_ADMIN_PASSWORD KRONIKA_BOOTSTRAP_TOKEN KRONIKA_INGEST_TOKEN

finish() {
  local status=$?
  if [ "$KEEP" = 1 ]; then
    step "the stack stays up; work dir $WORK"
    exit "$status"
  fi
  step "take the stack down"
  dc down -v --remove-orphans || status=1
  rm -rf "$WORK"
  exit "$status"
}
trap finish EXIT

if [ "$NO_BUILD" != 1 ]; then
  step "build the image $KRONIKA_IMAGE from this checkout"
  build
fi
step "start the daemon on 127.0.0.1:$HTTP_PORT"
dc up -d --wait tumultd
step "run the demo seed"
dc run --rm seed

step "copy the tumult CLI out of the image"
mkdir -p "$WORK/bin"
dc cp tumultd:/usr/local/bin/tumult "$WORK/bin/tumult"

python3 - "$WORK/stack.json" <<EOF
import json, os, sys
stack = {
    "url": "http://127.0.0.1:$HTTP_PORT",
    "grpc": "http://127.0.0.1:$GRPC_PORT",
    "container": "$PROJECT-tumultd",
    "target_port": $TARGET_PORT,
    "tumult": "$WORK/bin/tumult",
    "admin_password": os.environ["KRONIKA_BOOTSTRAP_ADMIN_PASSWORD"],
    "token": os.environ["KRONIKA_BOOTSTRAP_TOKEN"],
    "ingest_token": os.environ["KRONIKA_INGEST_TOKEN"],
}
with open(os.open(sys.argv[1], os.O_WRONLY | os.O_CREAT, 0o600), "w") as out:
    json.dump(stack, out)
EOF

step "add the data the pictures show"
python3 "$HERE/data.py" "$WORK"
step "take the pictures into $OUT"
node "$HERE/shoot.mjs" "$WORK" "$OUT"
step "record the terminal demo into $CAST"
python3 "$HERE/cast.py" "$WORK" "$CAST"
step "done"
