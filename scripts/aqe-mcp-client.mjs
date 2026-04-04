#!/usr/bin/env node
/**
 * Tumult MCP Client — demonstrates how AQE connects to tumult-mcp
 *
 * This is the integration pattern that AQE's chaos-engineer agent uses
 * to run experiments, GameDays, and analytics via Tumult's MCP server.
 *
 * Prerequisites:
 *   ./start.sh infra                        # chaos targets
 *   tumult-mcp --transport http --port 3100  # MCP server
 *
 * Usage:
 *   node scripts/aqe-mcp-client.mjs [command]
 *
 * Commands:
 *   discover     — list plugins and actions
 *   experiment   — run a single PG failover experiment
 *   gameday      — run the full Q2 PostgreSQL Resilience GameDay
 *   analyze      — analyze the latest GameDay results
 *   sql <query>  — run SQL against the analytics store
 *   demo         — run all steps (default)
 */

const BASE = process.env.TUMULT_MCP_URL || 'http://localhost:3100/mcp';
const TOKEN = process.env.TUMULT_MCP_TOKEN || '';

// ── MCP Client ──────────────────────────────────────────────

class TumultMCPClient {
  #session = null;
  #id = 0;

  async initialize() {
    const res = await this.#request('initialize', {
      protocolVersion: '2025-11-25',
      capabilities: {},
      clientInfo: { name: 'aqe-chaos-engineer', version: '1.0' },
    });
    this.#session = res.headers.get('mcp-session-id');
    const data = await this.#extractData(res);
    return { session: this.#session, server: data.result.serverInfo };
  }

  async listTools() {
    const data = await this.#call('tools/list', {});
    return data.result.tools;
  }

  async callTool(name, args = {}) {
    const data = await this.#call('tools/call', { name, arguments: args });
    if (data.result?.isError) throw new Error(data.result.content[0]?.text);
    return data.result.content[0]?.text || '';
  }

  // Convenience methods matching AQE's chaos-engineer interface
  async discover() { return this.callTool('tumult_discover'); }
  async runExperiment(path) { return this.callTool('tumult_run_experiment', { experiment_path: path }); }
  async runGameDay(path) { return this.callTool('tumult_gameday_run', { gameday_path: path }); }
  async analyzeGameDay(path) { return this.callTool('tumult_gameday_analyze', { gameday_path: path }); }
  async storeStats() { return this.callTool('tumult_store_stats'); }
  async sql(query) { return this.callTool('tumult_analyze_store', { query }); }

  async #call(method, params) {
    const res = await this.#request(method, params);
    return this.#extractData(res);
  }

  async #request(method, params) {
    const headers = {
      'Content-Type': 'application/json',
      'Accept': 'text/event-stream, application/json',
    };
    if (this.#session) headers['mcp-session-id'] = this.#session;
    if (TOKEN) headers['Authorization'] = `Bearer ${TOKEN}`;

    return fetch(BASE, {
      method: 'POST',
      headers,
      body: JSON.stringify({
        jsonrpc: '2.0',
        id: ++this.#id,
        method,
        params,
      }),
    });
  }

  async #extractData(res) {
    const text = await res.text();
    const dataLine = text.split('\n').find(l => l.startsWith('data: '));
    if (!dataLine) throw new Error(`No data in response: ${text.slice(0, 200)}`);
    return JSON.parse(dataLine.slice(6));
  }
}

// ── Commands ────────────────────────────────────────────────

async function main() {
  const cmd = process.argv[2] || 'demo';
  const client = new TumultMCPClient();

  console.log('Connecting to tumult-mcp at', BASE);
  const { session, server } = await client.initialize();
  console.log(`Session: ${session}`);
  console.log(`Server: ${server.name} v${server.version}\n`);

  switch (cmd) {
    case 'discover': {
      console.log(await client.discover());
      break;
    }
    case 'experiment': {
      const path = process.argv[3] || 'examples/postgres-failover.toon';
      console.log(`Running experiment: ${path}\n`);
      console.log(await client.runExperiment(path));
      break;
    }
    case 'gameday': {
      const path = process.argv[3] || 'gamedays/q2-postgres-resilience.gameday.toon';
      console.log(`Running GameDay: ${path}\n`);
      console.log(await client.runGameDay(path));
      break;
    }
    case 'analyze': {
      const path = process.argv[3] || 'gamedays/q2-postgres-resilience.gameday.toon';
      console.log(await client.analyzeGameDay(path));
      break;
    }
    case 'sql': {
      const query = process.argv[3] || 'SELECT title, status, duration_ms FROM experiments ORDER BY started_at_ns DESC LIMIT 5';
      console.log(await client.sql(query));
      break;
    }
    case 'demo': {
      // Full demo — same flow as AQE's chaos-engineer agent
      console.log('=== Step 1: Discover capabilities ===');
      const discovery = await client.discover();
      const plugins = discovery.split('\n')[0];
      const actions = discovery.split('\n').filter(l => l.includes('::')).length;
      console.log(`${plugins} | ${actions} actions\n`);

      console.log('=== Step 2: Run single experiment ===');
      const journal = await client.runExperiment('examples/postgres-failover.toon');
      const status = journal.split('\n').find(l => l.startsWith('status:'));
      console.log(`${status}\n`);

      console.log('=== Step 3: Run GameDay ===');
      console.log(await client.runGameDay('gamedays/q2-postgres-resilience.gameday.toon'));
      console.log('');

      console.log('=== Step 4: Analyze results ===');
      console.log(await client.analyzeGameDay('gamedays/q2-postgres-resilience.gameday.toon'));
      console.log('');

      console.log('=== Step 5: Store statistics ===');
      console.log(await client.storeStats());
      console.log('');

      console.log('=== Step 6: Trend analysis (SQL) ===');
      console.log(await client.sql(
        'SELECT status, count(*) as runs, round(avg(duration_ms)) as avg_ms FROM experiments GROUP BY status ORDER BY runs DESC'
      ));
      break;
    }
    default:
      console.error(`Unknown command: ${cmd}`);
      console.error('Usage: node aqe-mcp-client.mjs [discover|experiment|gameday|analyze|sql|demo]');
      process.exit(1);
  }
}

main().catch(err => {
  console.error('Error:', err.message);
  process.exit(1);
});
