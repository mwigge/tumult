---
title: CLI Reference
parent: Guides
nav_order: 3
---

# CLI Reference

Tumult provides a single binary `tumult` with the following commands.

## tumult run

Execute an experiment.

```
tumult run <experiment.toon> [OPTIONS]
```

| Option | Default | Description |
|--------|---------|-------------|
| `--journal-path <path>` | `journal.toon` | Output journal location |
| `--force` | `false` | Overwrite the journal file if it already exists |
| `--dry-run` | `false` | Validate and show plan without executing |
| `--rollback-strategy <s>` | `on-deviation` | `always`, `on-deviation`, or `never` (`deviated` is accepted as an alias for `on-deviation`) |
| `--baseline-mode <m>` | `full` | `full`/`skip`: declared tolerances; `only`: one probe-only observation, no fault/load/control/rollback |
| `--no-ingest` | `false` | Skip auto-ingestion into persistent analytics store |
| `--output-format <f>` | — | `json`: print the journal as JSON on stdout when the run has ended, also when it did not complete. Stdout then holds the journal only; the [summary](#summary) goes to stderr. Not with `--dry-run` |
| `--var KEY=VALUE` | — | Template variable substitution (repeatable) |
| `--target-version <v>` | — | The version of the target this run measures, recorded in the journal as `declared` (overrides `target.version`; an error when the definition has no `target` block). Without it and without `target.version`, a run that auto-ingests asks the daemon (or the local lake) once at its end which `service.version` the target's spans carried during the run and records it as `observed` |
| `--load <tool>` | — | Run a load test concurrently with the method (`k6` or `none`) |
| `--load-script <path>` | — | Path to load test script (k6 `.js`) |
| `--load-vus <n>` | — | Number of virtual users for the load test |
| `--load-duration <d>` | — | Load test duration (e.g. `30s`, `5m`) |

### Examples

```bash
# Basic run
tumult run experiment.toon

# Dry run — show the plan without executing: kind, target, recovery
# targets, objectives, load, hypothesis, guards, steps and rollbacks
tumult run experiment.toon --dry-run

# Custom journal path
tumult run experiment.toon --journal-path results/run-001.toon

# Always rollback regardless of outcome
tumult run experiment.toon --rollback-strategy always

# Use declared tolerances (automatic statistical acquisition is not integrated)
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

### Summary

When a run ends, it prints what it found.

```
Running experiment: Checkout answers while a slow job runs
Status: Completed
Duration: 3169ms
Method steps: 1 executed
Hypothesis: met before, met after
Objectives: 3 met, 1 breached, 1 inconclusive
  breached: a-million-requests: load.total_requests must be at least 1000000, observed 10611
  inconclusive: rpo: rpo must be at most 5, not measured
Load (oha): 10611 requests, 3531.4 requests/s, p50 0.5 ms, p95 0.8 ms, p99 1.4 ms, error rate 0.00 %, thresholds met
Journal written to: journal.toon
Journal SHA-256: e97edc233991dfdebf733d0af92a92e83b29c9710320016b7e814cfc2c80f474
Ingested into persistent analytics store
```

| Line | What it says |
|------|--------------|
| `Hypothesis` | Whether the steady state was `met`, `not met` or `not checked`, before and after the method |
| `Objectives` | How many [objectives](experiment-format.md) were met, breached or inconclusive. Each one that was not met gets a line with its rule and the value the run observed |
| `Load` | What the load measured in all. The line ends with `cut before its planned end` when the load was stopped early |
| `Journal SHA-256` | The hash of the journal file, the same value `sha256sum` prints |

A run without a steady state, objectives or a load has no line about it.

### Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Experiment completed successfully |
| 1 | Experiment failed, deviated, halted, interrupted, or aborted |

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

Note: `tumult validate` only validates **experiment** files — it does not validate `.gameday.toon` files (those are checked when you run `tumult gameday run`).

Reports:
- Title, description, tags
- Method and rollback step counts
- Hypothesis probe count
- Whether an estimate and a baseline block are present. A run does not use the baseline block, and validate says so
- Configuration and secret resolution status
- Template variable references (warns on undefined vars)

Validation fails when:
- a step, a guard or a control names a plugin, function or action that this binary does not have. The error lists every such step and what is available.
- an objective names a metric outside the [metric table](experiment-format.md#objectives).

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
| `--last <n>` | Show summary of the last N experiments (default: 1 if no `--query`) |
| `--all` | Show store-wide aggregate summary |

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

`json` writes the whole journal to `<journal>.json`. The other formats write
two tables in the working directory:

| File | Rows |
|------|------|
| `<journal>.<ext>` | The run: one row |
| `activities/<journal>.<ext>` | The steps of the run: one row per step |

Both tables have the column `experiment_id`. Each table has its own
directory, so a tool that reads every file of a directory as one table gets
the runs from the one and the steps from the other.

## tumult compliance

Generate regulatory compliance reports.

```
tumult compliance <journals-dir> --framework <name>
```

Supported frameworks: `dora`, `nis2`, `pci-dss`, `iso-22301`, `iso-27001`, `soc2`, `basel-iii`

The evidence summary counts two things:

| Line | What it counts |
|------|----------------|
| `Pass rate` | Of all the journals, the share whose run completed |
| `Recovery within 30s` | Of the runs that watched for recovery, the share whose target came back within 30 seconds |

A run watches for recovery when it has a steady state to probe after the
fault. A watched run whose target did not come back counts against the
share. When no run watched, the line says so, and the verdict ends with
`(pass-rate only)`.

### Example

```bash
tumult compliance journals/ --framework dora
tumult compliance journals/ --framework pci-dss
```

## tumult trend

Cross-run trend analysis over journal files. `trend` requires a `<journals>` path (directory or file) — it loads those journals into a temporary in-memory store for the query; it does not read the persistent store.

```
tumult trend <journals-dir> [OPTIONS]
```

| Option | Default | Description |
|--------|---------|-------------|
| `--metric <m>` | `resilience_score` | Metric to trend (`resilience_score`, `duration_ms`, `estimate_accuracy`, `method_step_count`) |
| `--last <window>` | — | Only the runs that started in this window: `30d`, `90d`, etc. |
| `--target <name>` | — | Only the runs on this target: the system or the technology the journal names (whole name, any case) |

The points and the summary line are over the same runs: the ones of the
window and of the target.

A journal names the target its experiment declares (since 3.0.2). With
`--target`, a journal that names no target is left out, and the output says
how many were.

### Examples

```bash
tumult trend journals/ --metric duration_ms --last 30d
tumult trend journals/ --target postgresql --metric resilience_score
```

## tumult report

Generate a report from a journal — HTML (default), PDF-ready HTML, JSON, or JUnit XML.

```
tumult report <journal.toon> [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--output <path>` | Output file path (default: `report.html`) |
| `--format <f>` | `html` (default), `pdf`, `json`, or `junit` |
| `--trace-ui-base <url>` | Base URL of a trace UI (e.g. Jaeger/Tempo) — HTML reports render each activity's `trace_id` as a clickable link (falls back to the `TUMULT_TRACE_UI_BASE` env var) |

The HTML report is one file that shows the whole journal: the halt, the
steady state, every step, the objectives, the load, the measurements, the
versions and the analysis. It follows the light or dark setting of the
browser. Set `SOURCE_DATE_EPOCH` (seconds) to fix the time the report says it
was generated at, so the same journal always gives the same file.

The report draws up to five charts: a timeline of the steps, the headroom
of each objective, the load latency, the load throughput with its error
rate, and the samples of each probe over time. The time a fault was active
is shaded. The charts need no network: the script that draws them is in
the file, which makes a report with a chart about 570 KB. A reader with
scripts off still gets every table.

The JUnit file has one test case per step, and one per check of the steady
state. A check that did not hold is a failed case, so a deviated run shows as a
failure in CI.

## tumult import

Import the Parquet directory `tumult store export` writes: the experiments
and activities, and with them, when the directory holds them, the load
evidence (`load_results.parquet` and `probe_samples.parquet`, both or
neither) and the run roots with the version history (`run_roots.parquet`
and `version_sightings.parquet`, both or neither), so that a performance
verdict can be computed from the imported runs. Rows the store already
holds are left as they are; importing a directory twice adds nothing. This
does not restore a complete operational backup; use `tumult store restore`
for that.

```
tumult import <parquet-dir>
```

Transactional import — each pair of files is committed only if both load successfully.

## tumult store

Manage the persistent analytics store.

```
tumult store <subcommand>
```

| Subcommand | Description |
|------------|-------------|
| `stats` | Show experiment/activity counts and store file size |
| `backup [--output <dir>]` | Copy the complete database and checksum manifest; stop writers first, protect credentials in backup |
| `export [--output <dir>]` | Write the Parquet directory `tumult import` reads (experiments, activities, load results, probe samples, run roots, version history); refuses a directory that already holds one |
| `restore --input <dir> --output <new-db>` | Verify a complete backup and restore into a new database; never overwrite an existing destination |
| `purge --older-than-days <N>` | Delete experiments older than N days, with their activities, load results, probe samples, NFR verdicts and own graph rows; telemetry, the run registry and audit trail, manual records, forecasts, sightings and autopilot tables are kept |
| `path` | Print the store file path |
| `migrate` | Migrate data from DuckDB to ClickHouse backend |
| `import-legacy [--analytics-db <path>] [--kronika-db <path>] [--store <path>]` | Import databases from older installs (old analytics store and/or kronika lake) into the current store; idempotent |

### Examples

```bash
tumult store stats
tumult store backup --output ~/tumult-backup-2026-10
tumult store restore --input ~/tumult-backup-2026-10 --output ~/tumult-restored.duckdb
tumult store purge --older-than-days 90
tumult store migrate   # requires TUMULT_CLICKHOUSE_URL
tumult store import-legacy --analytics-db ~/.tumult/analytics.duckdb
```

See [Data portability and recovery](data-portability.md) for archive coverage,
backup assets and restoration checks. A Parquet archive is not an operational
backup; ordinary queries do not automatically read cold archive files.

## tumult recommend

Recommend the next useful experiment from deterministic heuristics over the analytics store (coverage gaps, failing experiments, stale experiments), optionally enhanced by a local agent CLI.

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
| `--token <token>` | Require this bearer token on every request (sets `TUMULT_MCP_TOKEN`, mapped to the `operator` role) |
| `--auth-config <path>` | TOML auth config granting per-token roles (viewer/operator); overrides `--token` (sets `TUMULT_MCP_AUTH_CONFIG`) |

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
| `--refresh` | Also persist the derived gap sub-graph into the store so `chaosgraph query/neighbors` can navigate it — takes a write lock, so it conflicts with a running MCP server on the same store (coverage-gaps) |
| `--format <text\|json>` | Output format (all; default `text`) |
| `--store <path>` | Analytics store path (all; default `~/.tumult/lake.duckdb`) |

### Examples

```bash
# Every fault primitive that has appeared in a run
tumult chaosgraph query --kind fault

# What one experiment touched — nodes and edges within 1 hop
tumult chaosgraph neighbors --node "exp:Redis connectivity — verify basic key operations"

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

`tumult gameday run` writes its journal first and then exits with a code that says how the GameDay went.

| Code | Meaning |
|------|---------|
| 0 | The GameDay is `COMPLIANT` |
| 1 | The GameDay is `PARTIAL` or `NON-COMPLIANT`, or it could not run |

Before 3.0.2 the command exited with 0 for every GameDay that wrote a journal.

`tumult gameday run` keeps the journal of an earlier run (`<name>.gameday.journal.toon`): it stops before the first experiment unless you pass `--force`, as `tumult run` does.

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
| `approve` | Approve a proposed decision — its playbook runs only if the re-gate now answers enact |
| `deny` | Deny a proposed decision — records veto feedback |
| `notify-change` | Record a deploy/config change event against a service |
| `export` | Export the decision and event tables as Parquet files |

See the [Autopilot guide](autopilot.md).

## tumult gate

Ask the daemon's CI gate (`GET /api/gate/{target}`) for a target's resilience verdict and exit non-zero when it does not pass. Meant for a pipeline step before promotion.

```bash
export TUMULT_DAEMON_URL=http://tumultd.internal:4318
export TUMULT_DAEMON_TOKEN=kro_...   # viewer role is enough
tumult gate payments-api --min-score 70 --max-age-days 30 --env prod
```

| Flag | Description |
|------|-------------|
| `--min-score <0-100>` | Score floor (daemon default 70, the `good` band) |
| `--max-age-days <n>` | Evidence older than this is stale (daemon default 30) |
| `--env <name>` | Restrict the verdict to one environment (403 outside a scoped token's environments) |
| `--daemon-url <url>` | Overrides `TUMULT_DAEMON_URL` |
| `--fail-on fail\|warn\|never` | Which verdicts exit non-zero (default `fail`) |
| `--json` | Print the daemon's JSON answer |

Verdicts: `pass` (score at or above the floor, fresh evidence, no failing experiment), `warn` (stale evidence or a failed experiment on the latest run), `fail` (below the floor or no evidence). An unknown target is an error, never a silent pass. Pipeline templates for GitHub Actions and GitLab CI live in `deploy/ci/`.

## tumult perf

Ask the daemon for a target's performance regression verdict (`GET /api/perf/{target}`; with `--store`, `POST /api/perf/{target}/verdicts`): the latency samples of two of its `service.version`s inside one environment — by default the two deployed last — compared with a bootstrap confidence interval over whole runs, after what became of the requests. Exits 3 on a regression and, by default, 4 when the evidence gives no verdict. Meant for the pipeline step after a build has been deployed and exercised.

```bash
export TUMULT_DAEMON_URL=http://tumultd.internal:4318
export TUMULT_DAEMON_TOKEN=kro_...   # viewer reads; --store needs operator
tumult perf checkout --quantile p95 --margin-pct 5 --env staging --store
```

| Flag | Description |
|------|-------------|
| `--probe <series>` | The `probe_samples` series compared (daemon default `load.latency_p95_ms`) |
| `--quantile p50\|p95\|p99` | Quantile the verdict is keyed on (daemon default p95) |
| `--min-samples <n>` | Fewer samples on either side is inconclusive (daemon default 20; the daemon also asks for three runs a side) |
| `--min-runs <n>` | Fewer runs with samples on either side is inconclusive (daemon default 3; from 1 to 1000). An interval from fewer than 5 runs a side says so in the headline and among the reasons |
| `--seed <n>` | Seed of the resampling; the same runs and seed give the same interval (daemon default fixed) |
| `--margin-pct <n>` | Noise margin as a percentage of the baseline quantile (daemon default 5) |
| `--env <name>` | The environment the verdict is for, matched exactly (403 outside a scoped token's environments; 404 when the target has no telemetry tagged with it). Required when the target has versions in several environments and the named versions do not single one out |
| `--candidate <version>` | The version judged, instead of the one deployed latest. At most 128 characters, no control characters |
| `--baseline <version>` | The version compared against, instead of the one deployed just before the candidate. Same bounds |
| `--range <span>` | How far back runs and samples are read, such as `30d` or `12h` (daemon default `30d`). The order of the versions does not depend on it |
| `--include partial` | Keep the latency samples of aborted and truncated runs, which are left out by default |
| `--store` | Append the verdict to the daemon's `perf_verdicts` history (`POST`, operator role); without it the command only reads. A retry with the same evidence appends nothing and still counts as stored. Exits non-zero when the daemon did not store, whatever `--fail-on` says |
| `--daemon-url <url>` | Overrides `TUMULT_DAEMON_URL` |
| `--fail-on <verdicts>` | Which verdicts exit non-zero, comma-separated: `regressed`, `inconclusive`, or `never` alone (default `regressed,inconclusive`). Given without `--candidate`, the command warns on stderr that it gates on the version deployed latest |
| `--json` | Print the daemon's JSON answer |

| Exit code | Meaning |
|-----------|---------|
| `0` | `improved` or `unchanged`, or a verdict `--fail-on` lets pass |
| `1` | The command failed: daemon unreachable, an HTTP error, `--store` that did not store, or a verdict this build does not know |
| `2` | Usage error |
| `3` | `regressed` |
| `4` | `inconclusive` |

**Behaviour change.** The default was `--fail-on regressed`, under which `inconclusive` exited 0 and a regression exited 1. A pipeline that relied on that now fails on `inconclusive` with exit 4 and on a regression with exit 3. Pass `--fail-on regressed` to let `inconclusive` through again; a gate that passes because nothing could be measured is seldom what a pipeline wants.

Verdicts: `improved`, `unchanged`, `regressed` (slower beyond the margin, a failure share above the baseline's, or a candidate that left more than a tenth of its load windows unanswered while the baseline did not), `inconclusive` (fewer than three runs or twenty samples on a side, one version only, an interval too wide to tell, a significant shift whose size against the margin is unresolved, a baseline that lost more than a tenth of its load windows to silence, or a result that rests on a latency censored at the request timeout). With `--store` the verdict is appended to the lake's `perf_verdicts` table. See the [Performance Regression guide](performance-regression.md) for the rules.

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
| `TUMULT_DAEMON_URL` | Daemon URL (e.g. `http://localhost:4318`) — `tumult run` POSTs journals to the daemon's `/api/import/journal` instead of writing the store directly (see Auto-Ingest under `tumult run`) |
| `TUMULT_DAEMON_TOKEN` | `kro_...` API token sent as `Authorization: Bearer` on daemon journal POSTs |
| `CLAUDE_CODE_BIN` | Explicit path to the Claude Code binary for `recommend --agent` / `agents` |
| `CODEX_BIN` | Explicit path to the Codex binary for `recommend --agent` / `agents` |
| `TUMULT_CLICKHOUSE_URL` | ClickHouse URL for SigNoz cross-correlation mode |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | OTLP endpoint URL |
| `OTEL_SERVICE_NAME` | Service name for telemetry (default: `tumult`) |
| `DATABASE_HOST` / custom | Resolved via `configuration` blocks in experiment |
