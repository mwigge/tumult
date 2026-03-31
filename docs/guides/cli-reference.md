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

List all discovered plugins, actions, and probes.

```
tumult discover [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--plugin <name>` | Show details for a specific plugin |

### Plugin Search Paths

Plugins are discovered from (in order):

1. `./plugins/` — local to the experiment
2. `~/.tumult/plugins/` — user-global
3. `$TUMULT_PLUGIN_PATH` — custom paths (colon-separated)

At runtime you can override the search paths without modifying the binary.

### Examples

```bash
# List all plugins
tumult discover

# Show details for a specific plugin
tumult discover --plugin tumult-kafka
```

## tumult init

Create a new experiment from a template.

```
tumult init [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--plugin <name>` | Pre-fill template with a specific plugin's actions |

Creates `experiment.toon` in the current directory with a working template including steady-state hypothesis, method, and rollbacks.

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

## tumult mcp

Start the MCP (Model Context Protocol) server on stdio transport.

```
tumult mcp
```

Exposes 11 tools to AI assistants:

| Tool | Description |
|------|-------------|
| `tumult_run_experiment` | Execute an experiment and return the journal |
| `tumult_validate` | Validate experiment syntax and provider support |
| `tumult_analyze` | SQL query over journals via embedded DuckDB |
| `tumult_read_journal` | Read a TOON journal and return contents |
| `tumult_list_journals` | List .toon journal files in a directory |
| `tumult_discover` | List all plugins, actions, and probes |
| `tumult_create_experiment` | Create a new experiment from a template |
| `tumult_query_traces` | Query trace data for observability correlation |
| `tumult_store_stats` | Return persistent store statistics |
| `tumult_analyze_store` | SQL query directly against the persistent store |
| `tumult_list_experiments` | List experiment .toon files in a directory |

### Authentication

Set `TUMULT_MCP_TOKEN` to require bearer token auth on all tool calls. If not set, the server runs without authentication (log warning emitted).

```bash
TUMULT_MCP_TOKEN=my-secret tumult mcp
```

Callers must pass `Authorization: Bearer my-secret` in MCP request metadata.

## Environment Variables

| Variable | Description |
|----------|-------------|
| `TUMULT_PLUGIN_PATH` | Additional plugin search paths (colon-separated) |
| `TUMULT_OTEL_ENABLED` | Enable/disable OTel (default: `true`) |
| `TUMULT_OTEL_CONSOLE` | Print spans to console (default: `false`) |
| `TUMULT_MCP_TOKEN` | Bearer token for MCP server authentication |
| `TUMULT_CLICKHOUSE_URL` | ClickHouse URL for SigNoz cross-correlation mode |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | OTLP endpoint URL |
| `OTEL_SERVICE_NAME` | Service name for telemetry (default: `tumult`) |
| `DATABASE_HOST` / custom | Resolved via `configuration` blocks in experiment |
