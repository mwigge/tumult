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
| `--rollback-strategy <s>` | `deviated` | `always`, `deviated`, or `never` |
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

By default, `tumult run` writes the journal file **and** ingests experiment data into the persistent DuckDB store at `~/.tumult/analytics.duckdb`. Pass `--no-ingest` to skip store ingestion (useful in CI pipelines that manage their own storage).

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
Native plugins (`tumult-ssh`, `tumult-net`, `tumult-kubernetes`) are
registered in the binary itself and are always listed.

### Examples

```bash
# List all plugins
tumult discover

# Show details for a specific plugin (script or native)
tumult discover --plugin tumult-kafka
tumult discover --plugin tumult-ssh
```

From the repository root (10 script plugins in `./plugins/` plus the 3
built-in native plugins):

```text
$ tumult discover
Discovered 13 plugin(s) (10 script, 3 native):

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

Actions: 64
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
tumult init --plugin tumult-db
```

## tumult analyze

SQL analytics over journal files using embedded DuckDB.

```
tumult analyze [journals-dir] [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--query <sql>` | Custom SQL query |

If `journals-dir` is omitted, queries the persistent store at `~/.tumult/analytics.duckdb`.

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
| `--metric <m>` | `resilience_score` | Metric to trend (`resilience_score`, `recovery_time`, `duration_ms`) |
| `--last <window>` | — | Time window: `30d`, `90d`, etc. |
| `--target <tech>` | — | Filter by target system (matches experiment title) |

### Examples

```bash
tumult trend journals/ --metric recovery_time --last 30d
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

### Examples

```bash
tumult store stats
tumult store backup --output ~/tumult-backup-2026-03
tumult store purge --older-than-days 90
tumult store migrate   # requires TUMULT_CLICKHOUSE_URL
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
tumult mcp serve --transport http --port 3100       # HTTP/SSE (containers, CI/CD)
tumult mcp serve --transport http --token my-secret # require bearer auth
```

| Option | Description |
|--------|-------------|
| `--transport <stdio\|http>` | Transport mode (default: `stdio`) |
| `--host <addr>` | Bind address for HTTP transport and health endpoint (default: `0.0.0.0`) |
| `--port <port>` | Port for the HTTP transport (default: `3100`) |
| `--health-port <port>` | Port for the `/health` endpoint (default: `port + 1`) |
| `--token <token>` | Require this bearer token on every request (sets `TUMULT_MCP_TOKEN`) |

The exposed tools, authentication, and data model are identical to the standalone binary documented next.

## tumult-mcp

Start the MCP (Model Context Protocol) server — a separate binary, on stdio transport by default or HTTP/SSE. Equivalent to `tumult mcp serve`.

```
tumult-mcp                                # stdio (IDE integration)
tumult-mcp --transport http --port 3100   # HTTP/SSE (containers, CI/CD)
```

Exposes 24 tools to AI assistants:

| Tool | Description |
|------|-------------|
| `tumult_run_experiment` | Execute an experiment — persists the journal and auto-ingests it into the analytics store (`journal_path`, `no_ingest`, `store_path`, `format`) |
| `tumult_validate` | Validate experiment syntax and provider support |
| `tumult_analyze` | SQL query over journals via embedded DuckDB |
| `tumult_read_journal` | Read a journal as JSON (default) or raw TOON, full or summary |
| `tumult_list_journals` | List .toon journal files in a directory (paginated) |
| `tumult_discover` | List all plugins, actions, and probes |
| `tumult_create_experiment` | Create a new experiment from a template |
| `tumult_query_traces` | Query trace data for observability correlation |
| `tumult_store_stats` | Return persistent store statistics |
| `tumult_analyze_store` | SQL query directly against the persistent store |
| `tumult_list_experiments` | List experiment .toon files in a directory (paginated) |
| `tumult_report` | Render a journal as JSON or JUnit XML, inline or written to the workspace |
| `tumult_compliance` | Compliance summary and verdict for one of 7 frameworks (`dora`, `nis2`, `pci-dss`, `iso-22301`, `iso-27001`, `soc2`, `basel-iii`) |
| `tumult_trend` | Cross-run metric trend over journals with a direction verdict |
| `tumult_agents` | List agent CLI adapters (claude-code, codex) with install/version/auth state |
| `tumult_gameday_create` | Scaffold a `.gameday.toon` campaign (experiments, load config, framework) |
| `tumult_gameday_run` | Run a GameDay under shared load, return score and compliance status |
| `tumult_gameday_analyze` | Analyze a completed GameDay journal |
| `tumult_gameday_list` | List available `.gameday.toon` files (paginated) |
| `tumult_recommend` | Recommend what to test next — coverage gaps, failure patterns, stale experiments; optional agent enhancement (`agent`, `agent_model`, `agent_timeout_secs`, `generate_experiments_dir`) |
| `tumult_coverage` | Coverage report — plugins/actions/targets tested vs available |
| `tumult_agentic_list_scenarios` | List agentic fault-injection scenario packs (metadata only) |
| `tumult_agentic_smoke` | Run a deterministic local agentic smoke check |
| `tumult_agentic_run_experiment` | Run a bundled agentic experiment (metadata only) |

Every tool carries MCP tool annotations (`readOnlyHint` / `destructiveHint` / `idempotentHint` / `openWorldHint`), 16 tools return `structuredContent` with advertised output schemas, and workspace files are served as `tumult://journal|experiment|gameday/{file}` resources. See the [MCP Guide](mcp-guide.md) for the full data model.

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
| `--store <path>` | Analytics store path (all; default `~/.tumult/analytics.duckdb`) |

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
