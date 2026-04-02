#!/bin/sh
# Tumult — one-command setup
#
# Usage:
#   curl -sSL https://raw.githubusercontent.com/mwigge/tumult/main/install.sh | sh
#
# Or locally:
#   ./install.sh
#
# What this does:
#   1. Clones the repo (if not already in it)
#   2. Builds the tumult binary (requires Rust toolchain)
#   3. Starts the Docker infrastructure (requires Docker)
#   4. Extracts the SSH test key
#   5. Runs a sample experiment to verify everything works
#
# After install, run:
#   tumult init              — create a new experiment interactively
#   tumult run experiment.toon — run an experiment
#   tumult discover          — list available plugins and actions

set -eu

REPO_URL="https://github.com/mwigge/tumult.git"
BINARY_NAME="tumult"

# ── Colors ─────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
NC='\033[0m'

info()  { printf "${BLUE}[tumult]${NC} %s\n" "$1"; }
ok()    { printf "${GREEN}[tumult]${NC} %s\n" "$1"; }
warn()  { printf "${YELLOW}[tumult]${NC} %s\n" "$1"; }
fail()  { printf "${RED}[tumult]${NC} %s\n" "$1"; exit 1; }

# ── Prerequisites ──────────────────────────────────────────────

info "Checking prerequisites..."

command -v git >/dev/null 2>&1 || fail "git is required. Install it first."
command -v cargo >/dev/null 2>&1 || fail "Rust toolchain required. Install from https://rustup.rs"
command -v docker >/dev/null 2>&1 || fail "Docker is required. Install Docker Desktop or Colima."

# Check Docker is running
docker info >/dev/null 2>&1 || fail "Docker daemon is not running. Start Docker Desktop or 'colima start'."

ok "Prerequisites: git, cargo, docker — all present"

# ── Clone or detect repo ───────────────────────────────────────

if [ -f "Cargo.toml" ] && grep -q "tumult" Cargo.toml 2>/dev/null; then
    info "Already in Tumult repo"
    TUMULT_DIR="$(pwd)"
else
    info "Cloning Tumult..."
    git clone "$REPO_URL" tumult
    TUMULT_DIR="$(pwd)/tumult"
    cd "$TUMULT_DIR"
fi

# ── Build ──────────────────────────────────────────────────────

info "Building tumult (release mode)..."
cargo build --release -p tumult-cli 2>&1 | tail -3

TUMULT_BIN="$TUMULT_DIR/target/release/$BINARY_NAME"
if [ ! -f "$TUMULT_BIN" ]; then
    fail "Build failed — binary not found at $TUMULT_BIN"
fi
ok "Built: $TUMULT_BIN"

# ── Install binary ─────────────────────────────────────────────

INSTALL_DIR="/usr/local/bin"
if [ -w "$INSTALL_DIR" ]; then
    cp "$TUMULT_BIN" "$INSTALL_DIR/$BINARY_NAME"
    ok "Installed to $INSTALL_DIR/$BINARY_NAME"
else
    warn "Cannot write to $INSTALL_DIR — run: sudo cp $TUMULT_BIN $INSTALL_DIR/$BINARY_NAME"
    warn "Or add target/release to your PATH"
    export PATH="$TUMULT_DIR/target/release:$PATH"
fi

# ── Start Docker infrastructure ────────────────────────────────

info "Starting chaos targets + observability..."
make up-targets 2>&1 | tail -5

ok "Docker targets started"

# ── Extract SSH key ────────────────────────────────────────────

info "Extracting SSH test key..."
sleep 5
make ssh-key 2>/dev/null || warn "SSH key extraction failed — sshd container may still be starting"

# ── Verify ─────────────────────────────────────────────────────

info "Running verification experiment..."
"$TUMULT_BIN" run experiment.toon 2>&1 | tail -4

echo ""
ok "========================================="
ok "  Tumult is ready!"
ok "========================================="
echo ""
info "Quick start:"
echo "  tumult discover                       — list plugins and actions"
echo "  tumult run examples/redis-chaos.toon  — run Redis chaos experiment"
echo "  tumult run examples/postgres-failover.toon — run PG failover test"
echo "  tumult analyze --query 'SELECT * FROM experiments' — SQL analytics"
echo "  tumult init                           — create your own experiment"
echo ""
info "Docker targets:"
echo "  PostgreSQL:  localhost:15432  (tumult/tumult_test)"
echo "  Redis:       localhost:16379"
echo "  Kafka:       localhost:19092"
echo "  SSH:         localhost:12222  (key: /tmp/tumult-test-key)"
echo ""
info "Docs: https://mwigge.github.io/tumult/"
echo ""
