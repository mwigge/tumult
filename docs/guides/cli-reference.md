---
title: CLI Reference
parent: Guides
nav_order: 3
---

# CLI Reference

Tumult provides a single binary `tumult` with the following commands.

## tumult run

Execute a chaos experiment.

```
tumult run <experiment.toon> [OPTIONS]
```

| Option | Default | Description |
|--------|---------|-------------|
| `--journal-path <path>` | `journal.toon` | Output journal location |
| `--dry-run` | `false` | Validate and show plan without executing |
| `--rollback-strategy <s>` | `on-deviation` | `always`, `on-deviation`, or `never` (`deviated` is accepted as an alias for `on-deviation`) |
| `--baseline-mode <m>` | `full` | `full`, `skip`, or `only` |
| `--no-ingest` | `false` | Skip auto-ingestion into persistent analytics store |
| `--output-format <f>` | — | `json` — print journal as JSON to stdout after run |
| `--var KEY=VALUE` | — | Template variable substitution (repeatable) |

### Examples

```bash
# Basic run
tumult run experiment.toon

# Dry run — show plan without executing
tumult run experiment.toon --dry-run

# Custom journal path
tumult run experiment.toon --journal-path results/run-001.toon

# Always rollback regardless of outcome
tumult run experiment.toon --rollback-strategy always

# Skip baseline acquisition, use static tolerances
tumult run experiment.toon --baseline-mode skip

# Skip auto-ingest into persistent DuckDB store
tumult run experiment.toon --no-ingest

# Print journal as JSON to stdout (for piping/scripting)
tumult run experiment.toon --output-format json | jq '.status'

# Template variable substitution
tumult run experiment.toon --var env=staging --var cluster=eu-west-1
```

With the default `on-deviation` strategy, rollbacks run when the experiment
deviates from its hypothesis or when a method step fails after a fault was
injected. `always` runs them on every outcome; `never` skips them entirely.

### Template Variables

The `--var` flag substitutes `${KEY}` placeholders in the experiment's title and activity names before execution. This allows a single experiment template to be reused across environments:

```toon
title: Resilience test for ${env} cluster ${cluster}

method[1]:
  - name: kill-${env}-primary
    ...
```

```bash
tumult run template.toon --var env=production --var cluster=us-east-1
```

Undefined variables cause a hard error at startup, not at execution time.

### Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Experiment completed successfully |
| 1 | Experiment failed, deviated, interrupted, or aborted |

### Auto-Ingest

By default, `tumult run` writes the journal file **and** ingests experiment data into the persistent DuckDB store at `~/.tumult/lake.duckdb`. Pass `--no-ingest` to skip store ingestion (useful in CI pipelines that manage their own storage).

When `TUMULT_DAEMON_URL` is set (e.g. `http://localhost:4318`), the journal is POSTed to the daemon's `/api/import/journal` instead, so the write rides the daemon's single-writer channel rather than racing its store lock. If the daemon is unreachable (no HTTP response), the CLI falls back to the direct store write; any HTTP answer — including an error — is treated as final.

When `TUMULT_DAEMON_TOKEN` is set (a `kro_...` API token), the journal POST to the daemon sends `Authorization: Bearer <token>`. Unset means no header is sent, which matches a loopback dev daemon running without authentication.

The daemon can also execute experiments itself via its run-control API — `POST /api/runs/validate` (register a definition), `POST /api/runs/dry-run` (resolved plan preview), `POST /api/runs` (enqueue, bounded queue with 429 backpressure), `POST /api/runs/{id}/stop` (e-stop with rollbacks), `GET /api/runs[/{id}]` (state + audit trail), `GET /api/runs/{id}/audit/verify` (re-verify the audit hash chain). See ADR-011.

## tumult validate

Validate experiment syntax, structure, and plugin references.

```
tumult validate <experiment.toon>
```

Reports:
- Title, description, tags
- Method and rollback step counts
- Hypothesis probe count
- Phase 0/1 configuration presence
- Configuration and secret resolution status
- Template variable references (warns on undefined vars)

### Example

```bash
tumult validate experiment.toon
```

## tumult discover

List all available plugins and their actions — both script plugins
(discovered from the filesystem) and native plugins (compiled into the
binary), labeled `(script)` / `(native)`.

```
tumult discover [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--plugin <name>` | Show details for a specific plugin |

### Plugin Search Paths

Script plugins are discovered from (in order):

1. `./plugins/` — local to the experiment
2. `~/.tumult/plugins/` — user-global
3. `$TUMULT_PLUGIN_PATH` — custom paths (colon-separated)

At runtime you can override the search paths without modifying the binary.
Native plugins (`tumult-ssh`, `tumult-net`, `tumult-kubernetes`, `tumult-cloud`,
`tumult-windows`) are registered in the binary itself and are always listed.

### Examples

```bash
# List all plugins
tumult discover

# Show details for a specific plugin (script or native)
tumult discover --plugin tumult-kafka
tumult discover --plugin tumult-ssh
```

From the repository root (11 script plugins in `./plugins/` plus the 5
built-in native plugins):

```text
$ tumult discover
Discovered 16 plugin(s) (11 script, 5 native):

  tumult-cloud (native)
  tumult-containers (script)
  tumult-db-mysql (script)
  tumult-db-postgres (script)
  tumult-db-redis (script)
  tumult-kafka (script)
  tumult-kubernetes (native)
  tumult-loadtest (script)
  tumult-net (native)
  tumult-network (script)
  tumult-process (script)
  tumult-pumba (script)
  tumult-ssh (native)
  tumult-stress (script)
  tumult-timewarp (script)
  tumult-windows (native)

Actions: 91
  tumult-containers::kill-container
  ...
  tumult-kubernetes::delete_pod
  ...
  tumult-ssh::execute
  ...
```

## tumult init

Create a new experiment from a template.

```
tumult init [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--plugin <name>` | Reference a specific plugin name in the generated template |

Scaffolds `experiment.toon` in the current directory from a bundled, self-contained template (steady-state hypothesis, method, and rollbacks built only on `uname`/`sh`/`echo` — no Docker or network needed). This writes a static template; it does not prompt interactively.

### Example

```bash
tumult init
tumult init --plugin tumult-db-postgres
```

## tumult analyze

SQL analytics over journal files using embedded DuckDB.

```
tumult analyze [journals-dir] [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--query <sql>` | Custom SQL query |

If `journals-dir` is omitted, queries the persistent store at `~/.tumult/lake.duckdb`.

`--query` is read-only: only `SELECT` and `WITH` statements are accepted; anything else is rejected before execution.

### Examples

```bash
# Query persistent store (no path needed)
tumult analyze --query "SELECT status, count(*) FROM experiments GROUP BY status"

# Query a specific directory of journals
tumult analyze journals/ --query "SELECT title, duration_ms FROM experiments ORDER BY duration_ms DESC"

# Default query: experiment summary
tumult analyze journals/
```

## tumult export

Convert journal to other formats.

```
tumult export <journal.toon> [OPTIONS]
```

| Option | Default | Description |
|--------|---------|-------------|
| `--format <f>` | `parquet` | `parquet`, `arrow`, `csv`, or `json` |

## tumult compliance

Generate regulatory compliance reports.

```
tumult compliance <journals-dir> --framework <name>
```

Supported frameworks: `dora`, `nis2`, `pci-dss`, `iso-22301`, `iso-27001`, `soc2`, `basel-iii`

### Example

```bash
tumult compliance journals/ --framework dora
tumult compliance journals/ --framework pci-dss
```

## tumult trend

Cross-run trend analysis from the persistent store.

```
tumult trend <journals-dir> [OPTIONS]
```

| Option | Default | Description |
|--------|---------|-------------|
| `--metric <m>` | `resilience_score` | Metric to trend (`resilience_score`, `duration_ms`, `estimate_accuracy`, `method_step_count`) |
| `--last <window>` | — | Time window: `30d`, `90d`, etc. |
| `--target <tech>` | — | Filter by target system (matches experiment title) |

### Examples

```bash
tumult trend journals/ --metric duration_ms --last 30d
tumult trend journals/ --target postgresql --metric resilience_score
```

## tumult report

Generate HTML (or PDF-ready HTML) report from a journal.

```
tumult report <journal.toon> [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--output <path>` | Output file path (default: `report.html`) |
| `--format <f>` | `html` (default) |

## tumult import

Import journals from a Parquet backup directory.

```
tumult import <parquet-dir>
```

Transactional import — data is committed only if all files load successfully.

## tumult store

Manage the persistent analytics store.

```
tumult store <subcommand>
```

| Subcommand | Description |
|------------|-------------|
| `stats` | Show experiment/activity counts and store file size |
| `backup [--output <dir>]` | Dump store to Parquet files |
| `purge --older-than-days <N>` | Delete experiments older than N days |
| `path` | Print the store file path |
| `migrate` | Migrate data from DuckDB to ClickHouse backend |
| `import-legacy [--analytics-db <path>] [--kronika-db <path>] [--store <path>]` | Merge pre-unification databases (old analytics store and/or kronika lake) into the unified store; idempotent |

### Examples

```bash
tumult store stats
tumult store backup --output ~/tumult-backup-2026-03
tumult store purge --older-than-days 90
tumult store migrate   # requires TUMULT_CLICKHOUSE_URL
tumult store import-legacy --analytics-db ~/.tumult/analytics.duckdb
```

## tumult recommend

Recommend the next useful chaos experiment from deterministic heuristics over the analytics store (coverage gaps, failing experiments, stale experiments), optionally enhanced by a local agent CLI.

```
tumult recommend [OPTIONS]
```

| Flag | Description |
|------|-------------|
| `--goal <GOAL>` | Recommendation goal or operator intent |
| `--store-path <PATH>` | Analytics store path to inspect (default: persistent store) |
| `--model <MODEL>` | Model label to include in deterministic recommendation metadata |
| `--no-draft` | Do not include a draft TOON experiment |
| `--format <text\|json>` | Output format (default: `text`) |
| `--agent <NAME>` | Enhance recommendations with an agent CLI adapter (`claude-code`, `codex`) |
| `--agent-model <MODEL>` | Model override passed to the agent CLI (requires `--agent`) |
| `--agent-timeout <SECS>` | Agent CLI timeout in seconds (default: 120) |
| `--generate-experiments <DIR>` | Write validated agent-proposed experiments into `<DIR>` (requires `--agent`) |

With `--agent`, the heuristic output is printed first, followed by an "Agent-enhanced recommendations" section. With `--generate-experiments`, every proposed experiment is parsed and validated (`parse_experiment` + `validate_experiment`) before writing; valid ones are written to `<DIR>/<title-slug>.toon` (collisions get `-2`, `-3`, ... — never overwritten), invalid ones are rejected with the validation error and counted in a summary line. In JSON mode the output gains an `agent` object: `{ adapter, model, recommendations, experiments_written, experiments_rejected }`.

### Examples

```bash
# Deterministic heuristics only
tumult recommend --goal "harden the cache tier"

# Enhanced by Claude Code, generating experiment files
tumult recommend --agent claude-code --generate-experiments out/experiments

# Enhanced by Codex with model + timeout overrides, JSON output
tumult recommend --agent codex --agent-model gpt-5-codex --agent-timeout 300 --format json
```

See the [Agentic Recommendations guide](agentic-recommendations.md) for how the prompt is built and how the validation gate works.

## tumult agents

List agent CLI adapters and their detected state: name, installed, version, auth detail, and an install hint when the binary is missing.

```
tumult agents
```

```
ADAPTER        INSTALLED  VERSION    DETAIL
claude-code    yes        2.0.13     Authenticated via ANTHROPIC_API_KEY.
codex          no         -          Codex CLI not found on PATH. Install with: npm i -g @openai/codex
```

Binary resolution honors the `CLAUDE_CODE_BIN` / `CODEX_BIN` env overrides.

## tumult mcp serve

Start the MCP (Model Context Protocol) server from the main `tumult` binary. This is the recommended way to launch the server — it runs in-process, so no separate `tumult-mcp` executable needs to be installed alongside the CLI. (The standalone `tumult-mcp` binary below remains available and behaves identically.)

```
tumult mcp serve                                    # stdio (IDE integration)
tumult mcp serve --transport http --port 3100       # Streamable HTTP (containers, CI/CD)
tumult mcp serve --transport http --token my-secret # require bearer auth
```

| Option | Description |
|--------|-------------|
| `--transport <stdio\|http>` | Transport mode (default: `stdio`) |
| `--host <addr>` | Bind address for HTTP transport and health endpoint (default: `127.0.0.1`; a non-loopback bind such as `0.0.0.0` requires `--token`) |
| `--port <port>` | Port for the HTTP transport (default: `3100`) |
| `--health-port <port>` | Port for the `/health` endpoint (default: `port + 1`) |
| `--token <token>` | Require this bearer token on every request (sets `TUMULT_MCP_TOKEN`) |

The exposed tools, authentication, and data model are identical to the standalone binary documented next.

## tumult-mcp

Start the MCP (Model Context Protocol) server: a separate binary using stdio by default or Streamable HTTP. Equivalent to `tumult mcp serve`.

```
tumult-mcp                                # stdio (IDE integration)
tumult-mcp --transport http --port 3100   # Streamable HTTP (containers, CI/CD)
```

Exposes 40 tools to AI assistants, grouped by area:

| Area | Tools |
|---|---|
| Experiments | `tumult_run_experiment`, `tumult_validate`, `tumult_discover`, `tumult_create_experiment`, `tumult_list_experiments` |
| Journals and analytics | `tumult_read_journal`, `tumult_list_journals`, `tumult_analyze`, `tumult_analyze_store`, `tumult_store_stats`, `tumult_query_traces`, `tumult_report`, `tumult_compliance`, `tumult_trend` |
| GameDays | `tumult_gameday_run`, `tumult_gameday_analyze`, `tumult_gameday_create`, `tumult_gameday_list` |
| Intelligence | `tumult_recommend`, `tumult_coverage`, `tumult_agents`, `tumult_fault_catalog`, `tumult_scaffold_experiment` |
| Agentic testing | `tumult_agentic_list_scenarios`, `tumult_agentic_smoke`, `tumult_agentic_run_experiment` |
| ChaosGraph | `tumult_chaosgraph_query`, `tumult_chaosgraph_neighbors`, `tumult_chaosgraph_coverage_gaps`, `tumult_chaosgraph_cypher` |
| Topology | `tumult_topology_import`, `tumult_topology_map`, `tumult_compliance_lineage`, `tumult_recommend_injection` |
| Autopilot | `tumult_autopilot_run`, `tumult_autopilot_status`, `tumult_autopilot_respond`, `tumult_autopilot_export`, `tumult_autopilot_notify` |
| Access | `tumult_whoami` |

Every tool carries MCP tool annotations (`readOnlyHint` / `destructiveHint` / `idempotentHint` / `openWorldHint`), 30 tools return `structuredContent` with advertised output schemas, and workspace files are served as `tumult://journal|experiment|gameday/{file}` resources. The four destructive-annotated tools are `tumult_run_experiment`, `tumult_gameday_run`, `tumult_autopilot_run`, and `tumult_autopilot_respond`. See the [MCP Guide](mcp-guide.md) for the full data model.

Tool failures are returned with `isError: true` per the MCP specification. Authentication and rate-limit rejections are reported as such — not as "Unknown tool".

### Authentication

Set `TUMULT_MCP_TOKEN` to require bearer token auth on all tool calls. If not set, the server runs without authentication (log warning emitted).

```bash
TUMULT_MCP_TOKEN=my-secret tumult-mcp
```

Callers must pass `Authorization: Bearer my-secret` in MCP request metadata.

## tumult chaosgraph

Query the ChaosGraph knowledge graph — the typed node/edge model over accumulated chaos data that also backs the `chaosgraph_*` MCP tools. These commands read the analytics store directly, so an operator can explore the graph without an MCP client.

```
tumult chaosgraph query --kind <kind> [--filter <substr>]
tumult chaosgraph neighbors --node <id> [--rel <rel>] [--depth <n>]
tumult chaosgraph coverage-gaps [--framework <fw>] [--domain <plugin>]
```

| Option | Description |
|--------|-------------|
| `--kind <kind>` | Node kind to list: `experiment`, `fault`, `service`, `journal`, … |
| `--filter <substr>` | Case-insensitive label substring filter (query) |
| `--node <id>` | Node id to center on, e.g. `exp:My experiment` (neighbors) |
| `--rel <rel>` | Restrict traversal to one relation, e.g. `injects`, `targets` (neighbors) |
| `--depth <n>` | Traversal depth in hops (neighbors, default `1`) |
| `--framework <fw>` | Annotate gaps with a framework's still-unevidenced articles (coverage-gaps) |
| `--domain <plugin>` | Filter gaps to a fault domain / plugin (coverage-gaps) |
| `--format <text\|json>` | Output format (all; default `text`) |
| `--store <path>` | Analytics store path (all; default `~/.tumult/lake.duckdb`) |

### Examples

```bash
# Every fault primitive that has appeared in a run
tumult chaosgraph query --kind fault

# What one experiment touched — nodes and edges within 1 hop
tumult chaosgraph neighbors --node "exp:Redis resilience — verify recovery after disruption"

# Untested actions, with DORA articles still lacking evidence
tumult chaosgraph coverage-gaps --framework dora

# Structured output for scripting
tumult chaosgraph query --kind service --format json
```

The store must exist (run at least one experiment first); a missing store yields a clean `store not found` error.

## tumult new

Interactive experiment builder: pick a fault (domain → action → args → target → probe → title) and get a validated, ready-to-run experiment. With `--from <template>` it instantiates a curated starter non-interactively.

```
tumult new [--from <template>] [--set KEY=VALUE]... [--out <path>]
```

### Examples

```bash
# Interactive picker
tumult new

# Instantiate a curated starter with parameter overrides
tumult new --from postgres-connection-kill --set host=db.internal --out pg-kill.toon
```

## tumult templates

List the curated starter templates (name, description, params) accepted by `tumult new --from`.

```
tumult templates
```

## tumult agentic

Agentic AI fault-injection scenarios and local smoke tests — scenario packs, deterministic fixtures, multi-turn trajectories, and a live-traffic proxy.

```
tumult agentic <list-packs|smoke|run|trajectory|replay|proxy|run-live>
```

See the [Agentic Scenarios guide](agentic-scenarios.md) for pack authoring and the [Agentic Observability guide](agentic-observability.md) for trace capture.

## tumult gameday

Coordinated experiment campaigns with resilience scoring and compliance mapping.

```
tumult gameday <create|run|analyze>
```

| Subcommand | Description |
|------------|-------------|
| `create` | Create a `.gameday.toon` file from experiment paths |
| `run` | Run all experiments in a GameDay under shared load |
| `analyze` | Show aggregate analysis of a completed GameDay |

See the [Experiment Scheduling guide](scheduling.md) for recurring GameDays.

## tumult topology

Declared service topology, compliance lineage, and injection recommendations over the analytics store.

```
tumult topology <import|discover-k8s|map|lineage|recommend>
```

| Subcommand | Description |
|------------|-------------|
| `import` | Import a declared topology TOML (services + `depends_on`) into the store |
| `discover-k8s` | Propose a topology TOML from a live cluster (never writes the store) |
| `map` | Render the compliance-aware service map (text, Mermaid, or JSON) |
| `lineage` | Show the (article × service) compliance lineage matrix |
| `recommend` | Rank the next most valuable fault injections, with reasons |

See the [Topology guide](topology.md).

## tumult autopilot

Policy-gated autopilot: decide, record, and (only when told to) enact the next compliance-driven fault injections. Audit-before-act: decisions are persisted before anything runs.

```
tumult autopilot <once|status|approve|deny|notify-change|export>
```

| Subcommand | Description |
|------------|-------------|
| `once` | Run one pass of the decision loop (without `--execute` nothing is injected) |
| `status` | List recorded decisions with their latest lifecycle event |
| `approve` | Approve a proposed decision — runs its playbook experiment |
| `deny` | Deny a proposed decision — records veto feedback |
| `notify-change` | Record a deploy/config change event against a service |
| `export` | Export the decision and event tables as Parquet files |

See the [Autopilot guide](autopilot.md).

## tumult tui

Open the interactive analytics TUI over the store (read-only dashboard).

```
tumult tui [--store <path>] [--refresh-secs <n>]
```

## Environment Variables

| Variable | Description |
|----------|-------------|
| `TUMULT_PLUGIN_PATH` | Additional plugin search paths (colon-separated) |
| `TUMULT_OTEL_ENABLED` | Enable/disable OTel (default: `true`) |
| `TUMULT_OTEL_CONSOLE` | Print spans to console (default: `false`) |
| `RUST_LOG` | Tracing filter. When unset **and** no OTLP endpoint is configured, the CLI defaults it to `warn` to keep interactive output clean; set it explicitly (e.g. `info`) to see audit/telemetry logs |
| `TUMULT_MCP_TOKEN` | Bearer token for MCP server authentication |
| `CLAUDE_CODE_BIN` | Explicit path to the Claude Code binary for `recommend --agent` / `agents` |
| `CODEX_BIN` | Explicit path to the Codex binary for `recommend --agent` / `agents` |
| `TUMULT_CLICKHOUSE_URL` | ClickHouse URL for SigNoz cross-correlation mode |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | OTLP endpoint URL |
| `OTEL_SERVICE_NAME` | Service name for telemetry (default: `tumult`) |
| `DATABASE_HOST` / custom | Resolved via `configuration` blocks in experiment |
