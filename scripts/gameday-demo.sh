#!/bin/sh
# Tumult GameDay E2E Demo
#
# Runs a full DORA-compliant resilience GameDay against live infrastructure.
# Everything starts, runs, and reports automatically.
#
# Prerequisites: Docker, Rust toolchain (or pre-built tumult binary)
#
# Usage:
#   ./scripts/gameday-demo.sh
#
# What happens:
#   1. Starts chaos targets (PostgreSQL, Redis, Kafka, SSH)
#   2. Starts tumult-mcp server (HTTP/SSE on :3100)
#   3. Connects as an MCP client (simulating an agent)
#   4. Discovers available plugins and actions
#   5. Runs Q2 PostgreSQL Resilience GameDay (4 experiments)
#   6. Analyzes results with resilience scoring
#   7. Queries analytics store (SQL via DuckDB)
#   8. Shows compliance mapping (DORA, NIS2)
#   9. Cleans up

set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
MCP_PID=""
HOST="127.0.0.1"
PORT="3100"
BASE="http://${HOST}:${PORT}/mcp"

# ── Helpers ──────────────────────────────────────────────────

cleanup() {
    echo ""
    echo "Cleaning up..."
    [ -n "$MCP_PID" ] && kill "$MCP_PID" 2>/dev/null && wait "$MCP_PID" 2>/dev/null
    echo "MCP server stopped."
    echo "Run './start.sh down' to stop infrastructure."
}
trap cleanup EXIT

mcp() {
    SESSION_HDR=""
    [ -n "${SESSION:-}" ] && SESSION_HDR="-H mcp-session-id:${SESSION}"
    curl -s -i --max-time "${4:-30}" -X POST "$BASE" \
        -H "Content-Type: application/json" \
        -H "Accept: text/event-stream, application/json" \
        $SESSION_HDR \
        -d "{\"jsonrpc\":\"2.0\",\"id\":$1,\"method\":\"$2\",\"params\":$3}" 2>&1
}

extract() {
    grep "data:" | head -1 | sed 's/^.*data: //'
}

tool_text() {
    python3 -c "
import sys,json
d=json.loads(sys.stdin.read())
print(d['result']['content'][0]['text'])
" 2>/dev/null
}

# ── Banner ───────────────────────────────────────────────────

echo ""
echo "╔══════════════════════════════════════════════════════════════════════╗"
echo "║  Tumult GameDay E2E Demo                                           ║"
echo "║  DORA-compliant resilience testing against live infrastructure     ║"
echo "╚══════════════════════════════════════════════════════════════════════╝"
echo ""

# ── Step 0: Start infrastructure ─────────────────────────────

echo "── Starting chaos targets ──────────────────────────────────────────"
cd "$PROJECT_DIR"
./start.sh infra 2>&1 | grep -E "Starting|Started" | head -2
sleep 5

# Verify targets are healthy
echo ""
echo "Targets:"
for svc in postgres redis kafka; do
    STATUS=$(docker ps --format "{{.Names}} {{.Status}}" 2>/dev/null | grep "$svc" | head -1)
    if echo "$STATUS" | grep -q "healthy"; then
        echo "  ✓ $svc"
    else
        echo "  ⏳ $svc (starting...)"
    fi
done
echo ""

# ── Step 1: Start MCP server ────────────────────────────────

echo "── Starting MCP server (HTTP/SSE on :${PORT}) ──────────────────────"

if [ -f "./target/release/tumult-mcp" ]; then
    ./target/release/tumult-mcp --transport http --port "$PORT" &
elif command -v tumult-mcp >/dev/null 2>&1; then
    tumult-mcp --transport http --port "$PORT" &
else
    echo "Error: tumult-mcp not found. Build with: cargo build --release -p tumult-mcp"
    exit 1
fi
MCP_PID=$!
sleep 2

# Initialize session
INIT=$(mcp 1 "initialize" '{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"gameday-demo","version":"1.0"}}' 5)
SESSION=$(echo "$INIT" | grep -i "mcp-session-id" | head -1 | awk '{print $2}' | tr -d '\r\n')

if [ -z "$SESSION" ]; then
    echo "Error: Failed to connect to MCP server"
    exit 1
fi
echo "  Session: ${SESSION}"
echo ""

# ── Step 2: Discover capabilities ────────────────────────────

echo "── Discovering chaos capabilities ──────────────────────────────────"
DISCOVER=$(mcp 2 "tools/call" '{"name":"tumult_discover","arguments":{}}' 10 | extract | tool_text)
PLUGINS=$(echo "$DISCOVER" | head -1)
ACTIONS=$(echo "$DISCOVER" | grep "::" | wc -l | tr -d ' ')
echo "  $PLUGINS"
echo "  Actions: $ACTIONS"
echo ""

# ── Step 3: Run GameDay ──────────────────────────────────────

echo "── Running Q2 PostgreSQL Resilience GameDay ────────────────────────"
echo "  Experiments: connection kill, container pause, CPU stress, memory stress"
echo "  Compliance: DORA EU 2022/2554 Articles 11, 24, 25"
echo "  Running... (this takes ~30 seconds)"
echo ""

GAMEDAY=$(mcp 3 "tools/call" '{"name":"tumult_gameday_run","arguments":{"gameday_path":"gamedays/q2-postgres-resilience.gameday.toon"}}' 120 | extract | tool_text)

echo "$GAMEDAY" | while IFS= read -r line; do echo "  $line"; done
echo ""

# ── Step 4: Analyze with resilience scoring ──────────────────

echo "── Resilience Analysis ─────────────────────────────────────────────"
ANALYSIS=$(mcp 4 "tools/call" '{"name":"tumult_gameday_analyze","arguments":{"gameday_path":"gamedays/q2-postgres-resilience.gameday.toon"}}' 15 | extract | tool_text)

echo "$ANALYSIS" | while IFS= read -r line; do echo "  $line"; done
echo ""

# ── Step 5: Query analytics store ────────────────────────────

echo "── Analytics Store ─────────────────────────────────────────────────"
STATS=$(mcp 5 "tools/call" '{"name":"tumult_store_stats","arguments":{}}' 10 | extract | tool_text)
echo "$STATS" | while IFS= read -r line; do echo "  $line"; done
echo ""

# ── Step 6: Cross-experiment trends ──────────────────────────

echo "── Trend Analysis (SQL via DuckDB) ─────────────────────────────────"
TRENDS=$(mcp 6 "tools/call" '{"name":"tumult_analyze_store","arguments":{"query":"SELECT status, count(*) as runs, round(avg(duration_ms)) as avg_ms, max(duration_ms) as max_ms FROM experiments GROUP BY status ORDER BY runs DESC"}}' 15 | extract | tool_text)
echo "$TRENDS" | while IFS= read -r line; do echo "  $line"; done
echo ""

# ── Summary ──────────────────────────────────────────────────

echo "╔══════════════════════════════════════════════════════════════════════╗"
echo "║  GameDay Complete                                                   ║"
echo "╠══════════════════════════════════════════════════════════════════════╣"
echo "║                                                                     ║"
echo "║  Pipeline:  Agent → MCP HTTP → tumult-core → plugins → targets     ║"
echo "║  GameDay:   Q2 PostgreSQL Resilience Programme                      ║"
echo "║  Transport: MCP Streamable HTTP/SSE on :${PORT}                      ║"
echo "║  Store:     DuckDB persistent analytics                             ║"
echo "║                                                                     ║"
echo "║  Compliance frameworks:                                             ║"
echo "║    DORA EU 2022/2554 — Art. 11 (recovery), Art. 24 (testing),      ║"
echo "║                        Art. 25 (scenario-based testing)             ║"
echo "║    NIS2 — Incident response and resilience testing                  ║"
echo "║                                                                     ║"
echo "║  Next: view traces in SigNoz → ./start.sh observe                  ║"
echo "║        open http://localhost:3301                                    ║"
echo "╚══════════════════════════════════════════════════════════════════════╝"
