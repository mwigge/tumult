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
```

### Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Experiment completed successfully |
| 1 | Experiment failed, deviated, or aborted |

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

## tumult analyze (Phase 2)

SQL analytics over journal files using embedded DuckDB.

```
tumult analyze <journals-dir> [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--query <sql>` | Custom SQL query |

## tumult export (Phase 2)

Convert journal to other formats.

```
tumult export <journal.toon> [OPTIONS]
```

| Option | Default | Description |
|--------|---------|-------------|
| `--format <f>` | `parquet` | `parquet`, `csv`, or `json` |

## tumult compliance (Phase 2)

Generate regulatory compliance reports.

```
tumult compliance <journals-dir> --framework <name>
```

Supported frameworks: `dora`, `nis2`, `pci-dss`, `iso-22301`, `iso-27001`, `soc2`, `basel-iii`

## tumult report (Phase 3)

Generate HTML report from a journal.

```
tumult report <journal.toon> [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--output <path>` | Output file path |

## Environment Variables

| Variable | Description |
|----------|-------------|
| `TUMULT_PLUGIN_PATH` | Additional plugin search paths (colon-separated) |
| `TUMULT_OTEL_ENABLED` | Enable/disable OTel (default: `true`) |
| `TUMULT_OTEL_CONSOLE` | Print spans to console (default: `false`) |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | OTLP endpoint URL |
| `OTEL_SERVICE_NAME` | Service name for telemetry (default: `tumult`) |
