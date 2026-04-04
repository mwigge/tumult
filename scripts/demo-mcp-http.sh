#!/bin/sh
# Demo: Tumult MCP Server over HTTP/SSE
#
# Shows how an external agent (like AQE) can connect to tumult-mcp
# over HTTP to run chaos experiments, list tools, and analyze results.
#
# Prerequisites:
#   ./start.sh infra     # starts PG, Redis, Kafka, SSH
#   tumult-mcp --transport http --port 3100
#
# Usage:
#   ./scripts/demo-mcp-http.sh [host] [port]

set -eu

HOST="${1:-127.0.0.1}"
PORT="${2:-3100}"
BASE="http://${HOST}:${PORT}/mcp"

# ── Helpers ──────────────────────────────────────────────────

mcp_call() {
    ID="$1"
    METHOD="$2"
    PARAMS="$3"
    SESSION="${4:-}"

    HEADERS="-H 'Content-Type: application/json' -H 'Accept: text/event-stream, application/json'"
    if [ -n "$SESSION" ]; then
        HEADERS="$HEADERS -H 'mcp-session-id: $SESSION'"
    fi

    eval "curl -s -i $HEADERS -X POST '$BASE' \
        -d '{\"jsonrpc\":\"2.0\",\"id\":$ID,\"method\":\"$METHOD\",\"params\":$PARAMS}'"
}

extract_session() {
    grep -i "mcp-session-id" | head -1 | awk '{print $2}' | tr -d '\r\n'
}

extract_data() {
    grep "^data:" | head -1 | sed 's/^data: //'
}

# ── Step 1: Initialize ──────────────────────────────────────

echo "=== Step 1: Initialize MCP session ==="
INIT=$(mcp_call 1 "initialize" '{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"demo","version":"0.1"}}')
SESSION=$(echo "$INIT" | extract_session)
echo "Session: $SESSION"
echo "$INIT" | extract_data | python3 -m json.tool 2>/dev/null || echo "$INIT" | extract_data
echo ""

# ── Step 2: List tools ───────────────────────────────────────

echo "=== Step 2: List available tools ==="
TOOLS=$(mcp_call 2 "tools/list" '{}' "$SESSION")
echo "$TOOLS" | extract_data | python3 -c "
import sys, json
d = json.loads(sys.stdin.read())
tools = d['result']['tools']
print(f'Available tools: {len(tools)}')
for t in tools:
    print(f'  - {t[\"name\"]}: {t.get(\"description\",\"\")[:60]}')
" 2>/dev/null
echo ""

# ── Step 3: Discover plugins ────────────────────────────────

echo "=== Step 3: Discover chaos plugins ==="
DISCOVER=$(mcp_call 3 "tools/call" '{"name":"tumult_discover","arguments":{}}' "$SESSION")
echo "$DISCOVER" | extract_data | python3 -c "
import sys, json
d = json.loads(sys.stdin.read())
text = d['result']['content'][0]['text']
lines = text.strip().split('\n')
for line in lines[:20]:
    print(f'  {line}')
if len(lines) > 20:
    print(f'  ... ({len(lines)} total lines)')
" 2>/dev/null
echo ""

# ── Step 4: Run a chaos experiment ──────────────────────────

echo "=== Step 4: Run postgres-failover experiment ==="
RUN=$(mcp_call 4 "tools/call" '{"name":"tumult_run_experiment","arguments":{"experiment_path":"examples/postgres-failover.toon"}}' "$SESSION")
echo "$RUN" | extract_data | python3 -c "
import sys, json
d = json.loads(sys.stdin.read())
text = d['result']['content'][0]['text']
# Show first 30 lines
lines = text.strip().split('\n')
for line in lines[:30]:
    print(f'  {line}')
" 2>/dev/null
echo ""

# ── Step 5: List journals ───────────────────────────────────

echo "=== Step 5: List experiment journals ==="
JOURNALS=$(mcp_call 5 "tools/call" '{"name":"tumult_list_journals","arguments":{}}' "$SESSION")
echo "$JOURNALS" | extract_data | python3 -c "
import sys, json
d = json.loads(sys.stdin.read())
text = d['result']['content'][0]['text']
print(text[:500])
" 2>/dev/null
echo ""

# ── Step 6: Analyze with SQL ────────────────────────────────

echo "=== Step 6: Analyze journals with SQL ==="
ANALYZE=$(mcp_call 6 "tools/call" '{"name":"tumult_analyze","arguments":{"query":"SELECT title, status, duration_secs FROM journals ORDER BY start_time DESC LIMIT 5"}}' "$SESSION")
echo "$ANALYZE" | extract_data | python3 -c "
import sys, json
d = json.loads(sys.stdin.read())
text = d['result']['content'][0]['text']
print(text[:500])
" 2>/dev/null
echo ""

echo "=== Demo complete ==="
echo "MCP server: $BASE"
echo "Session: $SESSION"
